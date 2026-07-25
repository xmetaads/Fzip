// ZIP writing: parallel compression, ZIP64, AES-256.

package main

import (
	"bufio"
	"bytes"
	"compress/flate"
	"encoding/binary"
	"fmt"
	"hash/crc32"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"time"
)

// 0xFFFFFFFF is the ZIP64 *sentinel*, not a valid stored value: a field whose
// real value equals it must be moved into a ZIP64 record. Hence >=, never >.
const zip64Limit = 0xFFFFFFFF

// Same reasoning for the 16-bit entry count in the end-of-archive record.
const zip64CountLimit = 0xFFFF

// The ZIP name-length field is 16 bits, so a longer name cannot be recorded.
const maxNameLen = 0xFFFF

// One file destined for the archive.
type Source struct {
	Disk  string
	Name  string
	Size  uint64
	Mtime int64
	IsDir bool
}

// A compressed entry, ready to be written out.
type Packed struct {
	Name    string
	Method  uint16
	CRC     uint32
	USize   uint64
	Data    []byte
	DosDate uint16
	DosTime uint16
	IsDir   bool
}

// Record used to build the central directory.
type CdEntry struct {
	Name      string
	Method    uint16
	CRC       uint32
	CSize     uint64
	USize     uint64
	Offset    uint64
	DosDate   uint16
	DosTime   uint16
	IsDir     bool
	Encrypted bool
	// ForceZip64 is set when the local header already committed to ZIP64
	// sentinels. The central record must then agree, even if the final sizes
	// turned out small.
	ForceZip64 bool
}

// ---------------- collecting source files ----------------

// archiveName normalises the stored name: forward slashes, no drive letter, no
// `..`, and a trailing slash on directories.
func archiveName(rel string, isDir bool) string {
	var parts []string
	for _, p := range strings.Split(filepath.ToSlash(rel), "/") {
		if p == "" || p == "." || p == ".." {
			continue
		}
		parts = append(parts, p)
	}
	s := strings.Join(parts, "/")
	if isDir && s != "" {
		s += "/"
	}
	return s
}

// expandWildcards expands `*` and `?` in the last component of each input path.
//
// Unix shells glob before the program ever sees the arguments. cmd.exe and
// PowerShell do not — they hand `photos\*` through verbatim, and passing that to
// the filesystem fails with "The filename, directory name, or volume label
// syntax is incorrect". A Windows command-line tool has to do this itself.
func expandWildcards(inputs []string) ([]string, error) {
	out := make([]string, 0, len(inputs))

	for _, input := range inputs {
		if !strings.ContainsAny(input, "*?") {
			out = append(out, input)
			continue
		}

		pattern := filepath.Base(input)
		dir := filepath.Dir(input)
		if dir == "" {
			dir = "."
		}
		// Only the final component may hold a wildcard. `a\*\b.txt` would need
		// a recursive walk with different semantics; say so rather than
		// silently matching nothing.
		if strings.ContainsAny(dir, "*?") {
			return nil, fmt.Errorf(
				"%s: wildcards are only supported in the last part of a path", input)
		}

		entries, err := os.ReadDir(dir)
		if err != nil {
			return nil, fmt.Errorf("cannot read %s: %w", dir, err)
		}
		found := 0
		for _, ent := range entries {
			if wildMatch(pattern, ent.Name()) {
				out = append(out, filepath.Join(dir, ent.Name()))
				found++
			}
		}
		if found == 0 {
			return nil, fmt.Errorf("%s matched nothing", input)
		}
	}
	return out, nil
}

