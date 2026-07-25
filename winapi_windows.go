// Windows console and memory-mapping support.
//
// Fzip is a Windows tool, so this file has no portable counterpart. Everything
// here goes through the syscall package rather than a third-party binding, which
// keeps the module free of external dependencies.

package main

import (
	"bufio"
	"fmt"
	"os"
	"strings"
	"syscall"
	"unsafe"
)

var (
	kernel32 = syscall.NewLazyDLL("kernel32.dll")
	user32   = syscall.NewLazyDLL("user32.dll")

	procSetConsoleOutputCP    = kernel32.NewProc("SetConsoleOutputCP")
	procGetConsoleMode        = kernel32.NewProc("GetConsoleMode")
	procSetConsoleMode        = kernel32.NewProc("SetConsoleMode")
	procGetConsoleProcessList = kernel32.NewProc("GetConsoleProcessList")
	procGetConsoleWindow      = kernel32.NewProc("GetConsoleWindow")
	procIsWindowVisible       = user32.NewProc("IsWindowVisible")
	procIsIconic              = user32.NewProc("IsIconic")
)

const (
	enableVirtualTerminalProcessing = 0x0004
	enableEchoInput                 = 0x0004
	enableLineInput                 = 0x0002
)

// initConsole switches the console to UTF-8 so non-ASCII file names render
// correctly, and turns on ANSI handling so the progress bar redraws cleanly.
func initConsole() {
	procSetConsoleOutputCP.Call(65001)
	h := syscall.Handle(os.Stdout.Fd())
	var mode uint32
	if r, _, _ := procGetConsoleMode.Call(uintptr(h), uintptr(unsafe.Pointer(&mode))); r != 0 {
		procSetConsoleMode.Call(uintptr(h), uintptr(mode|enableVirtualTerminalProcessing))
	}
}

// isTerminal reports whether f is attached to a console rather than a pipe or a
// file. GetConsoleMode succeeds only on a real console handle.
func isTerminal(f *os.File) bool {
	var mode uint32
	r, _, _ := procGetConsoleMode.Call(f.Fd(), uintptr(unsafe.Pointer(&mode)))
	return r != 0
}

// ownsConsole reports whether this process is the only one attached to its
// console, i.e. the user launched it by double-clicking from Explorer rather
// than from an existing shell.
func ownsConsole() bool {
	var list [4]uint32
	n, _, _ := procGetConsoleProcessList.Call(
		uintptr(unsafe.Pointer(&list[0])), uintptr(len(list)))
	return n == 1
}

// consoleIsVisible reports whether there is a console window a person could
// actually be looking at.
//
// An installer that launches fzip hidden still gets a console allocated, and
// that console still reports itself as a terminal — so owning it, or stdin being
// a tty, is not enough to conclude anyone is watching. The window being visible
// is. Without this check a hidden run would stop at "Press Enter to exit..." and
// wait forever, with no keyboard to answer it.
func consoleIsVisible() bool {
	hwnd, _, _ := procGetConsoleWindow.Call()
	if hwnd == 0 {
		return false
	}
	vis, _, _ := procIsWindowVisible.Call(hwnd)
	if vis != 0 {
		return true
	}
	// Minimised still counts: the user can restore the window.
	icon, _, _ := procIsIconic.Call(hwnd)
	return icon != 0
}

// shouldPause reports whether to hold the window open at the end so a
// double-click user can read the output. Every condition must hold, because
// getting this wrong means an unattended run hangs forever instead of finishing.
func shouldPause(noPauseFlag bool) bool {
	if noPauseFlag {
		return false
	}
	if _, set := os.LookupEnv("FZIP_NO_PAUSE"); set {
		return false
	}
	return ownsConsole() && consoleIsVisible() && isTerminal(os.Stdin)
}

// readPassword prompts without echoing. Returns ok=false when there is no
// console to prompt on, so callers can fail loudly rather than silently
// continuing without a password.
func readPassword(prompt string) (string, bool) {
	if !isTerminal(os.Stdin) {
		return "", false
	}
	h := syscall.Handle(os.Stdin.Fd())
	var mode uint32
	if r, _, _ := procGetConsoleMode.Call(uintptr(h), uintptr(unsafe.Pointer(&mode))); r == 0 {
		return "", false
	}
	// Keep line editing on so backspace works; only the echo goes away.
	procSetConsoleMode.Call(uintptr(h), uintptr(mode&^enableEchoInput|enableLineInput))
	defer procSetConsoleMode.Call(uintptr(h), uintptr(mode))

	fmt.Fprint(os.Stderr, prompt)
	line, err := bufio.NewReader(os.Stdin).ReadString('\n')
	fmt.Fprintln(os.Stderr)
	if err != nil && line == "" {
		return "", false
	}
	return strings.TrimRight(line, "\r\n"), true
}

// ---------------- memory mapping ----------------

const (
	pageReadonly = 0x02
	fileMapRead  = 0x04
)

var (
	procCreateFileMapping = kernel32.NewProc("CreateFileMappingW")
	procMapViewOfFile     = kernel32.NewProc("MapViewOfFile")
	procUnmapViewOfFile   = kernel32.NewProc("UnmapViewOfFile")
)

// mmapReadOnly maps the whole file for reading. Extraction touches entries in
// arbitrary order from many goroutines at once, and mapping means the kernel
// serves those reads from the page cache with no syscall per entry.
func mmapReadOnly(f *os.File) ([]byte, func(), error) {
	st, err := f.Stat()
	if err != nil {
		return nil, nil, err
	}
	size := st.Size()
	if size == 0 {
		return nil, func() {}, nil
	}
	if size > 1<<47 {
		return nil, nil, fmt.Errorf("file is too large to map (%d bytes)", size)
	}

	h, _, errno := procCreateFileMapping.Call(
		f.Fd(), 0, pageReadonly,
		uintptr(uint32(size>>32)), uintptr(uint32(size)), 0)
	if h == 0 {
		return nil, nil, fmt.Errorf("cannot create file mapping: %v", errno)
	}
	addr, _, errno := procMapViewOfFile.Call(h, fileMapRead, 0, 0, uintptr(size))
	if addr == 0 {
		syscall.CloseHandle(syscall.Handle(h))
		return nil, nil, fmt.Errorf("cannot map view of file: %v", errno)
	}

	// `go vet` reports "possible misuse of unsafe.Pointer" on the next line, and
	// it cannot do otherwise: MapViewOfFile hands back an address as a uintptr,
	// so building a slice from it is the only way to reach mapped memory. The
	// conversion is sound here — the region belongs to the OS rather than to the
	// garbage collector, and it stays mapped until the returned closer runs.
	// `go test -gcflags=all=-d=checkptr=1` validates this at runtime and passes.
	// The vet run in run_tests.ps1 disables only this one check, so every other
	// vet diagnostic still has to be clean.
	data := unsafe.Slice((*byte)(unsafe.Pointer(addr)), size)
	closer := func() {
		procUnmapViewOfFile.Call(addr)
		syscall.CloseHandle(syscall.Handle(h))
	}
	return data, closer, nil
}
