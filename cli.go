// Command line parsing, name filters and help output.

package main

import (
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
)

const (
	Version = "2.0.0"
	// Product name as published. The executable stays lowercase `fzip`.
	Product = "Fzip"
	Vendor  = "Tcoder LLC"
)

// Upper bound for -t. Far above any real core count, low enough that a typo
// cannot exhaust the machine's thread budget.
const maxWorkers = 1024

type Mode int

const (
	ModeExtract Mode = iota
	ModeList
	ModeTest
	ModeAdd
)

type Overwrite int

const (
	OverwriteAlways Overwrite = iota // overwrite everything (default)
	OverwriteSkip                    // keep existing files
	OverwriteRename                  // write as name_1, name_2, ...
	OverwriteNewer                   // overwrite only when the archived copy is newer
)

// ---------------- glob matching ----------------

// wildMatch matches one path segment against a pattern containing * and ?.
// Comparison is case-insensitive because Windows filesystems are.
func wildMatch(pat, s string) bool {
	ps := []rune(strings.ToLower(pat))
	ss := []rune(strings.ToLower(s))
	var p, i int
	star, mark := -1, 0
	for i < len(ss) {
		switch {
		case p < len(ps) && (ps[p] == '?' || ps[p] == ss[i]):
			p++
			i++
		case p < len(ps) && ps[p] == '*':
			star = p
			mark = i
			p++
		case star >= 0:
			p = star + 1
			mark++
			i = mark
		default:
			return false
		}
	}
	for p < len(ps) && ps[p] == '*' {
		p++
	}
	return p == len(ps)
}

// globPattern is a slash-separated pattern where `**` matches any number of
// segments and `*` / `?` match within a single segment.
type globPattern struct{ segs []string }

func compileGlob(pat string) globPattern {
	return globPattern{segs: strings.Split(strings.ReplaceAll(pat, "\\", "/"), "/")}
}

func (g globPattern) match(name string) bool {
	return matchSegs(g.segs, strings.Split(name, "/"))
}

func matchSegs(pat, name []string) bool {
	if len(pat) == 0 {
		return len(name) == 0
	}
	if pat[0] == "**" {
		for i := 0; i <= len(name); i++ {
			if matchSegs(pat[1:], name[i:]) {
				return true
			}
		}
		return false
	}
	if len(name) == 0 {
		return false
	}
	if !wildMatch(pat[0], name[0]) {
		return false
	}
	return matchSegs(pat[1:], name[1:])
}

// Filter holds the name filters from -i / -x.
type Filter struct {
	include []globPattern
	exclude []globPattern
}