func collect(inputs []string, opts *Options) ([]Source, error) {
	var out []Source

	// Never swallow the archive we are currently writing. It exists on disk the
	// moment we create it, and reading a file that is concurrently growing
	// produces a garbage entry of nondeterministic size.
	selfPath, selfErr := filepath.Abs(opts.Archive)
	isSelf := func(p string) bool {
		if selfErr != nil {
			return false
		}
		abs, err := filepath.Abs(p)
		return err == nil && strings.EqualFold(abs, selfPath)
	}

	expanded, err := expandWildcards(inputs)
	if err != nil {
		return nil, err
	}

	for _, input := range expanded {
		st, err := os.Stat(input)
		if err != nil {
			return nil, fmt.Errorf("cannot read %s: %w", input, err)
		}

		if !st.IsDir() {
			if isSelf(input) {
				continue
			}
			name := filepath.Base(input)
			if opts.Filter.Matches(name) {
				out = append(out, Source{
					Disk: input, Name: name, Size: uint64(st.Size()),
					Mtime: st.ModTime().Unix(), IsDir: false,
				})
			}
			continue
		}

		// Directory: keep the top folder name as the stored prefix.
		base := filepath.Dir(input)
		walkErr := filepath.WalkDir(input, func(path string, d fs.DirEntry, err error) error {
			if err != nil {
				fmt.Fprintf(os.Stderr, "fzip: warning: skipping %s: %v\n", path, err)
				return nil
			}
			rel, rerr := filepath.Rel(base, path)
			if rerr != nil {
				rel = path
			}
			name := archiveName(rel, d.IsDir())
			if name == "" || !opts.Filter.Matches(name) {
				return nil
			}
			if len(name) > maxNameLen {
				fmt.Fprintf(os.Stderr,
					"fzip: warning: skipping %s - name is %d bytes, over the ZIP limit of %d\n",
					path, len(name), maxNameLen)
				return nil
			}
			if !d.IsDir() && isSelf(path) {
				return nil
			}
			info, ierr := d.Info()
			if ierr != nil {
				return nil
			}
			size := uint64(0)
			if !d.IsDir() {
				size = uint64(info.Size())
			}
			out = append(out, Source{
				Disk: path, Name: name, Size: size,
				Mtime: info.ModTime().Unix(), IsDir: d.IsDir(),
			})
			return nil
		})
		if walkErr != nil {
			return nil, walkErr
		}
	}
	return out, nil
}

// ---------------- compression ----------------

func deflateBlock(data []byte, lvl int) []byte {
	var buf bytes.Buffer
	w, err := flate.NewWriter(&buf, lvl)
	if err != nil {
		return nil
	}
	if _, err := w.Write(data); err != nil {
		return nil
	}
	if err := w.Close(); err != nil {
		return nil
	}
	return buf.Bytes()
}

func packOne(src *Source, opts *Options, lvl int) (*Packed, error) {
	dosDate, dosTime := unixToDos(src.Mtime)

	if src.IsDir {
		return &Packed{Name: src.Name, Method: 0, DosDate: dosDate, DosTime: dosTime,
			IsDir: true}, nil
	}

	raw, err := os.ReadFile(src.Disk)
	if err != nil {
		return nil, fmt.Errorf("cannot read %s: %w", src.Disk, err)
	}

	crc := crc32.ChecksumIEEE(raw)
	usize := uint64(len(raw))

	method := uint16(0)
	data := raw
	if lvl != 0 {
		comp := deflateBlock(raw, lvl)
		// Store verbatim when compression does not help, as 7-Zip and WinRAR do.
		if len(comp) > 0 && len(comp) < len(raw) {
			method, data = 8, comp
		}
	}

	if opts.HasPassword {
		if data, err = aesEncrypt(data, opts.Password); err != nil {
			return nil, err
		}
	}

	return &Packed{Name: src.Name, Method: method, CRC: crc, USize: usize,
		Data: data, DosDate: dosDate, DosTime: dosTime}, nil
}

// ---------------- writing the file ----------------

func putU16(b *[]byte, v uint16) { *b = binary.LittleEndian.AppendUint16(*b, v) }
func putU32(b *[]byte, v uint32) { *b = binary.LittleEndian.AppendUint32(*b, v) }
func putU64(b *[]byte, v uint64) { *b = binary.LittleEndian.AppendUint64(*b, v) }

// zip64LocalExtra builds the ZIP64 extra field for a local header.
func zip64LocalExtra(usize, csize uint64) []byte {
	var e []byte
	putU16(&e, 0x0001)
	putU16(&e, 16)
	putU64(&e, usize)
	putU64(&e, csize)
	return e
}

