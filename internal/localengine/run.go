package localengine

import (
	"fmt"
	"io"
	"os"
)

// Run runs the engine on stdin and stdout until stdin closes or a shutdown
// request, and returns the exit code. It needs SAYSO_MODELS_DIR; with
// SAYSO_MODEL_STORE the models that have a descriptor are in that store.
func Run() int {
	// The stdout guard comes first: no native library has loaded yet.
	return run(NewOutput(protocolStdout()), os.Stdin, sherpaLoader{})
}

func run(out *Output, input io.Reader, l loader) int {
	modelsDir := os.Getenv("SAYSO_MODELS_DIR")
	if modelsDir == "" {
		out.Error("SAYSO_MODELS_DIR is not set", nil)
		return 2
	}
	if err := os.MkdirAll(modelsDir, 0o755); err != nil {
		out.Error(fmt.Sprintf("cannot create %s: %v", modelsDir, err), nil)
		return 2
	}
	c, err := configFromEnv(modelsDir)
	if err != nil {
		out.Error(err.Error(), nil)
		return 2
	}
	if err := openStoreFromEnv(c, out); err != nil {
		out.Error(fmt.Sprintf("cannot open the model store: %v", err), nil)
		return 2
	}
	e, err := newEngine(c, out, l)
	if err != nil {
		out.Error(err.Error(), nil)
		return 2
	}
	out.Log("info", fmt.Sprintf("sayso-engine %s ready, models in %s, pid %d", Version, modelsDir, os.Getpid()))
	e.serve(input)
	return 0
}

// openStoreFromEnv opens SAYSO_MODEL_STORE, and starts a cleanup of it
// (docs/sdk.md, "Clean up"). SAYSO_STORE_TEST_DESCRIPTORS replaces the
// built-in descriptors, for tests.
func openStoreFromEnv(c *config, out *Output) error {
	if path := os.Getenv(descriptorsEnv); path != "" {
		data, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		d, err := parseDescriptors(data)
		if err != nil {
			return err
		}
		c.descriptors = d
	}
	root := os.Getenv(storeEnv)
	if root == "" {
		return nil
	}
	s, err := openModelStore(root)
	if err != nil {
		return err
	}
	go func() {
		if err := s.cleanup(); err != nil {
			out.Log("warning", fmt.Sprintf("the cleanup of the model store failed: %v", err))
		}
	}()
	c.store = s
	return nil
}
