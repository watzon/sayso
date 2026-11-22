package main

import (
	"fmt"
	"os"
)

func main() {
	fmt.Fprintln(os.Stderr, "sayso-engine runs on Linux and Windows. On macOS, Sayso uses SaysoEngine (native/macos/SaysoEngine).")
	os.Exit(2)
}
