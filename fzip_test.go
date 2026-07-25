package main

import (
	"bytes"
	"os"
	"path/filepath"
	"testing"
)

// ---------------- crypto ----------------

func TestAesRoundTrip(t *testing.T) {
	data := []byte("some data that really must stay confidential")
	enc, err := aesEncrypt(data, "Passw0rd!2026")
	if err != nil {
		t.Fatal(err)
	}
	dec, err := aesDecrypt(enc, 3, "Passw0rd!2026")
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(dec, data) {
		t.Fatalf("round trip changed the data")
	}
}

func TestAesRejectsWrongPassword(t *testing.T) {
	enc, err := aesEncrypt([]byte("abc"), "right")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := aesDecrypt(enc, 3, "wrong"); err != errWrongPassword {
		t.Fatalf("want errWrongPassword, got %v", err)
	}
}

func TestAesDetectsTampering(t *testing.T) {
	enc, err := aesEncrypt([]byte("important payload"), "pw")
	if err != nil {
		t.Fatal(err)
	}
	enc[len(enc)-15] ^= 0xFF // flip one byte of ciphertext
	if _, err := aesDecrypt(enc, 3, "pw"); err != errTampered {
		t.Fatalf("want errTampered, got %v", err)
	}
}

func TestAesSaltIsRandomEachTime(t *testing.T) {
	a, _ := aesEncrypt([]byte("x"), "pw")
	b, _ := aesEncrypt([]byte("x"), "pw")
	if bytes.Equal(a, b) {
		t.Fatal("salt must be random on every encryption")
	}
}

// The streaming writer must produce a byte stream the one-shot decryptor
// accepts, including across chunk boundaries that straddle AES blocks.
func TestStreamingMatchesOneShot(t *testing.T) {
	data := make([]byte, 5000)
	for i := range data {
		data[i] = byte(i % 251)
	}
	var sink bytes.Buffer
	w, err := newAesWriter(&sink, "pw")
	if err != nil {
		t.Fatal(err)
	}
	at := 0
	for _, n := range []int{1, 15, 16, 17, 4951} {
		if _, err := w.Write(data[at : at+n]); err != nil {
			t.Fatal(err)
		}
		at += n
	}
	if at != len(data) {
		t.Fatalf("test wrote %d of %d bytes", at, len(data))
	}
	if _, err := w.Finish(); err != nil {
		t.Fatal(err)
	}
	back, err := aesDecrypt(sink.Bytes(), 3, "pw")
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(back, data) {
		t.Fatal("streamed output did not decrypt to the input")
	}
}

// ---------------- timestamps ----------------

func TestDosRoundTripKeepsTheMoment(t *testing.T) {
	// DOS stores seconds in 2-second units, so only even seconds survive.
	for _, unix := range []int64{
		315532800,  // 1980-01-01
		1751371200, // 2025-07-01, inside DST for northern zones
		1735689600, // 2025-01-01, outside DST
		2000000000,
	} {
		d, tm := unixToDos(unix)
		back, ok := dosToUnix(d, tm)
		if !ok {
			t.Fatalf("packed value must decode, for %d", unix)
		}
		if back != unix {
			t.Fatalf("round trip drifted for %d: got %d", unix, back)
		}
	}
}

func TestDosRejectsNonsenseFields(t *testing.T) {
	// hour 31 / minute 63 / second 62 come straight from 0xFFFF
	if _, ok := dosToUnix(0x21, 0xFFFF); ok {
		t.Fatal("must reject hour 31")
	}
	if _, ok := dosToUnix(0, 0); ok {
		t.Fatal("must reject day 0")
	}
}

func TestPre1980ClampsToStartOfRange(t *testing.T) {
	d, tm := packDos(1975, 6, 15, 12, 30, 0)
	if d != (1<<5)|1 || tm != 0 {
		t.Fatalf("must clamp to 1980-01-01, got date=%d time=%d", d, tm)
	}
}

// ---------------- safe paths ----------------

func TestBlocksZipSlip(t *testing.T) {
	for _, name := range []string{"../evil.txt", "a/../../b", `..\evil`} {
		if _, err := sanitize(name); err != errTraversal {
			t.Fatalf("%s: want errTraversal, got %v", name, err)
		}
	}
}

func TestStripsAbsolutePaths(t *testing.T) {
	got, err := sanitize("C:/Windows/x.txt")
	if err != nil || got != filepath.Join("Windows", "x.txt") {
		t.Fatalf("got %q, %v", got, err)
	}
	got, err = sanitize("/etc/passwd")
	if err != nil || got != filepath.Join("etc", "passwd") {
		t.Fatalf("got %q, %v", got, err)
	}
}

func TestRenamesDeviceNames(t *testing.T) {
	cases := map[string]string{
		"CON": "_CON", "con.txt": "_con.txt", "LPT1.dat": "_LPT1.dat", "NUL": "_NUL",
		// Ordinary names that merely start the same are left alone
		"console.log": "console.log",
	}
	for in, want := range cases {
		got, err := sanitize(in)
		if err != nil || got != want {
			t.Fatalf("%s: got %q (%v), want %q", in, got, err, want)
		}
	}
}

