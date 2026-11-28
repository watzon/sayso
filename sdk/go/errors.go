package sayso

import "github.com/watzon/sayso/sdk/go/protocol"

// Error is every error of the SDK that has a code: an error reply of Sayso,
// or the same reason found by the SDK. errors.Is(err, ErrBusy) is true for
// each Error with the code busy.
type Error = protocol.Error

// ErrorCode is the code of an Error. A code that this version does not know
// is kept as it is.
type ErrorCode = protocol.ErrorCode

// The sentinels of the error codes, for errors.Is.
var (
	ErrNotApproved        = &Error{Code: protocol.CodeNotApproved}
	ErrCapability         = &Error{Code: protocol.CodeCapability}
	ErrBusy               = &Error{Code: protocol.CodeBusy}
	ErrDeclined           = &Error{Code: protocol.CodeDeclined}
	ErrModelNotDownloaded = &Error{Code: protocol.CodeModelNotDownloaded}
	ErrWrongRole          = &Error{Code: protocol.CodeWrongRole}
	ErrUnsupported        = &Error{Code: protocol.CodeUnsupported}
	ErrBadRequest         = &Error{Code: protocol.CodeBadRequest}
	ErrTooManyStreams     = &Error{Code: protocol.CodeTooManyStreams}
	ErrUnknownModel       = &Error{Code: protocol.CodeUnknownModel}
	ErrModelRetired       = &Error{Code: protocol.CodeModelRetired}
	ErrUnavailable        = &Error{Code: protocol.CodeUnavailable}
	ErrRefused            = &Error{Code: protocol.CodeRefused}
	ErrTimeout            = &Error{Code: protocol.CodeTimeout}
	ErrPermission         = &Error{Code: protocol.CodePermission}
	ErrHostNotRunning     = &Error{Code: protocol.CodeHostNotRunning}
	ErrConnectionLost     = &Error{Code: protocol.CodeConnectionLost}
	ErrHostClosing        = &Error{Code: protocol.CodeHostClosing}
)