// versionNeeded is "version needed to extract", in APPNOTE's tenths-of-a-version
// encoding. WinZip AES requires 5.1; ZIP64 requires 4.5; plain deflate 2.0.
func versionNeeded(encrypted, need64 bool) uint16 {
	switch {
	case encrypted:
		return 51
	case need64:
		return 45
	default:
		return 20
	}
}

// aesExtra builds the WinZip AE-2 / AES-256 extra field. The trailing method is
// patched by the caller to the real compression method.
func aesExtra(method uint16) []byte {
	var e []byte
	putU16(&e, 0x9901)
	putU16(&e, 7)
	putU16(&e, 2) // AE-2
	e = append(e, 'A', 'E')
	e = append(e, 3) // AES-256
	putU16(&e, method)
	return e
}

func writeLocal(w io.Writer, p *Packed, encrypted, need64 bool) (uint64, error) {
	name := []byte(p.Name)
	csize := uint64(len(p.Data))

	var extra []byte
	if need64 {
		extra = append(extra, zip64LocalExtra(p.USize, csize)...)
	}
	if encrypted {
		extra = append(extra, aesExtra(p.Method)...)
	}

	var hdr []byte
	putU32(&hdr, sigLocal)
	putU16(&hdr, versionNeeded(encrypted, need64))
	putU16(&hdr, 1<<11|boolBit(encrypted)) // UTF-8 names, plus the encrypted bit
	putU16(&hdr, methodField(encrypted, p.Method))
	putU16(&hdr, p.DosTime)
	putU16(&hdr, p.DosDate)
	putU32(&hdr, zeroIfEncrypted(encrypted, p.CRC)) // AE-2: CRC = 0
	putU32(&hdr, sentinelOr(need64, csize))
	putU32(&hdr, sentinelOr(need64, p.USize))
	putU16(&hdr, uint16(len(name)))
	putU16(&hdr, uint16(len(extra)))
	hdr = append(hdr, name...)
	hdr = append(hdr, extra...)

	if _, err := w.Write(hdr); err != nil {
		return 0, fmt.Errorf("write error: %w", err)
	}
	if _, err := w.Write(p.Data); err != nil {
		return 0, fmt.Errorf("write error: %w", err)
	}
	return uint64(len(hdr)) + csize, nil
}

func boolBit(b bool) uint16 {
	if b {
		return 1
	}
	return 0
}

func methodField(encrypted bool, method uint16) uint16 {
	if encrypted {
		return 99
	}
	return method
}

func zeroIfEncrypted(encrypted bool, crc uint32) uint32 {
	if encrypted {
		return 0
	}
	return crc
}

func sentinelOr(need64 bool, v uint64) uint32 {
	if need64 {
		return zip64Limit
	}
	return uint32(v)
}