func TestCleansForbiddenCharacters(t *testing.T) {
	if got, _ := sanitize("a<b>c.txt"); got != "a_b_c.txt" {
		t.Fatalf("got %q", got)
	}
	if got, _ := sanitize("name."); got != "name_" {
		t.Fatalf("got %q", got)
	}
	if _, err := sanitize("   "); err != errEmptyName {
		t.Fatalf("want errEmptyName, got %v", err)
	}
}

func TestKeepsUnicode(t *testing.T) {
	got, err := sanitize("文档/café.txt")
	if err != nil || got != filepath.Join("文档", "café.txt") {
		t.Fatalf("got %q, %v", got, err)
	}
}

// ---------------- globs ----------------

func TestGlobMatching(t *testing.T) {
	cases := []struct {
		pattern, name string
		want          bool
	}{
		{"*.txt", "a.txt", true},
		{"*.txt", "a.bin", false},
		{"*.TXT", "a.txt", true}, // Windows filesystems are case-insensitive
		{"a?c", "abc", true},
		{"a?c", "abbc", false},
		{"**/x.txt", "deep/nested/x.txt", true},
		{"src/*", "src/a.txt", true},
		{"src/*", "src/sub/a.txt", false},
		{"**", "anything/at/all", true},
	}
	for _, c := range cases {
		if got := compileGlob(c.pattern).match(c.name); got != c.want {
			t.Errorf("%q vs %q: got %v want %v", c.pattern, c.name, got, c.want)
		}
	}
}

func TestFilterMatchesAtAnyDepth(t *testing.T) {
	f := Filter{exclude: buildPatterns([]string{"*.tmp"})}
	if f.Matches("deep/nested/scratch.tmp") {
		t.Fatal("a bare pattern must exclude at any depth")
	}
	if !f.Matches("deep/nested/keep.txt") {
		t.Fatal("unrelated names must survive")
	}
}

// ---------------- sizes ----------------

func TestParseSize(t *testing.T) {
	cases := map[string]uint64{"512": 512, "64M": 64 << 20, "2G": 2 << 30, "1k": 1024}
	for in, want := range cases {
		got, ok := parseSize(in)
		if !ok || got != want {
			t.Errorf("%s: got %d (%v) want %d", in, got, ok, want)
		}
	}
	// Overflow must be refused rather than silently wrapping to a tiny cap.
	if _, ok := parseSize("99999999999999999999G"); ok {
		t.Error("overflow must be rejected")
	}
	if _, ok := parseSize("abc"); ok {
		t.Error("nonsense must be rejected")
	}
}

// ---------------- end to end, over a real mapping ----------------

// This exercises mmapReadOnly against a real file. Run under -race, which turns
// on checkptr, it is also the check that the mapped slice is formed correctly.
func TestZipRoundTripThroughMapping(t *testing.T) {
	dir := t.TempDir()
	srcDir := filepath.Join(dir, "src")
	if err := os.MkdirAll(filepath.Join(srcDir, "sub"), 0o777); err != nil {
		t.Fatal(err)
	}
	payload := bytes.Repeat([]byte("fzip round trip payload\n"), 400)
	if err := os.WriteFile(filepath.Join(srcDir, "a.txt"), payload, 0o666); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(srcDir, "sub", "b.bin"), payload, 0o666); err != nil {
		t.Fatal(err)
	}

	archive := filepath.Join(dir, "out.zip")
	opts := &Options{
		Mode: ModeAdd, Archive: archive, Inputs: []string{srcDir},
		Level: 5, MaxMemory: 1 << 30, AssumeYes: true, Quiet: true, CheckCRC: true,
	}
	if code := runAdd(opts); code != ExitOK {
		t.Fatalf("runAdd returned %d", code)
	}

	f, err := os.Open(archive)
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	data, unmap, err := mmapReadOnly(f)
	if err != nil {
		t.Fatal(err)
	}
	defer unmap()

	entries, err := parseZip(data)
	if err != nil {
		t.Fatal(err)
	}
	files := 0
	for i := range entries {
		if !entries[i].IsDir {
			files++
		}
	}
	if files != 2 {
		t.Fatalf("want 2 files in the archive, got %d", files)
	}

	out := filepath.Join(dir, "out")
	xopts := &Options{
		Mode: ModeExtract, Archive: archive, OutDir: out,
		Level: 5, MaxMemory: 1 << 30, Quiet: true, CheckCRC: true,
	}
	if code := runExtract(xopts, data, ModeExtract); code != ExitOK {
		t.Fatalf("runExtract returned %d", code)
	}
	got, err := os.ReadFile(filepath.Join(out, "src", "sub", "b.bin"))
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(got, payload) {
		t.Fatal("extracted content does not match the original")
	}
}

// A hostile entry must produce a clean error, never a panic from an
// out-of-range index.
func TestHostileOffsetsAreRefused(t *testing.T) {
	data := make([]byte, 200)
	if _, err := parseZip(data); err == nil {
		t.Fatal("a file with no end-of-archive record must be refused")
	}

	e := &Entry{Name: "x", LocalOff: 0xFFFFFFFFFFFFFFF0, CSize: 10}
	if _, err := dataStart(make([]byte, 1000), e); err == nil {
		t.Fatal("an offset near the top of the range must be refused")
	}
	e = &Entry{Name: "x", LocalOff: 0, CSize: 0xFFFFFFFFFFFFFFF0}
	if _, err := dataStart(make([]byte, 1000), e); err == nil {
		t.Fatal("a size near the top of the range must be refused")
	}
}
