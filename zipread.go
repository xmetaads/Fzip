// ZIP reading: structure parsing, decryption, parallel decompression.

package main

import (
	"bytes"
	"compress/flate"
	"encoding/binary"
	"errors"
	"fmt"
	"hash/crc32"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
	"sync"
	"sync/atomic"
	"time"
	"unicode/utf8"
)

const (
	sigEOCD      = 0x06054b50
	sigEOCD64Loc = 0x07064b50
	sigEOCD64    = 0x06064b50
	sigCDir      = 0x02014b50
	sigLocal     = 0x04034b50
)

// Above this size an entry streams to disk instead of buffering in RAM.
const streamThreshold = 32 * 1024 * 1024
const ioChunk = 1024 * 1024

// A declared uncompressed size beyond this is treated as corrupt rather than
// used to size a read limit: no real file is four exabytes, and the field is
// attacker-controlled.
const maxDeclaredSize = 1 << 62

type AesInfo struct {
	VendorVersion uint16
	Strength      byte
	RealMethod    uint16
}

type Entry struct {
	Name     string
	Method   uint16
	Flags    uint16
	CRC      uint32
	CSize    uint64
	USize    uint64
	LocalOff uint64
	DosTime  uint16
	DosDate  uint16
	IsDir    bool
	Aes      *AesInfo
}

func (e *Entry) Encrypted() bool { return e.Flags&1 != 0 }

// RealMethod is the true compression method; for AES it lives in the extra field.
func (e *Entry) RealMethod() uint16 {
	if e.Aes != nil {
		return e.Aes.RealMethod
	}
	return e.Method
}

func (e *Entry) MethodName() string {
	switch e.RealMethod() {
	case 0:
		return "Store"
	case 8:
		return "Deflate"
	case 9:
		return "Deflate64"
	case 12:
		return "BZip2"
	case 14:
		return "LZMA"
	case 93:
		return "Zstd"
	case 95:
		return "XZ"
	case 96:
		return "JPEG"
	case 98:
		return "PPMd"
	default:
		return "?"
	}
}

func (e *Entry) EncName() string {
	if !e.Encrypted() {
		return ""
	}
	if e.Aes != nil {
		switch e.Aes.Strength {
		case 1:
			return "AES128"
		case 2:
			return "AES192"
		default:
			return "AES256"
		}
	}
	return "ZipCrypto"
}

// ---------------- little-endian readers ----------------

func u16le(b []byte, o int) uint16 { return binary.LittleEndian.Uint16(b[o:]) }
func u32le(b []byte, o int) uint32 { return binary.LittleEndian.Uint32(b[o:]) }
func u64le(b []byte, o int) uint64 { return binary.LittleEndian.Uint64(b[o:]) }

// ---------------- structure parsing ----------------

func findEOCD(data []byte) (int, bool) {
	n := len(data)
	if n < 22 {
		return 0, false
	}
	scanStart := 0
	if n > 22+65535 {
		scanStart = n - 22 - 65535
	}
	for i := n - 22; ; i-- {
		if u32le(data, i) == sigEOCD {
			clen := int(u16le(data, i+20))
			if i+22+clen <= n {
				return i, true
			}
		}
		if i == scanStart {
			return 0, false
		}
	}
}

