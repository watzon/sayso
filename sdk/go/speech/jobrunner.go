package speech

import (
	"fmt"
	"sort"
	"strconv"
	"strings"
)

// The runner of a job (sayso-speech/src/jobs/runner.rs, plan.rs,
// speakers.rs): read the file, find speech, cut chunks, transcribe each
// chunk, and assign speakers.

const (
	// TranscriptionShare is the part of the progress for transcription when
	// a job also diarizes.
	TranscriptionShare     = 0.9
	maxChunkMs             = 30_000
	maxChunkMsWithoutTimes = 10_000
	maxJoinPauseMs         = 3_000
	padMs                  = 200
	vadWindowMs            = 10 * 60 * 1000
	reachesEndMs           = 1_000
	smallSpeakerShare      = 0.02
	smallSpeakerMs         = 10_000
)

// JobRunner runs jobs on the engine of the app.
type JobRunner struct {
	engine      Engine
	models      *ModelUse
	vadWindowMs uint64
}

// NewJobRunner runs on eng and tells models which models it holds.
func NewJobRunner(eng Engine, models *ModelUse) *JobRunner {
	return &JobRunner{engine: eng, models: models, vadWindowMs: vadWindowMs}
}

// WithVadWindow gives the VAD model windows of ms (tests).
func (r *JobRunner) WithVadWindow(ms uint64) *JobRunner { r.vadWindowMs = ms; return r }

// errStopped is a job that stopped: cancelled, or failed with a message.
type errStopped struct {
	cancelled bool
	message   string
}

func (e *errStopped) Error() string { return e.message }

var stopCancelled = &errStopped{cancelled: true}

func failed(format string, args ...any) *errStopped {
	return &errStopped{message: fmt.Sprintf(format, args...)}
}

// Run runs a job to its end.
func (r *JobRunner) Run(job JobSpec, run *JobRun) {
	text, err := r.runJob(job, run)
	if err != nil {
		run.Stopped(err.cancelled, err.message)
		return
	}
	run.Event(&JobDone{Job: run.ID, Text: text})
}

func (r *JobRunner) runJob(job JobSpec, run *JobRun) (string, *errStopped) {
	audio, err := ReadAudio(job.Path)
	if err != nil {
		return "", failed("%s", err.Error())
	}
	if run.Cancelled() {
		return "", stopCancelled
	}
	if r.models != nil {
		release := r.models.Hold(job.Models())
		defer release()
	}
	before := func() *errStopped {
		if !run.BeforeEngineCall() {
			return stopCancelled
		}
		return nil
	}
	for _, m := range job.Models() {
		if e := before(); e != nil {
			return "", e
		}
		if err := r.load(m); err != nil {
			return "", failed("cannot load the model %s: %v", m, err)
		}
	}
	parts, stop := speechParts(audio, job.Vad, r.engine, r.vadWindowMs, before)
	if stop != nil {
		return "", stop
	}
	maxMs := uint64(maxChunkMs)
	if job.Timestamps == TimestampsNone {
		maxMs = maxChunkMsWithoutTimes
	}
	chunks := planChunks(parts, audio.LenMs(), maxMs)
	share := 1.0
	if job.Diarization != "" {
		share = TranscriptionShare
	}
	lenMs := max(audio.LenMs(), 1)
	var times [][2]uint64
	var texts []string
	for i, c := range chunks {
		if e := before(); e != nil {
			return "", e
		}
		segments, e := r.transcribe(audio, c, job)
		if e != nil {
			return "", e
		}
		for _, s := range segments {
			times = append(times, [2]uint64{s.StartMs, s.EndMs})
			texts = append(texts, s.Text)
			id := run.ID
			run.Event(&SegmentEvent{Job: &id, Segment: s})
		}
		done := min(c.EndMs, lenMs)
		if i+1 == len(chunks) {
			done = lenMs
		}
		run.Event(&JobProgress{Job: run.ID, Fraction: F64(share * float64(done) / float64(lenMs))})
	}
	if job.Diarization != "" {
		if e := before(); e != nil {
			return "", e
		}
		turns, err := r.engine.Diarize(job.Diarization, audio.AllFloats(), job.Speakers)
		if err != nil {
			return "", failed("Diarization failed: %v", err)
		}
		if job.Speakers == nil {
			turns = foldSmallSpeakers(turns)
		}
		speakers := assignSpeakers(times, turns)
		id := run.ID
		run.Event(&SpeakersEvent{Job: &id, Turns: segmentTurns(times, speakers)})
	}
	return strings.Join(texts, " "), nil
}

func (r *JobRunner) load(model string) error {
	if r.engine.Status(model).IsUsable() {
		return nil
	}
	return r.engine.Load(model)
}