func writeCentral(w io.Writer, entries []CdEntry) (uint64, error) {
	var total uint64
	for i := range entries {
		e := &entries[i]
		name := []byte(e.Name)
		need64 := e.ForceZip64 || e.USize >= zip64Limit || e.CSize >= zip64Limit ||
			e.Offset >= zip64Limit

		var extra []byte
		if need64 {
			var z []byte
			putU64(&z, e.USize)
			putU64(&z, e.CSize)
			putU64(&z, e.Offset)
			putU16(&extra, 0x0001)
			putU16(&extra, uint16(len(z)))
			extra = append(extra, z...)
		}
		if e.Encrypted {
			extra = append(extra, aesExtra(e.Method)...)
		}

		var hdr []byte
		putU32(&hdr, sigCDir)
		putU16(&hdr, 0x031E) // made by: Unix, ZIP 3.0
		putU16(&hdr, versionNeeded(e.Encrypted, need64))
		putU16(&hdr, 1<<11|boolBit(e.Encrypted))
		putU16(&hdr, methodField(e.Encrypted, e.Method))
		putU16(&hdr, e.DosTime)
		putU16(&hdr, e.DosDate)
		putU32(&hdr, zeroIfEncrypted(e.Encrypted, e.CRC))
		putU32(&hdr, sentinelOr(need64, e.CSize))
		putU32(&hdr, sentinelOr(need64, e.USize))
		putU16(&hdr, uint16(len(name)))
		putU16(&hdr, uint16(len(extra)))
		putU16(&hdr, 0) // comment length
		putU16(&hdr, 0) // disk number
		putU16(&hdr, 0) // internal attributes
		// External attributes: the high 16 bits carry the Unix mode. 0x41ED is
		// 040755 (drwxr-xr-x) and 0x81A4 is 100644 (rw-r--r--); the low bit
		// 0x10 is the DOS directory flag. Without the permission bits,
		// extracted folders come out unreadable on Unix.
		extAttr := uint32(0x81A40000)
		if e.IsDir {
			extAttr = 0x41ED0010
		}
		putU32(&hdr, extAttr)
		putU32(&hdr, sentinelOr(need64, e.Offset))
		hdr = append(hdr, name...)
		hdr = append(hdr, extra...)

		if _, err := w.Write(hdr); err != nil {
			return 0, fmt.Errorf("write error: %w", err)
		}
		total += uint64(len(hdr))
	}
	return total, nil
}

func writeEnd(w io.Writer, count, cdOffset, cdSize uint64) error {
	need64 := count >= zip64CountLimit || cdOffset >= zip64Limit || cdSize >= zip64Limit
	var buf []byte

	if need64 {
		e64At := cdOffset + cdSize
		putU32(&buf, sigEOCD64)
		putU64(&buf, 44) // size of the remaining record
		putU16(&buf, 0x031E)
		putU16(&buf, 45)
		putU32(&buf, 0)
		putU32(&buf, 0)
		putU64(&buf, count)
		putU64(&buf, count)
		putU64(&buf, cdSize)
		putU64(&buf, cdOffset)

		putU32(&buf, sigEOCD64Loc)
		putU32(&buf, 0)
		putU64(&buf, e64At)
		putU32(&buf, 1)
	}

	putU32(&buf, sigEOCD)
	putU16(&buf, 0)
	putU16(&buf, 0)
	putU16(&buf, uint16(min64(count, 0xFFFF)))
	putU16(&buf, uint16(min64(count, 0xFFFF)))
	putU32(&buf, uint32(min64(cdSize, zip64Limit)))
	putU32(&buf, uint32(min64(cdOffset, zip64Limit)))
	putU16(&buf, 0)

	if _, err := w.Write(buf); err != nil {
		return fmt.Errorf("write error: %w", err)
	}
	return nil
}

func min64(a, b uint64) uint64 {
	if a < b {
		return a
	}
	return b
}

// ---------------- the 'a' command ----------------

