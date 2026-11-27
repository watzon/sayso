package protocol

import "fmt"

// Error is an error of the Host API: an error reply of the host, or an error
// that an SDK gives for the same reason (host_not_running, connection_lost,
// host_closing, and unsupported for a feature that the host lacks).
//
// errors.Is(err, &Error{Code: c}) is true for every Error with the code c.
type Error struct {
	Code       ErrorCode
	Message    string
	Capability Capability
	Reason     string
}

func (e *Error) Error() string {
	switch {
	case e.Code == "":
		return e.Message
	case e.Message == "":
		return string(e.Code)
	}
	return fmt.Sprintf("%s (%s)", e.Message, e.Code)
}

func (e *Error) Is(target error) bool {
	t, ok := target.(*Error)
	return ok && t.Code != "" && t.Code == e.Code
}

// Errorf is an Error with a code and a message.
func Errorf(code ErrorCode, format string, args ...any) *Error {
	return &Error{Code: code, Message: fmt.Sprintf(format, args...)}
}

// Err is the reply as an Error.
func (e *ErrorReply) Err() *Error {
	out := &Error{Message: e.Message}
	if e.Code != nil {
		out.Code = *e.Code
	}
	if e.Capability != nil {
		out.Capability = *e.Capability
	}
	if e.Reason != nil {
		out.Reason = *e.Reason
	}
	return out
}