func locateCentralDir(data []byte) (cdOff, count uint64, err error) {
	eocd, ok := findEOCD(data)
	if !ok {
		return 0, 0, errors.New("not a valid ZIP file (no end-of-archive record found)")
	}

	count = uint64(u16le(data, eocd+10))
	cdOff = uint64(u32le(data, eocd+16))
	need64 := count == 0xFFFF || cdOff == 0xFFFFFFFF

	if eocd >= 20 && u32le(data, eocd-20) == sigEOCD64Loc {
		// Offsets here are attacker-controlled. Compare in uint64 so a hostile
		// value cannot wrap the addition and slip past the bounds check.
		e64Off := u64le(data, eocd-20+8)
		if e64Off+56 >= e64Off && e64Off+56 <= uint64(len(data)) &&
			u32le(data, int(e64Off)) == sigEOCD64 {
			e64 := int(e64Off)
			count = u64le(data, e64+32)
			cdOff = u64le(data, e64+48)
		} else if need64 {
			return 0, 0, errors.New("corrupt ZIP64 file (bad end-of-archive location)")
		}
	} else if need64 {
		return 0, 0, errors.New("corrupt ZIP64 file (missing ZIP64 locator)")
	}

	if cdOff+22 < cdOff || cdOff+22 > uint64(len(data)) {
		return 0, 0, errors.New("corrupt archive (central directory lies outside the file)")
	}
	return cdOff, count, nil
}