func (r *JobRunner) transcribe(audio *Audio, c Chunk, job JobSpec) ([]Segment, *errStopped) {
	samples := audio.Floats(c.StartMs, c.EndMs)
	if job.Timestamps == TimestampsNone {
		text, err := r.engine.Transcribe(samples, job.Model, job.Language)
		if err != nil {
			return nil, failed("Transcription failed: %v", err)
		}
		text = strings.TrimSpace(text)
		if text == "" {
			return nil, nil
		}
		return []Segment{{StartMs: c.StartMs, EndMs: c.EndMs, Text: text}}, nil
	}
	found, err := r.transcribeSegments(samples, job)
	if err != nil {
		return nil, failed("Transcription failed: %v", err)
	}
	shift := c.StartMs
	var out []Segment
	for _, s := range found {
		text := strings.TrimSpace(s.Text)
		if text == "" {
			continue
		}
		seg := Segment{StartMs: s.StartMs + shift, EndMs: s.EndMs + shift, Text: text}
		if s.Words != nil {
			seg.Words = make([]Word, len(s.Words))
			for i, w := range s.Words {
				seg.Words[i] = Word{StartMs: w.StartMs + shift, EndMs: w.EndMs + shift, Text: w.Text}
			}
		}
		out = append(out, seg)
	}
	return out, nil
}

// transcribeSegments asks for segments, or makes one segment of the text
// when the engine has no segments (backend.rs).
func (r *JobRunner) transcribeSegments(samples []float32, job JobSpec) ([]Segment, error) {
	if !hasFeature(r.engine.Features(), "segments") {
		text, err := r.engine.Transcribe(samples, job.Model, job.Language)
		if err != nil {
			return nil, err
		}
		endMs := uint64(len(samples)) * 1000 / SampleRate
		return []Segment{{StartMs: 0, EndMs: endMs, Text: strings.TrimSpace(text)}}, nil
	}
	return r.engine.TranscribeSegments(samples, job.Model, job.Language)
}

// hasFeature is true when the engine names the feature, or names none (an
// old engine has them all).
func hasFeature(features []string, feature string) bool {
	if len(features) == 0 {
		return true
	}
	for _, f := range features {
		if f == feature {
			return true
		}
	}
	return false
}

// Chunk is a part of the audio that one engine call transcribes.
type Chunk struct{ StartMs, EndMs uint64 }

func satSub(a, b uint64) uint64 {
	if a < b {
		return 0
	}
	return a - b
}

// planChunks joins speech parts into chunks of at most maxMs, with a pad.
func planChunks(parts []SpeechPart, lenMs, maxMs uint64) []Chunk {
	var ps []SpeechPart
	for _, p := range parts {
		p = SpeechPart{StartMs: min(p.StartMs, lenMs), EndMs: min(p.EndMs, lenMs)}
		if p.EndMs > p.StartMs {
			ps = append(ps, p)
		}
	}
	sort.SliceStable(ps, func(i, j int) bool { return ps[i].StartMs < ps[j].StartMs })
	var joined []Chunk
	for _, p := range ps {
		if n := len(joined); n > 0 {
			last := &joined[n-1]
			if p.StartMs <= last.EndMs+maxJoinPauseMs && max(p.EndMs, last.EndMs)-last.StartMs <= maxMs {
				last.EndMs = max(last.EndMs, p.EndMs)
				continue
			}
		}
		joined = append(joined, Chunk{p.StartMs, p.EndMs})
	}
	padded := make([]Chunk, 0, len(joined))
	for i, c := range joined {
		room := satSub(maxMs, c.EndMs-c.StartMs) / 2
		pad := min(uint64(padMs), room)
		var prevEnd uint64
		if len(padded) > 0 {
			prevEnd = padded[len(padded)-1].EndMs
		}
		nextStart := lenMs
		if i+1 < len(joined) {
			nextStart = joined[i+1].StartMs
		}
		padded = append(padded, Chunk{StartMs: max(satSub(c.StartMs, pad), prevEnd), EndMs: min(c.EndMs+pad, nextStart, lenMs)})
	}
	return padded
}

func speechParts(audio *Audio, vad VadRun, eng Engine, windowMs uint64, before func() *errStopped) ([]SpeechPart, *errStopped) {
	if vad.Basic || vad.Model == "" {
		return basicParts(audio), nil
	}
	lenMs := audio.LenMs()
	windowMs = max(windowMs, 1)
	var parts []SpeechPart
	var start uint64
	for start < lenMs {
		if e := before(); e != nil {
			return nil, e
		}
		end := min(start+windowMs, lenMs)
		found, err := eng.DetectSpeech(vad.Model, audio.Floats(start, end))
		if err != nil {
			return nil, failed("Voice detection failed: %v", err)
		}
		shifted := make([]SpeechPart, len(found))
		for i, p := range found {
			shifted[i] = SpeechPart{StartMs: start + p.StartMs, EndMs: min(start+p.EndMs, end)}
		}
		next := end
		if end < lenMs && len(shifted) > 0 {
			last := shifted[len(shifted)-1]
			if last.EndMs+reachesEndMs >= end && last.StartMs > start {
				next = last.StartMs
				shifted = shifted[:len(shifted)-1]
			}
		}
		parts = append(parts, shifted...)
		start = next
	}
	return parts, nil
}

