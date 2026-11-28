//go:build !darwin

package audio

const systemFormats = ""

func readSystem(string) ([]float32, error) {
	return nil, ErrUnsupported
}