func runAdd(opts *Options) int {
	t0 := time.Now()

	if len(opts.Inputs) == 0 {
		fmt.Fprintln(os.Stderr, "fzip: no input files given")
		return ExitUsage
	}
	if _, err := os.Stat(opts.Archive); err == nil && !opts.AssumeYes {
		fmt.Fprintf(os.Stderr, "fzip: %s already exists - use -y to overwrite\n", opts.Archive)
		return ExitFatal
	}

	sources, err := collect(opts.Inputs, opts)
	if err != nil {
		fmt.Fprintf(os.Stderr, "fzip: %v\n", err)
		return ExitFatal
	}
	if len(sources) == 0 {
		fmt.Fprintln(os.Stderr, "fzip: nothing to add")
		return ExitFatal
	}

	var totalBytes uint64
	nFiles := 0
	for i := range sources {
		totalBytes += sources[i].Size
		if !sources[i].IsDir {
			nFiles++
		}
	}

	out, err := os.Create(opts.Archive)
	if err != nil {
		fmt.Fprintf(os.Stderr, "fzip: cannot create %s: %v\n", opts.Archive, err)
		return ExitFatal
	}
	w := bufio.NewWriterSize(out, 4*ioChunk)

	showBar := progressEnabled(opts.ForceProgress, opts.Quiet, opts.Verbose)
	progress := NewProgress(totalBytes, uint64(nFiles), showBar)

	if !opts.Quiet {
		fmt.Printf("Creating %s from %d files (%s)...\n",
			opts.Archive, nFiles, fmtSize(totalBytes))
	}
	progress.Start(t0)

	lvl := opts.Level
	workers := opts.Workers
	if workers <= 0 {
		workers = runtime.NumCPU()
	}

	var (
		offset uint64
		cd     []CdEntry
		errs   []string
		fatal  bool
	)

	// Batch to bound RAM: compress a batch in parallel, then write it in order.
	//
	// A buffered file does not cost its own size in RAM, it costs a multiple:
	// the raw bytes, the compression output buffer, and for encryption two more
	// copies. Budgeting on the raw size alone would overshoot --max-memory
	// several times over, so every decision below uses the multiple.
	budget := opts.MaxMemory
	if budget < 64<<20 {
		budget = 64 << 20
	}
	costFactor := uint64(2)
	if opts.HasPassword {
		costFactor = 4
	}
	streamAbove := budget / 2 / costFactor
	if streamAbove < streamThreshold {
		streamAbove = streamThreshold
	}

	var batch []*Source
	var batchBytes uint64

	// A failed write leaves an unknown number of bytes in the file, so offset
	// can no longer be trusted. Continuing would make a later streamed entry
	// seek backwards into the previous entry's payload and corrupt it silently.
	// Any write failure therefore aborts the whole archive.
	flushBatch := func() {
		if len(batch) == 0 {
			return
		}
		packed := make([]*Packed, len(batch))
		perr := make([]error, len(batch))
		var wg sync.WaitGroup
		sem := make(chan struct{}, workers)
		for i, s := range batch {
			wg.Add(1)
			go func(i int, s *Source) {
				defer wg.Done()
				sem <- struct{}{}
				defer func() { <-sem }()
				packed[i], perr[i] = packOne(s, opts, lvl)
			}(i, s)
		}
		wg.Wait()

		for i, p := range packed {
			if perr[i] != nil {
				// A source that could not be read is reported and skipped; the
				// archive itself is still consistent.
				errs = append(errs, perr[i].Error())
				continue
			}
			csize := uint64(len(p.Data))
			need64 := p.USize >= zip64Limit || csize >= zip64Limit || offset >= zip64Limit
			encrypted := opts.HasPassword && !p.IsDir
			written, werr := writeLocal(w, p, encrypted, need64)
			if werr != nil {
				errs = append(errs, werr.Error())
				fatal = true
				break
			}
			cd = append(cd, CdEntry{
				Name: p.Name, Method: p.Method, CRC: p.CRC, CSize: csize,
				USize: p.USize, Offset: offset, DosDate: p.DosDate, DosTime: p.DosTime,
				IsDir: p.IsDir, Encrypted: encrypted, ForceZip64: need64,
			})
			offset += written
			progress.AddBytes(p.USize)
			if !p.IsDir {
				progress.AddFile()
			}
		}
		batch = batch[:0]
	}

	for i := range sources {
		s := &sources[i]
		// Large files stream through compression and encryption alike, so RAM
		// stays flat no matter how big the input or whether -p was given.
		if s.Size >= streamAbove && !s.IsDir {
			flushBatch()
			if fatal {
				break
			}
			batchBytes = 0
			entry, written, serr := streamAdd(w, out, s, opts, offset, progress)
			if serr != nil {
				errs = append(errs, serr.Error())
				fatal = true
				break
			}
			offset += written
			cd = append(cd, entry)
			progress.AddFile()
			continue
		}

		batch = append(batch, s)
		batchBytes += s.Size * costFactor
		if batchBytes >= budget {
			flushBatch()
			if fatal {
				break
			}
			batchBytes = 0
		}
	}
	if !fatal {
		flushBatch()
	}

	if fatal {
		w.Flush()
		out.Close()
		progress.Stop()
		for _, e := range errs {
			fmt.Fprintf(os.Stderr, "fzip: ERROR: %s\n", e)
		}
		// Half an archive is worse than none: a reader would accept it silently.
		os.Remove(opts.Archive)
		fmt.Fprintf(os.Stderr, "fzip: writing failed - removed the incomplete archive %s\n",
			opts.Archive)
		return ExitFatal
	}

	cdOffset := offset
	cdSize, cerr := writeCentral(w, cd)
	if cerr != nil {
		errs = append(errs, cerr.Error())
		cdSize = 0
	}
	if eerr := writeEnd(w, uint64(len(cd)), cdOffset, cdSize); eerr != nil {
		errs = append(errs, eerr.Error())
	}
	if ferr := w.Flush(); ferr != nil {
		errs = append(errs, fmt.Sprintf("write error: %v", ferr))
	}
	if cerr := out.Close(); cerr != nil {
		errs = append(errs, fmt.Sprintf("write error: %v", cerr))
	}

	progress.Stop()

	for _, e := range errs {
		fmt.Fprintf(os.Stderr, "fzip: ERROR: %s\n", e)
	}

	if !opts.Quiet {
		secs := time.Since(t0).Seconds()
		var archiveSize uint64
		if st, err := os.Stat(opts.Archive); err == nil {
			archiveSize = uint64(st.Size())
		}
		ratio := 0.0
		if totalBytes > 0 {
			ratio = float64(archiveSize) / float64(totalBytes) * 100
		}
		var speed uint64
		if secs > 0 {
			speed = uint64(float64(totalBytes) / secs)
		}
		fmt.Printf("Done: %d files, %s -> %s (%.1f%%) in %.3fs (%s/s)\n",
			nFiles, fmtSize(totalBytes), fmtSize(archiveSize), ratio, secs, fmtSize(speed))
		if opts.HasPassword {
			fmt.Println("  encrypted with AES-256")
		}
	}

	if len(errs) == 0 {
		return ExitOK
	}
	return ExitFatal
}