func basicParts(audio *Audio) []SpeechPart {
	vad := NewBasicVad()
	var parts []SpeechPart
	samples := audio.Samples()
	floats := make([]float32, 0, SampleRate)
	for i := 0; i < len(samples); i += SampleRate {
		end := min(i+SampleRate, len(samples))
		floats = floats[:0]
		for _, s := range samples[i:end] {
			floats = append(floats, float32(s)/32768)
		}
		parts = append(parts, vad.Accept(floats)...)
	}
	return append(parts, vad.Finish()...)
}

// foldSmallSpeakers gives the turns of speakers with little speech to the
// speaker who overlaps them most.
func foldSmallSpeakers(turns []RawTurn) []RawTurn {
	speech := map[uint32]uint64{}
	for _, t := range turns {
		speech[t.Speaker] += satSub(t.EndMs, t.StartMs)
	}
	var all uint64
	for _, ms := range speech {
		all += ms
	}
	var largest *uint32
	for s, ms := range speech {
		if largest == nil || ms > speech[*largest] || (ms == speech[*largest] && s < *largest) {
			v := s
			largest = &v
		}
	}
	small := func(s uint32) bool {
		ms := speech[s]
		return (largest == nil || s != *largest) && ms < smallSpeakerMs && float64(ms) < smallSpeakerShare*float64(all)
	}
	var kept []RawTurn
	for _, t := range turns {
		if !small(t.Speaker) {
			kept = append(kept, t)
		}
	}
	out := make([]RawTurn, len(turns))
	for i, t := range turns {
		if small(t.Speaker) {
			if s, ok := bestSpeaker(t.StartMs, t.EndMs, kept); ok {
				t.Speaker = s
			}
		}
		out[i] = t
	}
	sort.SliceStable(out, func(i, j int) bool {
		if out[i].StartMs != out[j].StartMs {
			return out[i].StartMs < out[j].StartMs
		}
		return out[i].EndMs < out[j].EndMs
	})
	return out
}

func assignSpeakers(segments [][2]uint64, turns []RawTurn) []*uint32 {
	out := make([]*uint32, len(segments))
	for i, s := range segments {
		if sp, ok := bestSpeaker(s[0], s[1], turns); ok {
			v := sp
			out[i] = &v
		}
	}
	return out
}

func segmentTurns(segments [][2]uint64, speakers []*uint32) []Turn {
	labels := map[uint32]string{}
	type turn struct {
		speaker    uint32
		start, end uint64
	}
	var turns []turn
	var last *uint32
	for i, s := range segments {
		sp := speakers[i]
		switch {
		case sp != nil && len(turns) > 0 && last != nil && *last == *sp:
			turns[len(turns)-1].end = max(turns[len(turns)-1].end, s[1])
		case sp != nil:
			turns = append(turns, turn{*sp, s[0], s[1]})
		}
		last = sp
	}
	out := make([]Turn, 0, len(turns))
	for _, t := range turns {
		label, ok := labels[t.speaker]
		if !ok {
			label = strconv.Itoa(len(labels) + 1)
			labels[t.speaker] = label
		}
		out = append(out, Turn{StartMs: t.start, EndMs: t.end, Speaker: label})
	}
	return out
}

func bestSpeaker(start, end uint64, turns []RawTurn) (uint32, bool) {
	overlap := map[uint32]uint64{}
	var nearestGap uint64
	var nearest uint32
	hasNearest := false
	for _, t := range turns {
		if shared := satSub(min(end, t.EndMs), max(start, t.StartMs)); shared > 0 {
			overlap[t.Speaker] += shared
		}
		gap := max(satSub(t.StartMs, end), satSub(start, t.EndMs))
		if !hasNearest || gap < nearestGap || (gap == nearestGap && t.Speaker < nearest) {
			nearestGap, nearest, hasNearest = gap, t.Speaker, true
		}
	}
	var most uint32
	var mostMs uint64
	found := false
	for s, ms := range overlap {
		if !found || ms > mostMs || (ms == mostMs && s < most) {
			most, mostMs, found = s, ms, true
		}
	}
	if found {
		return most, true
	}
	return nearest, hasNearest
}
