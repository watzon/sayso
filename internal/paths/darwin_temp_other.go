//go:build !darwin && !windows

package paths

func darwinUserTempDir() string { return "" }
