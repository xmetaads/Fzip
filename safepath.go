// Safe path handling: zip-slip defence, Windows device names, long paths.

package main

import (
	"errors"
	"os"
	"path/filepath"
	"strings"
)

// Reserved Windows device names. A file actually named CON or NUL is nearly
// impossible to delete from Explorer, so these are always renamed.
var reservedNames = map[string]bool{
	"CON": true, "PRN": true, "AUX": true, "NUL": true,
	"COM1": true, "COM2": true, "COM3": true, "COM4": true, "COM5": true,
	"COM6": true, "COM7": true, "COM8": true, "COM9": true, "COM0": true,
	"LPT1": true, "LPT2": true, "LPT3": true, "LPT4": true, "LPT5": true,
	"LPT6": true, "LPT7": true, "LPT8": true, "LPT9": true, "LPT0": true,
	"CONIN$": true, "CONOUT$": true, "CLOCK$": true,
	"COM¹": true, "COM²": true, "COM³": true,
}

// Why an archive entry was rejected.
var (
	errTraversal = errors.New("path escapes target folder")
	errEmptyName = errors.New("empty name")
)

// cleanComponent sanitises a single path component for Windows.
func cleanComponent(part string) string {
	var b strings.Builder
	for _, c := range part {
		switch {
		case c == '<' || c == '>' || c == ':' || c == '"' || c == '|' || c == '?' || c == '*':
			b.WriteByte('_')
		case c < 32:
			b.WriteByte('_')
		default:
			b.WriteRune(c)
		}
	}
	cleaned := b.String()

	// Windows silently strips trailing spaces and dots; append '_' to keep the
	// name distinct instead of letting two entries collide.
	trimmed := strings.TrimRight(cleaned, " .")
	if len(trimmed) != len(cleaned) {
		if trimmed == "" {
			cleaned = ""
		} else {
			cleaned = trimmed + "_"
		}
	}

	// Device names are reserved even with an extension, e.g. CON.txt
	stem, _, _ := strings.Cut(cleaned, ".")
	if stem != "" && reservedNames[strings.ToUpper(stem)] {
		cleaned = "_" + cleaned
	}
	return cleaned
}

// sanitize turns an archive entry name into a safe relative path, blocking `..`
// traversal, absolute paths, drive letters, forbidden characters and reserved
// device names.
func sanitize(name string) (string, error) {
	var parts []string
	for _, raw := range strings.FieldsFunc(name, func(r rune) bool {
		return r == '/' || r == '\\'
	}) {
		if raw == "." {
			continue
		}
		if raw == ".." {
			return "", errTraversal
		}
		// Drop a leading drive specifier such as "C:"
		if len(raw) == 2 && raw[1] == ':' &&
			((raw[0] >= 'a' && raw[0] <= 'z') || (raw[0] >= 'A' && raw[0] <= 'Z')) {
			continue
		}
		cleaned := cleanComponent(raw)
		if cleaned == "" {
			continue
		}
		parts = append(parts, cleaned)
	}
	if len(parts) == 0 {
		return "", errEmptyName
	}
	return filepath.Join(parts...), nil
}

// prepareRoot creates the destination folder and returns it as an absolute
// path. Go's os package converts long absolute paths to the `\\?\` form itself,
// so paths beyond the 260-character limit work without extra handling here.
func prepareRoot(dir string) (string, error) {
	if err := os.MkdirAll(dir, 0o777); err != nil {
		return "", err
	}
	abs, err := filepath.Abs(dir)
	if err != nil {
		return dir, nil
	}
	return abs, nil
}

// displayPath strips the `\\?\` prefix when showing a path to a person.
func displayPath(p string) string {
	if rest, ok := strings.CutPrefix(p, `\\?\UNC\`); ok {
		return `\\` + rest
	}
	if rest, ok := strings.CutPrefix(p, `\\?\`); ok {
		return rest
	}
	return p
}
