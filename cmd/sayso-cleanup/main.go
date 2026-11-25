// Command sayso-cleanup is the cleanup runtime of Sayso: it runs the local
// cleanup models (S1-mini, Cleanup 1) with llama.cpp, through yzma and the
// prebuilt llama.cpp libraries of internal/llamalib. NDJSON requests on
// stdin, NDJSON replies on stdout, logs on stderr. It exits when stdin closes,
// after it frees the context, the model, and the llama.cpp backend, in that
// order.
//
// The protocol is protocol 1 of internal/cleanupd.
// The app starts it and kills it when a request passes its deadline
// (internal/engine/cleanup.go).
//
// SAYSO_CLEANUP_LOG sets the log level (default info). debug or trace also
// keeps the logs of llama.cpp and ggml. SAYSO_LLAMA_LIB_DIR names the folder
// of the libraries (development).
package main

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"os"
	"runtime"
	"strings"
	"time"

	"github.com/watzon/sayso/internal/cleanupd"
	"github.com/watzon/sayso/internal/cleanupd/llama"
	"github.com/watzon/sayso/internal/llamalib"
)

// downloadLimit bounds the download of the libraries, when no package has them.
const downloadLimit = 10 * time.Minute

func main() {
	// llama.cpp calls stay on one thread.
	runtime.LockOSThread()
	// First, before llama.cpp can print: stdout is for protocol lines only.
	out := protocolWriter()
	level := strings.ToLower(strings.TrimSpace(os.Getenv("SAYSO_CLEANUP_LOG")))
	var lv slog.Level
	switch level {
	case "off":
		lv = slog.LevelError + 100
	case "debug", "trace":
		lv = slog.LevelDebug
	case "warn":
		lv = slog.LevelWarn
	case "error":
		lv = slog.LevelError
	default:
		lv = slog.LevelInfo
	}
	slog.SetDefault(slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: lv})))
	slog.Info("sayso-cleanup ready", "pid", os.Getpid(), "llama.cpp", llamalib.Tag)

	exe, _ := os.Executable()
	engine := &llama.Engine{
		LibDir: func() (string, error) { return libDir(exe) },
		Found: func() bool {
			_, _, err := llamalib.Locate(exe)
			return err == nil
		},
		Logs: lv == slog.LevelDebug,
	}
	rt := cleanupd.NewRuntime(engine)
	err := cleanupd.Serve(rt, os.Stdin, out)
	rt.Close()
	if err != nil {
		slog.Error("the protocol stream failed", "error", err)
		os.Exit(1)
	}
}

// libDir finds the libraries, and downloads them into the user cache when no
// place has them.
func libDir(exe string) (string, error) {
	dir, from, err := llamalib.Locate(exe)
	if err == nil {
		slog.Info("llama.cpp libraries", "dir", dir, "from", string(from))
		return dir, nil
	}
	if !errors.Is(err, llamalib.ErrNotFound) {
		return "", err
	}
	a, _ := llamalib.Current()
	dest := llamalib.UserCacheDir(a)
	slog.Info("downloading the llama.cpp libraries", "url", a.URL, "to", dest)
	ctx, cancel := context.WithTimeout(context.Background(), downloadLimit)
	defer cancel()
	if err := llamalib.Fetch(ctx, nil, a, dest, ""); err != nil {
		return "", fmt.Errorf("cannot download the llama.cpp libraries: %w", err)
	}
	return dest, nil
}