// streamAdd compresses one very large file as a stream, writing straight into
// the archive and patching the sizes in afterwards.
func streamAdd(w *bufio.Writer, file *os.File, s *Source, opts *Options,
	offset uint64, progress *Progress) (CdEntry, uint64, error) {

	dosDate, dosTime := unixToDos(s.Mtime)
	lvl := opts.Level
	method := uint16(8)
	if lvl == 0 {
		method = 0
	}
	encrypted := opts.HasPassword

	name := []byte(s.Name)
	extra := zip64LocalExtra(0, 0)
	if encrypted {
		extra = append(extra, aesExtra(method)...)
	}

	var hdr []byte
	putU32(&hdr, sigLocal)
	putU16(&hdr, versionNeeded(encrypted, true))
	putU16(&hdr, 1<<11|boolBit(encrypted))
	putU16(&hdr, methodField(encrypted, method))
	putU16(&hdr, dosTime)
	putU16(&hdr, dosDate)
	putU32(&hdr, 0) // CRC: patched later, and stays 0 for AE-2
	putU32(&hdr, zip64Limit)
	putU32(&hdr, zip64Limit)
	putU16(&hdr, uint16(len(name)))
	putU16(&hdr, uint16(len(extra)))
	hdr = append(hdr, name...)
	// The ZIP64 field is written first, so this offset still points at it.
	extraAt := offset + uint64(len(hdr))
	crcAt := offset + 14
	hdr = append(hdr, extra...)
	if _, err := w.Write(hdr); err != nil {
		return CdEntry{}, 0, fmt.Errorf("write error: %w", err)
	}

	f, err := os.Open(s.Disk)
	if err != nil {
		return CdEntry{}, 0, fmt.Errorf("cannot read %s: %w", s.Disk, err)
	}
	defer f.Close()

	// Read -> [deflate] -> [AES] -> file, one chunk at a time, so peak memory
	// stays at one buffer regardless of how large the file is.
	sink, err := newPayload(w, opts.Password, opts.HasPassword, lvl, method)
	if err != nil {
		return CdEntry{}, 0, err
	}

	h := crc32.NewIEEE()
	buf := make([]byte, ioChunk)
	var usize uint64
	for {
		n, rerr := f.Read(buf)
		if n > 0 {
			h.Write(buf[:n])
			usize += uint64(n)
			if _, werr := sink.Write(buf[:n]); werr != nil {
				return CdEntry{}, 0, fmt.Errorf("write error: %w", werr)
			}
			progress.AddBytes(uint64(n))
		}
		if rerr == io.EOF {
			break
		}
		if rerr != nil {
			return CdEntry{}, 0, fmt.Errorf("read error: %w", rerr)
		}
	}
	csize, err := sink.Finish()
	if err != nil {
		return CdEntry{}, 0, err
	}
	crc := h.Sum32()

	// Seek back and fill in the real sizes and CRC.
	if err := w.Flush(); err != nil {
		return CdEntry{}, 0, fmt.Errorf("write error: %w", err)
	}
	end, err := file.Seek(0, io.SeekCurrent)
	if err != nil {
		return CdEntry{}, 0, fmt.Errorf("seek error: %w", err)
	}
	if !encrypted {
		// AE-2 defines the CRC field as zero; only patch it for plain entries.
		var cb []byte
		putU32(&cb, crc)
		if _, err := file.WriteAt(cb, int64(crcAt)); err != nil {
			return CdEntry{}, 0, fmt.Errorf("seek error: %w", err)
		}
	}
	var z []byte
	putU16(&z, 0x0001)
	putU16(&z, 16)
	putU64(&z, usize)
	putU64(&z, csize)
	if _, err := file.WriteAt(z, int64(extraAt)); err != nil {
		return CdEntry{}, 0, fmt.Errorf("seek error: %w", err)
	}
	if _, err := file.Seek(end, io.SeekStart); err != nil {
		return CdEntry{}, 0, fmt.Errorf("seek error: %w", err)
	}

	return CdEntry{
		Name: s.Name, Method: method, CRC: crc, CSize: csize, USize: usize,
		Offset: offset, DosDate: dosDate, DosTime: dosTime,
		// The streamed local header is always written in ZIP64 form, so the
		// central directory entry must match it regardless of final size.
		ForceZip64: true, IsDir: false, Encrypted: encrypted,
	}, uint64(end) - offset, nil
}

