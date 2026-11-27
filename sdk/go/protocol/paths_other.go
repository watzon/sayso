//go:build !darwin && !windows

package protocol

func darwinUserTempDir() string { return "" }
