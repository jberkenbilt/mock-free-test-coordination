// Comments throughout of the form "Note n: ..." are expanded in the
// text of the post.

package scanner_test

import (
	"io/fs"
	"os"
	"path"
	"testing"
	"time"

	"github.com/jberkenbilt/mock-free-test-coordination/go/scanner"
)

// Note 1: helper function
func checkNil(t *testing.T, err error) {
	t.Helper()
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
}

// Note 2: life-cycle test
func TestLifeCycle(t *testing.T) {
	dir := t.TempDir()
	defer func() {
		_ = os.RemoveAll(dir)
	}()
	// Make a scanner with a fake ticker so we can control loop iterations.
	// Create a test channel so we can monitor private state, stored in
	// a local variable, after each iteration.
	ticker := make(chan time.Time, 1)
	fileChan := make(chan string, 1)
	handler := func(name string, _info fs.FileInfo) {
		fileChan <- name
	}
	noFile := func(t *testing.T) {
		t.Helper()
		select {
		case <-fileChan:
			t.Error("a file was unexpectedly handled")
		default:
		}
	}

	stateChan := make(chan *map[string]*scanner.Stat, 1)
	// Note 3: Override things for test
	s := scanner.New(
		scanner.WithDir(dir),
		scanner.WithHandler(handler),
		scanner.TestWithTicker(ticker),
		scanner.TestWithStateChan(stateChan),
	)
	done := make(chan bool, 1)
	// Note 4: Run the `Run` method.
	go func() {
		s.Run()
		done <- true
	}()

	// Note 5: Force one iteration of the loop
	iterate := func() *map[string]*scanner.Stat {
		ticker <- time.Now()
		return <-stateChan
	}
	// Note 6: Run the full life cycle.
	state := iterate()
	if len(*state) != 0 {
		t.Error("state is unexpectedly non-empty")
	}
	// Set a pattern, create some files, and iterate. We don't expect
	// anything yet since no files will have stabilized in size and
	// modification time.
	s.SetPattern(".*\\.a")
	checkNil(
		t,
		os.WriteFile(path.Join(dir, "one.a"), []byte("potato"), 0o666),
	)
	checkNil(
		t,
		os.WriteFile(path.Join(dir, "one.b"), []byte("salad"), 0o666),
	)
	state = iterate()
	noFile(t)
	if _, ok := (*state)["one.a"]; !ok {
		t.Error("one.a is not in state")
	}
	// We don't expect one.b since it doesn't match the pattern.
	if _, ok := (*state)["one.b"]; ok {
		t.Error("one.b is in state")
	}
	iterate()
	// The file is stable now. It should be passed to the handler.
	if <-fileChan != "one.a" {
		t.Error("didn't get file from handler")
	}
	// Iterate again. The handler should not be called again.
	iterate()
	noFile(t)

	// Change pattern and iterate. We should see the other file. Then
	// modify the file and iterate again. At the third iteration, the
	// file is stable, and the handler is called.
	s.SetPattern(".*\\.b")
	state = iterate()
	if _, ok := (*state)["one.a"]; ok {
		t.Error("one.a is still in state after changing pattern")
	}
	if _, ok := (*state)["one.b"]; !ok {
		t.Error("one.b is not in state after changing pattern")
	}
	checkNil(
		t,
		os.WriteFile(path.Join(dir, "one.b"), []byte("salad!"), 0o666),
	)
	iterate()
	noFile(t)
	iterate()
	if <-fileChan != "one.b" {
		t.Error("didn't see one.b")
	}
	// We shouldn't get another handler call.
	iterate()
	noFile(t)

	// Send an empty string as a pattern to cause the scanner to exit.
	s.SetPattern("")
	<-done
}

// Note 7: test the real ticker
func TestRealTicker(t *testing.T) {
	// This test exercises using default values for everything except
	// the loop interval. It exercises that the default ticker works
	// by ensuring that two loop iterations take at least
	// approximately two intervals.
	stateChan := make(chan *map[string]*scanner.Stat, 1)
	s := scanner.New(
		scanner.WithInterval(10*time.Millisecond),
		scanner.TestWithStateChan(stateChan),
	)
	now := time.Now()
	done := make(chan struct{}, 1)
	go func() {
		s.Run()
		done <- struct{}{}
	}()
	// We know the ticker is behaving like a ticker because we can see
	// multiple state updates without having to poke the loop.
	<-stateChan
	<-stateChan
	elapsed := time.Since(now)
	// This is a little bit fragile because of the upper bound check,
	// but we need some upper bound check to ensure that the
	// overridden interval is actually being seen. A test to make sure
	// it took less than the default interval does this and gives us a
	// great deal of headroom even in a congested CI environment. If
	// this fails, proceed with caution in loosening the upper bound
	// or use some other method to exercise the ticker.
	if elapsed < 20*time.Millisecond || elapsed >= scanner.DefaultInterval {
		t.Errorf("unexpected time elapsed: %v", elapsed)
	}
	s.SetPattern("")
	<-done
}
