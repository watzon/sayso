package localengine

import (
	"math"
	"sort"
)

// clusterThreshold is the clustering threshold when the client gives no
// number of speakers. A lower value finds more speakers. The sherpa-onnx
// default, 0.5, found 20 to 30 speakers in meetings of 4 to 6; the benchmark
// in docs/extensions.md had the fewest errors at 0.9 to 1.0.
const clusterThreshold = 0.9

// rawTurn is a turn as the model gives it, in seconds, with the model's
// speaker number.
type rawTurn struct {
	start, end float32
	speaker    int32
}

// turn is a turn of the protocol.
type turn struct {
	StartMs uint64 `json:"start_ms"`
	EndMs   uint64 `json:"end_ms"`
	Speaker uint32 `json:"speaker"`
}

// finishDiarization makes the turns of the protocol: whole milliseconds inside
// the audio, speakers numbered from 0 in the order in which they first speak,
// the turns of one speaker joined where they touch or overlap, in order.
func finishDiarization(raw []rawTurn, audioMs uint64) (speakers uint32, turns []turn) {
	ms := func(seconds float32) uint64 {
		v := math.Round(float64(seconds) * 1000)
		if v < 0 {
			v = 0
		}
		return min(uint64(v), audioMs)
	}
	type span struct {
		start, end uint64
		speaker    int32
	}
	var spans []span
	for _, t := range raw {
		s := span{ms(t.start), ms(t.end), t.speaker}
		if s.end > s.start {
			spans = append(spans, s)
		}
	}
	sort.Slice(spans, func(i, j int) bool {
		a, b := spans[i], spans[j]
		if a.start != b.start {
			return a.start < b.start
		}
		if a.end != b.end {
			return a.end < b.end
		}
		return a.speaker < b.speaker
	})
	numbers := map[int32]uint32{}
	for _, s := range spans {
		if _, ok := numbers[s.speaker]; !ok {
			numbers[s.speaker] = uint32(len(numbers))
		}
	}
	turns = []turn{}
	lastOf := map[uint32]int{}
	for _, s := range spans {
		speaker := numbers[s.speaker]
		if at, ok := lastOf[speaker]; ok && s.start <= turns[at].EndMs {
			turns[at].EndMs = max(turns[at].EndMs, s.end)
			continue
		}
		lastOf[speaker] = len(turns)
		turns = append(turns, turn{StartMs: s.start, EndMs: s.end, Speaker: speaker})
	}
	sort.SliceStable(turns, func(i, j int) bool {
		a, b := turns[i], turns[j]
		if a.StartMs != b.StartMs {
			return a.StartMs < b.StartMs
		}
		if a.EndMs != b.EndMs {
			return a.EndMs < b.EndMs
		}
		return a.Speaker < b.Speaker
	})
	return uint32(len(numbers)), turns
}
