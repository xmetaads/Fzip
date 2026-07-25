// Shared foundations: exit codes, formatting, DOS timestamps, progress.

package main

import (
	"fmt"
	"os"
	"strings"
	"sync/atomic"
	"time"
)

// Exit codes, following the 7-Zip convention.
const (
	ExitOK      = 0
	ExitWarning = 1
	ExitFatal   = 2
	ExitUsage   = 7
)

// ---------------- formatting ----------------

func fmtSize(n uint64) string {
	units := [...]string{"B", "KB", "MB", "GB", "TB", "PB"}
	v := float64(n)
	u := 0
	for v >= 1024 && u < len(units)-1 {
		v /= 1024
		u++
	}
	if u == 0 {
		return fmt.Sprintf("%d %s", n, units[u])
	}
	return fmt.Sprintf("%.2f %s", v, units[u])
}

func fmtDuration(secs float64) string {
	switch {
	case secs < 60:
		return fmt.Sprintf("%.0fs", secs)
	case secs < 3600:
		return fmt.Sprintf("%.0fm%02.0fs", secs/60, float64(int(secs)%60))
	default:
		return fmt.Sprintf("%.0fh%02.0fm", secs/3600, float64(int(secs)%3600)/60)
	}
}

// ---------------- DOS date/time ----------------
//
// ZIP stores timestamps in LOCAL time. Go's time.Local applies the timezone
// rules that were in force on the date being converted, so a July timestamp
// gets the summer offset and a January one gets the winter offset. Using a
// single "current" offset for both would make fzip disagree with every other
// archiver for half the year and drift by an hour on each round trip.

// dosToUnix converts a DOS date/time pair (local) to a Unix timestamp (UTC).
// Returns ok=false when any field is nonsense, which matters because every one
// of them is attacker-controlled.
func dosToUnix(date, dtime uint16) (int64, bool) {
	day := int(date & 31)
	month := int((date >> 5) & 15)
	year := 1980 + int((date>>9)&127)
	sec := int((dtime & 31) * 2)
	min := int((dtime >> 5) & 63)
	hour := int((dtime >> 11) & 31)

	if day == 0 || month < 1 || month > 12 || hour > 23 || min > 59 || sec > 59 {
		return 0, false
	}
	t := time.Date(year, time.Month(month), day, hour, min, sec, 0, time.Local)
	return t.Unix(), true
}

// unixToDos converts a Unix timestamp (UTC) to a DOS date/time pair (local).
func unixToDos(unix int64) (date, dtime uint16) {
	t := time.Unix(unix, 0).In(time.Local)
	return packDos(t.Year(), int(t.Month()), t.Day(), t.Hour(), t.Minute(), t.Second())
}

// packDos packs a local civil time into the DOS pair. DOS cannot represent
// anything before 1980, so earlier timestamps clamp to the very start of its
// range rather than keeping a month and day that no longer belong to the year.
func packDos(y, mo, d, h, mi, s int) (uint16, uint16) {
	if y < 1980 {
		return (1 << 5) | 1, 0 // 1980-01-01 00:00
	}
	if y > 2107 {
		return (127 << 9) | (12 << 5) | 31, (23 << 11) | (59 << 5) | 29
	}
	date := uint16(y-1980)<<9 | uint16(mo)<<5 | uint16(d)
	dtime := uint16(h)<<11 | uint16(mi)<<5 | uint16(s/2)
	return date, dtime
}

// ---------------- progress bar ----------------

type Progress struct {
	bytesDone  atomic.Uint64
	filesDone  atomic.Uint64
	totalBytes uint64
	totalFiles uint64
	enabled    bool

	stopCh chan struct{}
	doneCh chan struct{}
}

func NewProgress(totalBytes, totalFiles uint64, enabled bool) *Progress {
	return &Progress{
		totalBytes: totalBytes,
		totalFiles: totalFiles,
		enabled:    enabled,
		stopCh:     make(chan struct{}),
		doneCh:     make(chan struct{}),
	}
}

func (p *Progress) AddBytes(n uint64) { p.bytesDone.Add(n) }
func (p *Progress) AddFile()          { p.filesDone.Add(1) }
func (p *Progress) BytesDone() uint64 { return p.bytesDone.Load() }

func (p *Progress) render(start time.Time, lastLen *int) {
	done := p.bytesDone.Load()
	files := p.filesDone.Load()
	secs := time.Since(start).Seconds()
	var speed float64
	if secs > 0.01 {
		speed = float64(done) / secs
	}

	var line string
	if p.totalBytes == 0 && p.totalFiles == 0 {
		// Totals unknown: show a plain activity line rather than a fake bar.
		line = fmt.Sprintf("\r  %d files  %s  %s/s", files, fmtSize(done), fmtSize(uint64(speed)))
	} else {
		var pct float64
		if p.totalBytes > 0 {
			pct = float64(done) / float64(p.totalBytes) * 100
		} else {
			pct = float64(files) / float64(p.totalFiles) * 100
		}
		if pct > 100 {
			pct = 100
		}
		eta := "--"
		if speed > 1 && p.totalBytes > done {
			eta = fmtDuration(float64(p.totalBytes-done) / speed)
		}
		const w = 24
		filled := int(pct / 100 * w)
		var bar strings.Builder
		for i := 0; i < w; i++ {
			switch {
			case i < filled:
				bar.WriteByte('=')
			case i == filled && pct < 100:
				bar.WriteByte('>')
			default:
				bar.WriteByte(' ')
			}
		}
		line = fmt.Sprintf("\r[%s] %5.1f%%  %d/%d  %s  %s/s  ETA %s",
			bar.String(), pct, files, p.totalFiles, fmtSize(done), fmtSize(uint64(speed)), eta)
	}

	pad := *lastLen - len([]rune(line))
	if pad < 0 {
		pad = 0
	}
	*lastLen = len([]rune(line))
	os.Stdout.WriteString(line + strings.Repeat(" ", pad))
}

// Start begins rendering in the background. Stop must always be called.
func (p *Progress) Start(start time.Time) {
	if !p.enabled {
		close(p.doneCh)
		return
	}
	go func() {
		defer close(p.doneCh)
		lastLen := 0
		tick := time.NewTicker(100 * time.Millisecond)
		defer tick.Stop()
		for {
			select {
			case <-p.stopCh:
				p.render(start, &lastLen)
				fmt.Println()
				return
			case <-tick.C:
				p.render(start, &lastLen)
			}
		}
	}()
}

func (p *Progress) Stop() {
	if p.enabled {
		close(p.stopCh)
	}
	<-p.doneCh
}

// progressEnabled draws a bar only on a real terminal, so logs and pipes stay
// clean.
func progressEnabled(force, quiet, verbose bool) bool {
	return !quiet && !verbose && (force || isTerminal(os.Stdout))
}
