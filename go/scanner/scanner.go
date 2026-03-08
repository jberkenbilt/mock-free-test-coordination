// Package scanner scans a directory waiting for files to stabilize.
// When they have stabilized, a configured handler is called. This
// code accompanies the blog post mentioned in the top-level
// README.md. Comments throughout of the form "Note n: ..." are
// expanded in the text of the post.
package scanner

import (
	"fmt"
	"io/fs"
	"log/slog"
	"maps"
	"os"
	"regexp"
	"time"
)

const DefaultInterval = 2 * time.Second

// Note 1: the scanner struct contains all private fields. An instance
// is created using the `New` function, which takes zero or more
// Options functions as arguments.

type Options func(*Scanner)

type Scanner struct {
	dir       string
	handler   func(string, fs.FileInfo)
	interval  time.Duration
	patChan   chan string
	ticker    <-chan time.Time
	stateChan chan<- *map[string]*Stat
}

func New(options ...Options) *Scanner {
	s := &Scanner{}
	for _, fn := range options {
		fn(s)
	}
	// Note 2: create sensible defaults after calling options
	// functions.
	if s.patChan == nil {
		s.patChan = make(chan string, 10)
	}
	if s.interval == 0 {
		s.interval = DefaultInterval
	}
	if s.ticker == nil {
		s.ticker = time.NewTicker(s.interval).C
	}
	if s.dir == "" {
		s.dir = "."
	}
	if s.handler == nil {
		s.handler = func(name string, info fs.FileInfo) {
			slog.Info("found file", "name", name, "size", info.Size())
		}
	}
	return s
}

// Note 3: Options functions

// WithInterval overrides the default loop iteration interval.
func WithInterval(duration time.Duration) Options {
	return func(s *Scanner) {
		s.interval = duration
	}
}

// WithDir sets the directory to scan.
func WithDir(dir string) Options {
	return func(s *Scanner) {
		s.dir = dir
	}
}

// WithHandler provides a handler that gets called whenever a file has
// remained stable in size and modification time for two consecutive
// scans.
func WithHandler(h func(string, fs.FileInfo)) Options {
	return func(s *Scanner) {
		s.handler = h
	}
}

// Note 4. Test-only Options Functions

// TestWithTicker allows us to supply our own loop ticker. This enables
// the test code to explicitly control loop iteration.
func TestWithTicker(ticker <-chan time.Time) Options {
	return func(s *Scanner) {
		s.ticker = ticker
	}
}

// TestWithStateChan allows the test code to supply a channel that
// receives a pointer to internal state after each loop iteration.
func TestWithStateChan(stateChan chan<- *map[string]*Stat) Options {
	return func(s *Scanner) {
		s.stateChan = stateChan
	}
}

type Stat = struct {
	size    int64
	modTime time.Time
	called  bool
}

func (s *Scanner) SetPattern(p string) {
	s.patChan <- p
}

func (s *Scanner) Run() {
	var pattern *regexp.Regexp
	fileData := map[string]*Stat{}
	for {
		// Note 5: use of select; use of empty pattern to trigger exit.
		select {
		case p := <-s.patChan:
			if p == "" {
				return
			}
			re, err := regexp.Compile(p)
			if err != nil {
				slog.Error("invalid regular expression", "error", err)
				pattern = nil
			} else {
				pattern = re
			}
			continue
		case <-s.ticker:
		}
		// Note 6: main body of scanner
		entries, err := os.ReadDir(s.dir)
		if err != nil {
			slog.Error(fmt.Sprintf("opening %s as directory", s.dir), "error", err)
			return
		}
		seen := map[string]struct{}{}
		for _, e := range entries {
			name := e.Name()
			info, err := e.Info()
			if err != nil {
				slog.Warn(
					fmt.Sprintf("unable to get info on %s/%s", s.dir, name),
					"error",
					err,
				)
				continue
			}
			if pattern == nil {
				continue
			}
			if !pattern.Match([]byte(name)) {
				continue
			}
			seen[name] = struct{}{}
			modTime := info.ModTime()
			size := info.Size()
			stat := Stat{
				size:    size,
				modTime: modTime,
			}
			old, ok := fileData[name]
			if ok && old.size == stat.size && old.modTime.Equal(stat.modTime) {
				if !old.called {
					old.called = true
					s.handler(name, info)
				}
			} else {
				fileData[name] = &stat
			}
		}
		for name := range maps.Keys(fileData) {
			if _, ok := seen[name]; !ok {
				delete(fileData, name)
			}
		}
		// Note 7: test support code: send state
		if s.stateChan != nil {
			// We are sharing a pointer to a private, internal,
			// locally declared variable. On the plus side, this
			// prevents us from having to expose something just so the
			// tests can see it. On the minus side, there could be a
			// potential race here. The test code must avoid doing
			// anything with the state while there is a chance of the
			// loop running.
			s.stateChan <- &fileData
		}
	}
}