func parseZip(data []byte) ([]Entry, error) {
	cdOff, count, err := locateCentralDir(data)
	if err != nil {
		return nil, err
	}
	// A central directory record is at least 46 bytes, so the file size is a
	// hard upper bound on the entry count. Reserving from the declared count
	// alone would let a tiny hostile archive demand a huge allocation.
	plausible := uint64(len(data)/46 + 1)
	capHint := count
	if capHint > plausible {
		capHint = plausible
	}
	entries := make([]Entry, 0, capHint)
	p := int(cdOff)

	for idx := uint64(0); idx < count; idx++ {
		if p < 0 || p+46 > len(data) || u32le(data, p) != sigCDir {
			return nil, fmt.Errorf("corrupt central directory at entry %d", idx)
		}
		flags := u16le(data, p+8)
		method := u16le(data, p+10)
		dosTime := u16le(data, p+12)
		dosDate := u16le(data, p+14)
		crc := u32le(data, p+16)
		csize := uint64(u32le(data, p+20))
		usize := uint64(u32le(data, p+24))
		nameLen := int(u16le(data, p+28))
		extraLen := int(u16le(data, p+30))
		commentLen := int(u16le(data, p+32))
		extAttrs := u32le(data, p+38)
		localOff := uint64(u32le(data, p+42))

		if p+46+nameLen+extraLen+commentLen > len(data) {
			return nil, fmt.Errorf("corrupt central directory at entry %d", idx)
		}
		nameRaw := data[p+46 : p+46+nameLen]
		extra := data[p+46+nameLen : p+46+nameLen+extraLen]

		var aesInfo *AesInfo
		for q := 0; q+4 <= len(extra); {
			id := u16le(extra, q)
			sz := int(u16le(extra, q+2))
			if q+4+sz > len(extra) {
				break
			}
			switch {
			case id == 0x0001:
				r := q + 4
				if usize == 0xFFFFFFFF && r+8 <= q+4+sz {
					usize = u64le(extra, r)
					r += 8
				}
				if csize == 0xFFFFFFFF && r+8 <= q+4+sz {
					csize = u64le(extra, r)
					r += 8
				}
				if localOff == 0xFFFFFFFF && r+8 <= q+4+sz {
					localOff = u64le(extra, r)
				}
			case id == 0x9901 && sz >= 7:
				aesInfo = &AesInfo{
					VendorVersion: u16le(extra, q+4),
					Strength:      extra[q+8],
					RealMethod:    u16le(extra, q+9),
				}
			}
			q += 4 + sz
		}

		name := decodeName(nameRaw, flags&(1<<11) != 0)
		// A directory entry may end in either separator. The spec says `/`, but
		// plenty of Windows producers write `\`, and treating one of those as a
		// file makes fzip try to create a file over an existing directory —
		// which fails with "Access is denied" and loses the whole subtree.
		isDir := strings.HasSuffix(name, "/") || strings.HasSuffix(name, `\`) ||
			(extAttrs&0x10 != 0 && usize == 0)

		entries = append(entries, Entry{
			Name: name, Method: method, Flags: flags, CRC: crc,
			CSize: csize, USize: usize, LocalOff: localOff,
			DosTime: dosTime, DosDate: dosDate, IsDir: isDir, Aes: aesInfo,
		})
		p += 46 + nameLen + extraLen + commentLen
	}
	return entries, nil
}

// dataStart locates an entry's payload, validating every attacker-controlled
// offset in uint64 so a crafted value cannot wrap and index past the mapping.
func dataStart(data []byte, e *Entry) (int, error) {
	if e.LocalOff+30 < e.LocalOff || e.LocalOff+30 > uint64(len(data)) {
		return 0, fmt.Errorf("%s: corrupt local header", e.Name)
	}
	off := int(e.LocalOff)
	if u32le(data, off) != sigLocal {
		return 0, fmt.Errorf("%s: corrupt local header", e.Name)
	}
	nameLen := uint64(u16le(data, off+26))
	extraLen := uint64(u16le(data, off+28))
	start := e.LocalOff + 30 + nameLen + extraLen
	if start+e.CSize < start || start+e.CSize > uint64(len(data)) {
		return 0, fmt.Errorf("%s: data extends past end of file", e.Name)
	}
	return int(start), nil
}

// ---------------- entry names ----------------

var cp437High = [128]rune{
	'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å',
	'É', 'æ', 'Æ', 'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', '¢', '£', '¥', '₧', 'ƒ',
	'á', 'í', 'ó', 'ú', 'ñ', 'Ñ', 'ª', 'º', '¿', '⌐', '¬', '½', '¼', '¡', '«', '»',
	'░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕', '╣', '║', '╗', '╝', '╜', '╛', '┐',
	'└', '┴', '┬', '├', '─', '┼', '╞', '╟', '╚', '╔', '╩', '╦', '╠', '═', '╬', '╧',
	'╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘', '┌', '█', '▄', '▌', '▐', '▀',
	'α', 'ß', 'Γ', 'π', 'Σ', 'σ', 'µ', 'τ', 'Φ', 'Θ', 'Ω', 'δ', '∞', 'φ', 'ε', '∩',
	'≡', '±', '≥', '≤', '⌠', '⌡', '÷', '≈', '°', '∙', '·', '√', 'ⁿ', '²', '■', ' ',
}

func decodeName(raw []byte, utf8Flag bool) string {
	if utf8Flag || utf8.Valid(raw) {
		return string(raw)
	}
	var b strings.Builder
	for _, c := range raw {
		if c < 0x80 {
			b.WriteByte(c)
		} else {
			b.WriteRune(cp437High[c-0x80])
		}
	}
	return b.String()
}

// ---------------- decompressing one entry ----------------

func unsupported(e *Entry) error {
	return fmt.Errorf("%s: compression method %d (%s) is not supported",
		e.Name, e.RealMethod(), e.MethodName())
}

// readLimit is the number of bytes worth reading before an entry has proved it
// understated its size. Reading one byte past the declared size is enough.
func (e *Entry) readLimit() int64 {
	n := e.USize
	if n > maxDeclaredSize {
		n = maxDeclaredSize
	}
	return int64(n) + 1
}

// inflateAll decompresses fully into memory: used for small entries and
// encrypted ones, whose HMAC covers the whole payload.
//
// The size cap matters: without it a few hundred bytes of deflate can expand to
// gigabytes, because the declared size is only a hint and a hostile archive
// simply understates it.
func inflateAll(raw []byte, e *Entry, method uint16) ([]byte, error) {
	switch method {
	case 0:
		out := make([]byte, len(raw))
		copy(out, raw)
		return out, nil
	case 8:
		fr := flate.NewReader(bytes.NewReader(raw))
		defer fr.Close()
		var buf bytes.Buffer
		if e.USize < 1<<26 {
			buf.Grow(int(e.USize))
		}
		if _, err := io.Copy(&buf, io.LimitReader(fr, e.readLimit())); err != nil {
			return nil, fmt.Errorf("%s: deflate error: %w", e.Name, err)
		}
		if uint64(buf.Len()) > e.USize {
			return nil, fmt.Errorf(
				"%s: entry expands beyond its declared size of %s - refusing to continue",
				e.Name, fmtSize(e.USize))
		}
		return buf.Bytes(), nil
	default:
		return nil, unsupported(e)
	}
}

// decryptEntry decrypts an entry, returning data that is still COMPRESSED.
func decryptEntry(raw []byte, e *Entry, password string, hasPassword bool) ([]byte, error) {
	if !hasPassword {
		return nil, fmt.Errorf("%s: encrypted - use -p <password>", e.Name)
	}

	if e.Aes != nil {
		out, err := aesDecrypt(raw, e.Aes.Strength, password)
		switch {
		case err == nil:
			return out, nil
		case errors.Is(err, errWrongPassword):
			return nil, fmt.Errorf("%s: wrong password", e.Name)
		case errors.Is(err, errTampered):
			return nil, fmt.Errorf(
				"%s: authentication failed - file was modified or corrupted", e.Name)
		default:
			return nil, fmt.Errorf("%s: encrypted data too short", e.Name)
		}
	}

	if e.Flags&(1<<6) != 0 {
		return nil, fmt.Errorf("%s: PKWARE strong encryption is not supported", e.Name)
	}
	if e.Method == 99 {
		return nil, fmt.Errorf("%s: AES entry is missing its header (corrupt file)", e.Name)
	}

	check := byte(e.CRC >> 24)
	if e.Flags&(1<<3) != 0 {
		check = byte(e.DosTime >> 8)
	}
	out, err := zipCryptoDecrypt(raw, password, check)
	if err != nil {
		if errors.Is(err, errWrongPassword) {
			return nil, fmt.Errorf("%s: wrong password", e.Name)
		}
		return nil, fmt.Errorf("%s: encrypted data too short", e.Name)
	}
	return out, nil
}

// streamTo decompresses into w, keeping memory flat. Returns (crc, bytes).
//
// Streaming to disk turns an unbounded expansion into a full volume rather than
// an out-of-memory abort, so the declared-size cap applies here too.
func streamTo(raw []byte, e *Entry, method uint16, w io.Writer, p *Progress) (uint32, uint64, error) {
	var src io.Reader
	switch method {
	case 0:
		src = bytes.NewReader(raw)
	case 8:
		fr := flate.NewReader(bytes.NewReader(raw))
		defer fr.Close()
		src = fr
	default:
		return 0, 0, unsupported(e)
	}

	limited := io.LimitReader(src, e.readLimit())
	h := crc32.NewIEEE()
	buf := make([]byte, ioChunk)
	var total uint64

	for {
		n, err := limited.Read(buf)
		if n > 0 {
			total += uint64(n)
			if total > e.USize {
				return 0, 0, fmt.Errorf(
					"%s: entry expands beyond its declared size of %s - refusing to continue",
					e.Name, fmtSize(e.USize))
			}
			h.Write(buf[:n])
			if _, werr := w.Write(buf[:n]); werr != nil {
				return 0, 0, fmt.Errorf("%s: write error: %w", e.Name, werr)
			}
			p.AddBytes(uint64(n))
		}
		if err == io.EOF {
			break
		}
		if err != nil {
			return 0, 0, fmt.Errorf("%s: read error: %w", e.Name, err)
		}
	}
	return h.Sum32(), total, nil
}

// ---------------- extraction plan ----------------

type planned struct {
	entry Entry
	path  string
}

type Plan struct {
	Files         []planned
	Dirs          []string
	SkippedUnsafe []string
	FilteredOut   int
}

func buildPlan(entries []Entry, root string, opts *Options) Plan {
	dirSet := make(map[string]struct{})
	var files []planned
	var skippedUnsafe []string
	filteredOut := 0

	for _, e := range entries {
		if !opts.Filter.Matches(e.Name) {
			filteredOut++
			continue
		}
		rel, err := sanitize(e.Name)
		if err != nil {
			skippedUnsafe = append(skippedUnsafe, fmt.Sprintf("%s (%v)", e.Name, err))
			continue
		}
		// -e / --flat: drop folder structure, keep the file name only
		if opts.Flatten {
			rel = filepath.Base(rel)
		}
		full := filepath.Join(root, rel)
		if e.IsDir {
			dirSet[full] = struct{}{}
		} else {
			dirSet[filepath.Dir(full)] = struct{}{}
			files = append(files, planned{entry: e, path: full})
		}
	}

	// Duplicate names (Windows is case-insensitive): keep the LAST copy so two
	// workers never write the same file at once.
	seen := make(map[string]struct{}, len(files))
	kept := make([]planned, 0, len(files))
	for i := len(files) - 1; i >= 0; i-- {
		key := strings.ToLower(files[i].path)
		if _, dup := seen[key]; dup {
			continue
		}
		seen[key] = struct{}{}
		kept = append(kept, files[i])
	}
	for i, j := 0, len(kept)-1; i < j; i, j = i+1, j-1 {
		kept[i], kept[j] = kept[j], kept[i]
	}

	dirs := make([]string, 0, len(dirSet))
	for d := range dirSet {
		dirs = append(dirs, d)
	}
	return Plan{Files: kept, Dirs: dirs, SkippedUnsafe: skippedUnsafe, FilteredOut: filteredOut}
}

// ---------------- the 'l' command ----------------

func runList(opts *Options, data []byte) int {
	entries, err := parseZip(data)
	if err != nil {
		fmt.Fprintf(os.Stderr, "fzip: %v\n", err)
		return ExitFatal
	}

	fmt.Printf("%14s  %14s  %-9s %-10s Name\n", "Size", "Packed", "Method", "Crypto")
	fmt.Println(strings.Repeat("-", 14) + "  " + strings.Repeat("-", 14) + "  " +
		strings.Repeat("-", 9) + " " + strings.Repeat("-", 10) + " " + strings.Repeat("-", 30))

	var totalU, totalC, n uint64
	for i := range entries {
		e := &entries[i]
		if e.IsDir || !opts.Filter.Matches(e.Name) {
			continue
		}
		n++
		totalU += e.USize
		totalC += e.CSize
		fmt.Printf("%14d  %14d  %-9s %-10s %s\n",
			e.USize, e.CSize, e.MethodName(), e.EncName(), e.Name)
	}
	fmt.Println(strings.Repeat("-", 14) + "  " + strings.Repeat("-", 14) + "  " +
		strings.Repeat("-", 9) + " " + strings.Repeat("-", 10) + " " + strings.Repeat("-", 30))
	ratio := 0.0
	if totalU > 0 {
		ratio = float64(totalC) / float64(totalU) * 100
	}
	fmt.Printf("%14s  %14s  %d files, %.1f%% of original\n",
		fmtSize(totalU), fmtSize(totalC), n, ratio)
	return ExitOK
}

// ---------------- the 'x' and 't' commands ----------------

func runExtract(opts *Options, data []byte, mode Mode) int {
	t0 := time.Now()
	testing := mode == ModeTest

	entries, err := parseZip(data)
	if err != nil {
		fmt.Fprintf(os.Stderr, "fzip: %v\n", err)
		return ExitFatal
	}

	// Ask for the password up front when any entry needs one.
	password, hasPassword := opts.Password, opts.HasPassword
	if !hasPassword {
		needs := false
		for i := range entries {
			if !entries[i].IsDir && entries[i].Encrypted() {
				needs = true
				break
			}
		}
		if needs {
			pw, ok := readPassword("Password: ")
			if !ok {
				fmt.Fprintln(os.Stderr, "fzip: archive is encrypted - use -p <password>")
				return ExitFatal
			}
			password, hasPassword = pw, true
		}
	}

	root := ""
	if !testing {
		want := opts.ResolveOutDir()
		r, err := prepareRoot(want)
		if err != nil {
			fmt.Fprintf(os.Stderr, "fzip: cannot create output folder %s: %v\n", want, err)
			return ExitFatal
		}
		root = r
	}

	plan := buildPlan(entries, root, opts)

	if !testing {
		for _, d := range plan.Dirs {
			os.MkdirAll(d, 0o777)
		}
	}

	files := plan.Files
	// Largest first, so all workers finish at roughly the same time.
	sort.SliceStable(files, func(i, j int) bool {
		return files[i].entry.USize > files[j].entry.USize
	})

	var totalBytes uint64
	for i := range files {
		totalBytes += files[i].entry.USize
	}

	// Warn on an implausible expansion ratio, the classic zip-bomb signature.
	archiveLen := uint64(len(data))
	if archiveLen > 0 && totalBytes/archiveLen > 500 && totalBytes > 1<<30 {
		fmt.Fprintf(os.Stderr, "fzip: warning: archive expands %dx to %s - possible zip bomb\n",
			totalBytes/archiveLen, fmtSize(totalBytes))
	}

	showBar := progressEnabled(opts.ForceProgress, opts.Quiet, opts.Verbose)
	progress := NewProgress(totalBytes, uint64(len(files)), showBar)

	if !opts.Quiet {
		if testing {
			fmt.Printf("Testing %s (%d files, %s)...\n",
				displayPath(opts.Archive), len(files), fmtSize(totalBytes))
		} else {
			fmt.Printf("Extracting %s (%d files, %s) -> %s\n",
				displayPath(opts.Archive), len(files), fmtSize(totalBytes), displayPath(root))
		}
	}
	progress.Start(t0)

	workers := opts.Workers
	if workers <= 0 {
		workers = runtime.NumCPU()
	}
	if workers > len(files) {
		workers = len(files)
	}
	if workers < 1 {
		workers = 1
	}

	var (
		mu           sync.Mutex
		errs         []string
		skippedExist atomic.Uint64
		wg           sync.WaitGroup
	)
	jobs := make(chan int)
	for w := 0; w < workers; w++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for idx := range jobs {
				f := &files[idx]
				if err := extractOne(data, &f.entry, f.path, opts,
					password, hasPassword, progress, testing, &skippedExist); err != nil {
					mu.Lock()
					errs = append(errs, err.Error())
					mu.Unlock()
				}
			}
		}()
	}
	for i := range files {
		jobs <- i
	}
	close(jobs)
	wg.Wait()

	progress.Stop()

	return report(opts, t0, progress, errs, plan.SkippedUnsafe, plan.FilteredOut,
		skippedExist.Load(), len(files), archiveLen, testing)
}

func extractOne(data []byte, e *Entry, path string, opts *Options,
	password string, hasPassword bool, progress *Progress, testing bool,
	skippedExist *atomic.Uint64) error {

	finalPath := path
	if !testing {
		p, ok := resolveOverwrite(path, e, opts)
		if !ok {
			skippedExist.Add(1)
			progress.AddBytes(e.USize)
			progress.AddFile()
			return nil
		}
		finalPath = p
	}

	start, err := dataStart(data, e)
	if err != nil {
		return err
	}
	raw := data[start : start+int(e.CSize)]
	method := e.RealMethod()

	// AE-2 stores no CRC; the field is always 0.
	skipCRC := e.Aes != nil && e.Aes.VendorVersion == 2
	wantCRC := opts.CheckCRC && e.USize > 0 && !skipCRC

	var (
		inMemory []byte
		streamed bool
		gotCRC   uint32
		gotSize  uint64
	)

	switch {
	case e.Encrypted():
		// Encrypted entries must be buffered: the HMAC covers the whole payload.
		if e.USize > opts.MaxMemory {
			return fmt.Errorf(
				"%s: encrypted entry is %s - exceeds memory limit %s (raise with --max-memory)",
				e.Name, fmtSize(e.USize), fmtSize(opts.MaxMemory))
		}
		compressed, err := decryptEntry(raw, e, password, hasPassword)
		if err != nil {
			return err
		}
		if method == 0 {
			inMemory = compressed
		} else if inMemory, err = inflateAll(compressed, e, method); err != nil {
			return err
		}

	case e.USize >= streamThreshold:
		// Large entries stream to disk, so RAM stays flat.
		streamed = true
		if testing {
			gotCRC, gotSize, err = streamTo(raw, e, method, io.Discard, progress)
			if err != nil {
				return err
			}
		} else {
			f, err := createFile(finalPath, e)
			if err != nil {
				return err
			}
			gotCRC, gotSize, err = streamTo(raw, e, method, f, progress)
			if err != nil {
				f.Close()
				return err
			}
			if err := f.Close(); err != nil {
				return fmt.Errorf("%s: write error: %w", e.Name, err)
			}
			setMtime(finalPath, e)
		}

	default:
		// Small entries decompress in one shot, which is fastest.
		if inMemory, err = inflateAll(raw, e, method); err != nil {
			return err
		}
	}

	if streamed {
		if wantCRC && gotCRC != e.CRC {
			return crcError(e, gotCRC)
		}
		if gotSize != e.USize && e.USize > 0 {
			return fmt.Errorf("%s: size mismatch (expected %d, got %d)", e.Name, e.USize, gotSize)
		}
	} else {
		if wantCRC {
			if got := crc32.ChecksumIEEE(inMemory); got != e.CRC {
				return crcError(e, got)
			}
		}
		if testing {
			progress.AddBytes(uint64(len(inMemory)))
		} else {
			f, err := createFile(finalPath, e)
			if err != nil {
				return err
			}
			for off := 0; off < len(inMemory); off += ioChunk {
				end := off + ioChunk
				if end > len(inMemory) {
					end = len(inMemory)
				}
				if _, err := f.Write(inMemory[off:end]); err != nil {
					f.Close()
					return fmt.Errorf("%s: write error: %w", e.Name, err)
				}
				progress.AddBytes(uint64(end - off))
			}
			if err := f.Close(); err != nil {
				return fmt.Errorf("%s: write error: %w", e.Name, err)
			}
			setMtime(finalPath, e)
		}
	}

	progress.AddFile()
	if opts.Verbose {
		fmt.Printf("  %s\n", e.Name)
	}
	return nil
}

func crcError(e *Entry, got uint32) error {
	return fmt.Errorf("%s: CRC mismatch - file is corrupt (expected %08x, got %08x)",
		e.Name, e.CRC, got)
}

func createFile(path string, e *Entry) (*os.File, error) {
	f, err := os.Create(path)
	if err == nil {
		return f, nil
	}

	// The target is a directory: an archive can name the same path as both a
	// folder and a file. Report that plainly rather than destroying the folder
	// and everything under it.
	//
	// This is checked before the error is classified, because the errno differs
	// by platform and Go version — Windows reports "is a directory" here rather
	// than a permission error, which an earlier version of this function
	// assumed and so never produced the message below.
	if st, serr := os.Stat(path); serr == nil && st.IsDir() {
		return nil, fmt.Errorf("%s: a folder of that name already exists here", e.Name)
	}

	if errors.Is(err, fs.ErrPermission) {
		// The target is read-only, which is what re-installing over a previous
		// version looks like. Clear the flag and retry once; --overwrite all
		// means overwrite.
		if os.Chmod(path, 0o666) == nil {
			if f, rerr := os.Create(path); rerr == nil {
				return f, nil
			}
		}
		return nil, fmt.Errorf(
			"%s: cannot create file: %w - the file may be open in another program",
			e.Name, err)
	}
	return nil, fmt.Errorf("%s: cannot create file: %w", e.Name, err)
}

func setMtime(path string, e *Entry) {
	if ts, ok := dosToUnix(e.DosDate, e.DosTime); ok {
		t := time.Unix(ts, 0)
		os.Chtimes(path, t, t)
	}
}

// resolveOverwrite reports where to write, or ok=false when the entry should be
// skipped.
func resolveOverwrite(path string, e *Entry, opts *Options) (string, bool) {
	if _, err := os.Lstat(path); err != nil {
		return path, true
	}
	switch opts.Overwrite {
	case OverwriteAlways:
		return path, true
	case OverwriteSkip:
		return "", false
	case OverwriteNewer:
		entryTS := int64(0)
		if ts, ok := dosToUnix(e.DosDate, e.DosTime); ok {
			entryTS = ts
		}
		diskTS := int64(0)
		if st, err := os.Stat(path); err == nil {
			diskTS = st.ModTime().Unix()
		}
		if entryTS > diskTS {
			return path, true
		}
		return "", false
	case OverwriteRename:
		ext := filepath.Ext(path)
		stem := strings.TrimSuffix(path, ext)
		// Claim the name atomically with O_EXCL. A plain existence test would
		// let two workers pick the same candidate and have one silently
		// overwrite the other.
		for i := 1; i < 10000; i++ {
			cand := fmt.Sprintf("%s_%d%s", stem, i, ext)
			f, err := os.OpenFile(cand, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o666)
			if err == nil {
				f.Close()
				return cand, true
			}
			if !errors.Is(err, fs.ErrExist) {
				return "", false
			}
		}
		return "", false
	}
	return path, true
}

// ---------------- final report ----------------

func report(opts *Options, t0 time.Time, progress *Progress, errs, skippedUnsafe []string,
	filteredOut int, skippedExist uint64, totalPlanned int, archiveLen uint64, testing bool) int {

	for _, e := range errs {
		fmt.Fprintf(os.Stderr, "fzip: ERROR: %s\n", e)
	}
	for _, s := range skippedUnsafe {
		fmt.Fprintf(os.Stderr, "fzip: SKIPPED (unsafe): %s\n", s)
	}

	if opts.Quiet {
		if len(errs) == 0 {
			return ExitOK
		}
		return ExitFatal
	}

	secs := time.Since(t0).Seconds()
	done := progress.BytesDone()
	ok := totalPlanned - len(errs) - int(skippedExist)
	var speed uint64
	if secs > 0 {
		speed = uint64(float64(done) / secs)
	}

	if testing {
		if len(errs) == 0 {
			fmt.Printf("OK: %d files verified, %s in %.3fs (%s/s)\n",
				ok, fmtSize(done), secs, fmtSize(speed))
		} else {
			fmt.Printf("FAILED: %d of %d files are damaged\n", len(errs), totalPlanned)
		}
	} else {
		fmt.Printf("Done: %d files, %s -> %s in %.3fs (%s/s)\n",
			ok, fmtSize(archiveLen), fmtSize(done), secs, fmtSize(speed))
	}

	if skippedExist > 0 {
		fmt.Printf("  %d files skipped (already exist)\n", skippedExist)
	}
	if filteredOut > 0 {
		fmt.Printf("  %d entries excluded by filters\n", filteredOut)
	}
	if len(skippedUnsafe) > 0 {
		fmt.Printf("  %d entries skipped as unsafe\n", len(skippedUnsafe))
	}
	if len(errs) > 0 {
		fmt.Printf("  %d errors (see above)\n", len(errs))
	}

	switch {
	case len(errs) > 0:
		return ExitFatal
	case len(skippedUnsafe) > 0 || skippedExist > 0:
		return ExitWarning
	default:
		return ExitOK
	}
}
