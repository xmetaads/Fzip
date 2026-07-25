// Fzip - fast portable zip tool for Windows.
//
// Copyright (c) 2026 Tcoder LLC. MIT licensed.
//
// Portable in the same sense as Microsoft's azcopy.exe: a single self-contained
// .exe with nothing to install, driven entirely from cmd.exe or PowerShell.
//
// Speed comes from three things:
//  1. Memory-mapping the whole archive, so entries are read without a syscall each
//  2. Decompressing entries in parallel, one goroutine per core
//  3. Streaming anything large, so memory stays flat regardless of archive size
//
// Reads zip. Writes zip, optionally encrypted with AES-256.

package main

import (
	"bytes"
	"fmt"
	"os"
	"strings"
)

// pauseIfOwnConsole keeps the window open when launched by double-click, the
// way azcopy.exe does.
func pauseIfOwnConsole(noPause bool) {
	if !shouldPause(noPause) {
		return
	}
	fmt.Print("\nPress Enter to exit...")
	var b [1]byte
	for {
		n, err := os.Stdin.Read(b[:])
		if err != nil || n == 0 || b[0] == '\n' {
			return
		}
	}
}

// describeForeignFormat names an archive Fzip no longer handles, so someone
// arriving from an older release gets an explanation rather than "not a ZIP".
func describeForeignFormat(head []byte) string {
	switch {
	case bytes.HasPrefix(head, []byte("Rar!\x1A\x07")):
		return "a RAR archive"
	case bytes.HasPrefix(head, []byte{'7', 'z', 0xBC, 0xAF, 0x27, 0x1C}):
		return "a 7z archive"
	case bytes.HasPrefix(head, []byte{0x1F, 0x8B}):
		return "a gzip file"
	case bytes.HasPrefix(head, []byte("BZh")):
		return "a bzip2 file"
	case bytes.HasPrefix(head, []byte{0xFD, '7', 'z', 'X', 'Z', 0x00}):
		return "an xz file"
	case bytes.HasPrefix(head, []byte{0x28, 0xB5, 0x2F, 0xFD}):
		return "a zstd file"
	case len(head) > 262 && bytes.Equal(head[257:262], []byte("ustar")):
		return "a tar archive"
	}
	return ""
}

func main() {
	initConsole()

	opts, err := parseArgs(os.Args[1:])
	if err != nil {
		code := ExitOK
		if msg := err.Error(); msg != "" {
			fmt.Fprintf(os.Stderr, "fzip: %s\n\n", msg)
			code = ExitUsage
		}
		printHelp()
		// Argument parsing failed, so no --no-pause was captured; the
		// environment variable is still honoured inside shouldPause.
		pauseIfOwnConsole(false)
		os.Exit(code)
	}

	code := run(opts)
	pauseIfOwnConsole(opts.NoPause)
	os.Exit(code)
}

func run(opts *Options) int {
	// Creating an archive needs no format detection.
	if opts.Mode == ModeAdd {
		if !strings.EqualFold(fileExt(opts.Archive), ".zip") && !opts.Quiet {
			fmt.Fprintf(os.Stderr,
				"fzip: warning: %s does not end in .zip, but a zip is what will be written\n",
				opts.Archive)
		}
		return runAdd(opts)
	}

	f, err := os.Open(opts.Archive)
	if err != nil {
		fmt.Fprintf(os.Stderr, "fzip: cannot open %s: %v\n", opts.Archive, err)
		return ExitFatal
	}
	defer f.Close()

	data, unmap, err := mmapReadOnly(f)
	if err != nil {
		fmt.Fprintf(os.Stderr, "fzip: cannot read %s: %v\n", opts.Archive, err)
		return ExitFatal
	}
	defer unmap()

	// Identify by magic bytes, never by extension, so a misnamed archive is
	// still handled correctly and a genuinely foreign one is named.
	if !bytes.HasPrefix(data, []byte("PK")) {
		if what := describeForeignFormat(data); what != "" {
			fmt.Fprintf(os.Stderr,
				"fzip: %s is %s, and this version reads zip only\n",
				opts.Archive, what)
			return ExitFatal
		}
		fmt.Fprintf(os.Stderr, "fzip: %s is not a valid zip file\n", opts.Archive)
		return ExitFatal
	}

	if opts.Mode == ModeList {
		return runList(opts, data)
	}
	return runExtract(opts, data, opts.Mode)
}

func fileExt(p string) string {
	if i := strings.LastIndexByte(p, '.'); i >= 0 {
		return p[i:]
	}
	return ""
}