func buildPatterns(raw []string) []globPattern {
	var out []globPattern
	for _, p := range raw {
		out = append(out, compileGlob(p))
		// A pattern without a separator should also match at any depth.
		if !strings.ContainsAny(p, `/\`) && !strings.HasPrefix(p, "**") {
			out = append(out, compileGlob("**/"+p))
		}
	}
	return out
}

// Matches reports whether an archive entry name is selected.
func (f *Filter) Matches(name string) bool {
	norm := strings.TrimRight(strings.ReplaceAll(name, "\\", "/"), "/")
	for _, g := range f.exclude {
		if g.match(norm) {
			return false
		}
	}
	if len(f.include) == 0 {
		return true
	}
	for _, g := range f.include {
		if g.match(norm) {
			return true
		}
	}
	return false
}

// ---------------- options ----------------

type Options struct {
	Mode          Mode
	Archive       string
	Inputs        []string
	OutDir        string
	Password      string
	HasPassword   bool
	Workers       int
	CheckCRC      bool
	Quiet         bool
	Verbose       bool
	ForceProgress bool
	Overwrite     Overwrite
	Filter        Filter
	Flatten       bool
	Level         int
	MaxMemory     uint64
	AssumeYes     bool
	// NoPause never waits for a keypress at the end. For installers and scripts
	// that run fzip with no window and no keyboard to answer with.
	NoPause bool
}

// ResolveOutDir gives the destination folder: -o when supplied, otherwise a
// folder named after the archive, created NEXT TO THE ARCHIVE — not in the
// working directory. That is what 7-Zip and WinRAR do, and it is what makes
// drag-and-drop onto fzip.exe behave sensibly, since the working directory then
// belongs to Explorer rather than to the archive.
func (o *Options) ResolveOutDir() string {
	if o.OutDir != "" {
		return o.OutDir
	}
	stem := filepath.Base(o.Archive)
	if strings.EqualFold(filepath.Ext(stem), ".zip") {
		stem = stem[:len(stem)-4]
	}
	if stem == "" || stem == "." || stem == string(filepath.Separator) {
		stem = "fzip_out"
	}
	if dir := filepath.Dir(o.Archive); dir != "" && dir != "." {
		return filepath.Join(dir, stem)
	}
	return stem
}

// parseSize turns "512", "64M" or "2G" into a byte count. Reports ok=false on
// overflow rather than silently wrapping to a tiny cap.
func parseSize(s string) (uint64, bool) {
	s = strings.TrimSpace(s)
	if s == "" {
		return 0, false
	}
	mul := uint64(1)
	switch s[len(s)-1] {
	case 'K', 'k':
		mul, s = 1024, s[:len(s)-1]
	case 'M', 'm':
		mul, s = 1024*1024, s[:len(s)-1]
	case 'G', 'g':
		mul, s = 1024*1024*1024, s[:len(s)-1]
	}
	n, err := strconv.ParseUint(strings.TrimSpace(s), 10, 64)
	if err != nil {
		return 0, false
	}
	if n != 0 && n > ^uint64(0)/mul {
		return 0, false
	}
	return n * mul, true
}

// usageError carries an argument problem. An empty message means "just print
// help", which is what a bare `fzip` does.
type usageError struct{ msg string }

func (e *usageError) Error() string { return e.msg }

func parseArgs(args []string) (*Options, error) {
	if len(args) == 0 {
		return nil, &usageError{}
	}

	o := &Options{
		Workers:   0,
		CheckCRC:  true,
		Overwrite: OverwriteAlways,
		Level:     5,
		MaxMemory: 1 << 30,
	}
	var positional, include, exclude []string
	modeSet := false
	promptPassword := false

	next := func(i *int, what string) (string, error) {
		*i++
		if *i >= len(args) {
			return "", &usageError{fmt.Sprintf("missing value for %s", what)}
		}
		return args[*i], nil
	}

	for i := 0; i < len(args); i++ {
		a := args[i]
		var err error

		switch a {
		case "-h", "--help", "/?":
			return nil, &usageError{}
		case "-V", "--version":
			fmt.Printf("%s %s\n", Product, Version)
			fmt.Printf("Copyright (c) 2026 %s. MIT licensed.\n", Vendor)
			fmt.Println("Fast portable zip tool for Windows")
			fmt.Println("Reads zip. Writes zip.")
			os.Exit(ExitOK)
		case "-o", "--output":
			if o.OutDir, err = next(&i, "-o"); err != nil {
				return nil, err
			}
		case "-p", "--password":
			// A bare -p (or -p followed by another option) prompts instead,
			// which keeps the password out of the shell history.
			if i+1 < len(args) && !strings.HasPrefix(args[i+1], "-") {
				i++
				o.Password, o.HasPassword = args[i], true
			} else {
				promptPassword = true
			}
		case "-t", "--threads":
			v, err := next(&i, "-t")
			if err != nil {
				return nil, err
			}
			n, cerr := strconv.Atoi(v)
			if cerr != nil || n < 0 {
				return nil, &usageError{fmt.Sprintf("invalid thread count: %s", v)}
			}
			if n > maxWorkers {
				return nil, &usageError{fmt.Sprintf(
					"thread count %d is too high (maximum %d)", n, maxWorkers)}
			}
			o.Workers = n
		case "-i", "--include":
			v, err := next(&i, "-i")
			if err != nil {
				return nil, err
			}
			include = append(include, v)
		case "-x", "--exclude":
			v, err := next(&i, "-x")
			if err != nil {
				return nil, err
			}
			exclude = append(exclude, v)
		case "-y", "--yes":
			o.AssumeYes = true
		case "-e", "--flat":
			o.Flatten = true
		case "-q", "--quiet":
			o.Quiet = true
		case "-v", "--verbose":
			o.Verbose = true
		case "--no-crc":
			o.CheckCRC = false
		case "--progress":
			o.ForceProgress = true
		case "--no-pause":
			o.NoPause = true
		case "--max-memory":
			v, err := next(&i, "--max-memory")
			if err != nil {
				return nil, err
			}
			n, ok := parseSize(v)
			if !ok {
				return nil, &usageError{fmt.Sprintf("invalid size: %s", v)}
			}
			o.MaxMemory = n
		case "--overwrite", "-ao":
			v, err := next(&i, "--overwrite")
			if err != nil {
				return nil, err
			}
			switch strings.ToLower(v) {
			case "all", "a", "always":
				o.Overwrite = OverwriteAlways
			case "skip", "s", "never":
				o.Overwrite = OverwriteSkip
			case "rename", "r":
				o.Overwrite = OverwriteRename
			case "newer", "u":
				o.Overwrite = OverwriteNewer
			default:
				return nil, &usageError{fmt.Sprintf(
					"unknown overwrite mode '%s' (use all, skip, rename or newer)", v)}
			}
		case "--level":
			v, err := next(&i, "--level")
			if err != nil {
				return nil, err
			}
			o.Level = clampLevel(v)
		default:
			switch {
			case strings.HasPrefix(a, "-mx"):
				o.Level = clampLevel(strings.TrimPrefix(a[3:], "="))
			case strings.HasPrefix(a, "-p") && len(a) > 2:
				// 7-Zip style: -pSECRET attached to the flag
				o.Password, o.HasPassword = a[2:], true
			case strings.HasPrefix(a, "-i!"):
				include = append(include, a[3:])
			case strings.HasPrefix(a, "-x!"):
				exclude = append(exclude, a[3:])
			case strings.HasPrefix(a, "-") && len(a) > 1 && modeSet:
				return nil, &usageError{fmt.Sprintf("unknown option: %s", a)}
			case !modeSet && len(positional) == 0:
				switch strings.ToLower(a) {
				case "x", "e", "extract", "extractl", "unzip":
					o.Mode, modeSet = ModeExtract, true
				case "l", "ls", "list":
					o.Mode, modeSet = ModeList, true
				case "t", "test":
					o.Mode, modeSet = ModeTest, true
				case "a", "add", "c", "create":
					o.Mode, modeSet = ModeAdd, true
				default:
					positional = append(positional, a)
				}
			default:
				positional = append(positional, a)
			}
		}
	}

	if len(positional) == 0 {
		return nil, &usageError{"no archive specified"}
	}
	o.Archive = positional[0]
	o.Inputs = positional[1:]

	if o.Mode != ModeAdd {
		if st, err := os.Stat(o.Archive); err != nil || st.IsDir() {
			return nil, &usageError{fmt.Sprintf("file not found: %s", o.Archive)}
		}
	}

	// A bare -p means "ask me now". Failing to read one must be an error:
	// silently creating an UNENCRYPTED archive would be the worst outcome.
	if promptPassword && !o.HasPassword {
		pw, ok := readPassword("Password: ")
		if !ok {
			return nil, &usageError{
				"cannot read a password here (not an interactive terminal); " +
					"pass it as -p <password>"}
		}
		o.Password, o.HasPassword = pw, true
	}

	o.Filter = Filter{include: buildPatterns(include), exclude: buildPatterns(exclude)}
	return o, nil
}

func clampLevel(s string) int {
	n, err := strconv.Atoi(strings.TrimSpace(s))
	if err != nil {
		return 5
	}
	if n < 0 {
		return 0
	}
	if n > 9 {
		return 9
	}
	return n
}

func printHelp() {
	fmt.Printf("%s %s - fast portable zip tool for Windows (no install needed)\n",
		Product, Version)
	fmt.Printf("Published by %s. MIT licensed.\n\n", Vendor)
	fmt.Println("This is a command line tool. Open cmd.exe or PowerShell and run:")
	fmt.Println()
	fmt.Println("COMMANDS:")
	fmt.Println("  fzip x <archive.zip> [options]    extract")
	fmt.Println("  fzip <archive.zip>                extract (shorthand)")
	fmt.Println("  fzip l <archive.zip>              list contents")
	fmt.Println("  fzip t <archive.zip>              test integrity, write nothing")
	fmt.Println("  fzip a <archive.zip> <files...>   create a zip")
	fmt.Println()
	fmt.Println("READS:  zip")
	fmt.Println("WRITES: zip, optionally encrypted with AES-256")
	fmt.Println()
	fmt.Println("OPTIONS:")
	rows := [][2]string{
		{"-o <dir>", "output folder (default: archive name)"},
		{"-p <pass>", "password; prompts securely if omitted"},
		{"-t <n>", "worker count (default: all cores)"},
		{"-i <glob>", "include only matching names"},
		{"-x <glob>", "exclude matching names"},
		{"-e", "flatten: ignore folder structure"},
		{"-y", "assume yes (overwrite archive when creating)"},
		{"--overwrite <m>", "all | skip | rename | newer (default: all)"},
		{"-mx<0-9>", "compression level for 'a' (0 = store, 9 = best)"},
		{"--max-memory <n>", "RAM cap, e.g. 512M or 2G (default 1G)"},
		{"--no-crc", "skip CRC verification"},
		{"--progress", "force the progress bar even when redirected"},
		{"--no-pause", "never wait for a keypress (installers, scripts)"},
		{"-q", "quiet: errors only"},
		{"-v", "verbose: list every file"},
		{"-V", "show version"},
	}
	for _, r := range rows {
		fmt.Printf("  %-18s %s\n", r[0], r[1])
	}
	fmt.Println()
	fmt.Println("EXAMPLES:")
	fmt.Println(`  fzip x data.zip -o D:\out`)
	fmt.Println("  fzip x secret.zip -p MyPass123")
	fmt.Println(`  fzip x big.zip -x "*.tmp" --overwrite skip`)
	fmt.Println("  fzip a backup.zip photos docs -mx9 -p MyPass123")
	fmt.Println("  fzip t archive.zip")
	fmt.Println()
	fmt.Println("Tip: you can also drag a .zip file onto fzip.exe.")
	fmt.Println()
	fmt.Println("EXIT CODES: 0 = ok, 1 = warning, 2 = error, 7 = bad command line")
}