// payload is the compress-then-encrypt chain under a streamed entry. Both
// stages are optional, so this models all four combinations without duplicating
// the read loop.
type payload struct {
	count *countingWriter
	aes   *aesWriter    // nil when not encrypting
	fw    *flate.Writer // nil when storing
	top   io.Writer
}

func newPayload(w io.Writer, password string, hasPassword bool, lvl int,
	method uint16) (*payload, error) {

	p := &payload{count: &countingWriter{inner: w}}
	var under io.Writer = p.count

	if hasPassword {
		aw, err := newAesWriter(p.count, password)
		if err != nil {
			return nil, err
		}
		p.aes = aw
		under = aw
	}
	if method != 0 {
		fw, err := flate.NewWriter(under, lvl)
		if err != nil {
			return nil, fmt.Errorf("cannot start compression: %w", err)
		}
		p.fw = fw
		under = fw
	}
	p.top = under
	return p, nil
}

func (p *payload) Write(b []byte) (int, error) { return p.top.Write(b) }

// Finish flushes every stage and reports how many bytes reached the file.
func (p *payload) Finish() (uint64, error) {
	if p.fw != nil {
		if err := p.fw.Close(); err != nil {
			return 0, fmt.Errorf("write error: %w", err)
		}
	}
	if p.aes != nil {
		if _, err := p.aes.Finish(); err != nil {
			return 0, err
		}
	}
	return p.count.count, nil
}

// countingWriter counts bytes written, so a streamed entry's output size is known.
type countingWriter struct {
	inner io.Writer
	count uint64
}

func (c *countingWriter) Write(b []byte) (int, error) {
	n, err := c.inner.Write(b)
	c.count += uint64(n)
	return n, err
}
