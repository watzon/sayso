// The result of diarization (docs/engine-protocol.md, "Diarization").
//
// The offline diarizer of FluidAudio gives turns with its own speaker labels, in seconds.
// `Diarization.finish` makes the turns of the protocol: whole milliseconds, speakers
// numbered from 0 in the order in which they first speak, the turns of one speaker joined
// where they touch or overlap, and the turns in order.
import Foundation

enum Diarization {
    /// A turn as the model gives it. The speaker is the label of the model.
    struct RawTurn: Equatable {
        let start: Double
        let end: Double
        let speaker: String
    }

    /// A turn of the protocol.
    struct Turn: Equatable {
        let startMs: Int
        let endMs: Int
        let speaker: Int
    }

    /// The reply of `diarize`.
    struct Result: Equatable {
        let speakers: Int
        let turns: [Turn]

        var json: [String: Any] {
            [
                "speakers": speakers,
                "turns": turns.map { ["start_ms": $0.startMs, "end_ms": $0.endMs, "speaker": $0.speaker] },
            ]
        }
    }

    /// Make the turns of the protocol. `audioMs` is the length of the audio: no turn passes it.
    static func finish(_ raw: [RawTurn], audioMs: Int) -> Result {
        func ms(_ seconds: Double) -> Int {
            guard seconds.isFinite else { return 0 }
            return min(max(Int((seconds * 1000).rounded()), 0), audioMs)
        }
        let turns = raw
            .map { (start: ms($0.start), end: ms($0.end), speaker: $0.speaker) }
            .filter { $0.end > $0.start }
            // The order of the first turns gives the numbers.
            .sorted { ($0.start, $0.end, $0.speaker) < ($1.start, $1.end, $1.speaker) }
        var numbers: [String: Int] = [:]
        for turn in turns where numbers[turn.speaker] == nil {
            numbers[turn.speaker] = numbers.count
        }
        // Join the turns of each speaker. The turns of one speaker are in start order now.
        var joined: [Turn] = []
        var lastOf: [Int: Int] = [:]
        for turn in turns {
            let speaker = numbers[turn.speaker]!
            if let at = lastOf[speaker], turn.start <= joined[at].endMs {
                let last = joined[at]
                joined[at] = Turn(startMs: last.startMs, endMs: max(last.endMs, turn.end), speaker: speaker)
            } else {
                lastOf[speaker] = joined.count
                joined.append(Turn(startMs: turn.start, endMs: turn.end, speaker: speaker))
            }
        }
        joined.sort { ($0.startMs, $0.endMs, $0.speaker) < ($1.startMs, $1.endMs, $1.speaker) }
        return Result(speakers: numbers.count, turns: joined)
    }
}
