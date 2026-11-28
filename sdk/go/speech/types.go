// Package speech is Local mode without the Host: file jobs and live
// sessions that run on a speech engine of this process. It is a port of the
// jobs and live sessions of the Sayso host (sdk/PROVENANCE.md), so a job or a
// session gives the same events in both modes.
package speech

import (
	"github.com/watzon/sayso/sdk/go/audio"
	"github.com/watzon/sayso/sdk/go/protocol"
)

// Names of the protocol that jobs and sessions use.
type (
	Source          = protocol.Source
	ErrorCode       = protocol.ErrorCode
	Event           = protocol.Event
	Segment         = protocol.Segment
	Word            = protocol.Word
	Turn            = protocol.Turn
	SourceEndReason = protocol.SourceEndReason
	TimestampLevel  = protocol.TimestampLevel
	F64             = protocol.F64

	JobProgress   = protocol.JobProgress
	JobDone       = protocol.JobDone
	JobFailed     = protocol.JobFailed
	SegmentEvent  = protocol.SegmentEvent
	SpeakersEvent = protocol.SpeakersEvent
	PartialEvent  = protocol.PartialEvent
	SourceGap     = protocol.SourceGap
	SourceEnded   = protocol.SourceEnded
	PartFailed    = protocol.PartFailed
	SessionEnded  = protocol.SessionEnded
	SilenceLimit  = protocol.SilenceLimit

	// SpeechPart is a part of audio with speech.
	SpeechPart = audio.Part
	BasicVad   = audio.BasicVad
	Audio      = audio.Audio
)

const (
	SampleRate = protocol.SampleRate

	SourceMic    = protocol.SourceMic
	SourceSystem = protocol.SourceSystem
	SourceClient = protocol.SourceClient

	EndPermission = protocol.EndPermission
	EndFailed     = protocol.EndFailed

	TimestampsWord    = protocol.TimestampsWord
	TimestampsSegment = protocol.TimestampsSegment
	TimestampsNone    = protocol.TimestampsNone

	CodeBusy               = protocol.CodeBusy
	CodeBadRequest         = protocol.CodeBadRequest
	CodeCapability         = protocol.CodeCapability
	CodeUnsupported        = protocol.CodeUnsupported
	CodePermission         = protocol.CodePermission
	CodeTooManyStreams     = protocol.CodeTooManyStreams
	CodeModelNotDownloaded = protocol.CodeModelNotDownloaded
	CodeUnknownModel       = protocol.CodeUnknownModel
	CodeWrongRole          = protocol.CodeWrongRole
	CodeUnavailable        = protocol.CodeUnavailable
)

// NewBasicVad is the energy VAD of the audio package.
func NewBasicVad() *BasicVad { return audio.NewBasicVad() }

// ReadAudio reads an audio file as 16 kHz mono.
func ReadAudio(path string) (*Audio, error) { return audio.ReadFile(path) }

// DecodeF32 reads base64 of 16-bit PCM as floats.
func DecodeF32(pcm string) ([]float32, error) { return protocol.DecodeF32(pcm) }

// EventSink takes the events of a job or a session.
type EventSink interface{ Event(Event) }

// SinkFunc is an EventSink of a function.
type SinkFunc func(Event)

func (f SinkFunc) Event(e Event) { f(e) }
