package main

import "github.com/jberkenbilt/mock-free-test-coordination/go/scanner"

func main() {
	s := scanner.New()
	s.SetPattern(".*\\.go")
	s.Run()
}
