// Voice activity detection (docs/engine-protocol.md, "VAD").
//
// The Silero VAD of FluidAudio gives one speech probability for each window of 4096
// samples (256 ms). `SpeechTracker` turns the probabilities into the parts of the protocol,
// and it holds the rules of the protocol. The `vad` request and a VAD session both use a
// `VadStream`, so the same audio gives the same parts.
//
// The rules, and what FluidAudio 0.17.5 does for them:
// - Speech under 250 ms is left out. `segmentSpeech` has `minSpeechDuration` (0.15 s by
//   default), but `processStreamingChunk` has no minimum at all.
// - A pause under 500 ms does not end speech. Both use `minSilenceDuration` (0.75 s by default).
// - A part is at most 30 s. `segmentSpeech` splits at `maxSpeechDuration` at the longest
//   pause it saw, so its parts have gaps between them, and `processStreamingChunk` does not
//   split at all.
// So the two APIs of FluidAudio give different parts for the same audio, and neither follows
// all three rules. The engine takes only the probability of each window from FluidAudio and
// applies the rules itself.
import FluidAudio
import Foundation

/// The rules of VAD, in samples at 16 kHz.
enum VadRules {
    static let sampleRate = 16_000
    /// Speech under this is left out (250 ms).
    static let minSpeech = 4_000
    /// A pause under this does not end speech (500 ms).
    static let minSilence = 8_000
    /// The longest part (30 s). Longer speech is cut into parts of this length.
    static let maxPart = 480_000

    static func ms(_ samples: Int) -> Int { samples * 1000 / sampleRate }
}

/// An event of a VAD session. The times are in milliseconds from the first audio.
enum SpeechEvent: Equatable {
    case start(startMs: Int)
    case end(startMs: Int, endMs: Int)

    /// The parts with speech, from the events of a whole audio.
    static func parts(_ events: [SpeechEvent]) -> [(startMs: Int, endMs: Int)] {
        events.compactMap { event in
            if case .end(let start, let end) = event { return (start, end) }
            return nil
        }
    }
}

/// Turns the speech probability of each window into speech parts, with the rules of `VadRules`.
///
/// Speech starts at the first window with a probability of `threshold` or more. A pause
/// starts at a window under `negativeThreshold`, and a window at `threshold` or more ends
/// it. Between the two thresholds nothing changes, as in Silero. Positions are in samples
/// from the first window.
struct SpeechTracker {
    let window: Int
    let threshold: Float
    let negativeThreshold: Float

    /// The samples of all windows, with the silence that fills the last window.
    private var heard = 0
    /// The samples of the caller.
    private var given = 0
    /// The start of the speech that is open.
    private var start: Int?
    /// True when `speech_start` of the open speech went out.
    private var reported = false
    /// Where the pause in the open speech began.
    private var silence: Int?
    /// The end of the last part.
    private var lastEnd = 0

    init(window: Int, threshold: Float, negativeThreshold: Float) {
        self.window = window
        self.threshold = threshold
        self.negativeThreshold = negativeThreshold
    }

    /// Hear one window. `real` is the number of samples of the caller in it, less than
    /// `window` only for the last window of the audio.
    mutating func accept(_ probability: Float, real: Int? = nil) -> [SpeechEvent] {
        let windowStart = heard
        heard += window
        given += min(real ?? window, window)
        var events: [SpeechEvent] = []
        if probability >= threshold {
            silence = nil
            if start == nil { start = max(windowStart, lastEnd) }
        } else if start != nil {
            if silence == nil, probability < negativeThreshold { silence = windowStart }
            if let pause = silence, heard - pause >= VadRules.minSilence {
                close(at: pause, &events)
                return events
            }
        }
        if let open = start, !reported, silence == nil, min(heard, given) - open >= VadRules.minSpeech {
            reported = true
            events.append(.start(startMs: VadRules.ms(open)))
        }
        cutLongPart(&events)
        return events
    }

    /// No more audio comes. Ends the open speech where its pause began, or at the end of the audio.
    mutating func finish() -> [SpeechEvent] {
        var events: [SpeechEvent] = []
        close(at: silence ?? given, &events)
        return events
    }

    /// End the open speech at `end`. Speech under the shortest speech is left out. A part
    /// over 30 s is cut, so that no part is longer and the last one is not too short.
    private mutating func close(at end: Int, _ events: inout [SpeechEvent]) {
        guard let open = start else { return }
        let end = max(open, min(end, given))
        let wasReported = reported
        start = nil
        silence = nil
        reported = false
        if !wasReported {
            guard end - open >= VadRules.minSpeech else { return }
            events.append(.start(startMs: VadRules.ms(open)))
        }
        var from = open
        while end - from > VadRules.maxPart {
            let cut = min(from + VadRules.maxPart, end - VadRules.minSpeech)
            events.append(.end(startMs: VadRules.ms(from), endMs: VadRules.ms(cut)))
            events.append(.start(startMs: VadRules.ms(cut)))
            from = cut
        }
        events.append(.end(startMs: VadRules.ms(from), endMs: VadRules.ms(end)))
        lastEnd = end
    }

    /// Cut the open speech when it is 30 s long. The next part starts at the cut.
    ///
    /// The cut waits until the speech has gone on a pause and a shortest speech after the
    /// cut point. A pause that is open then began after the cut point plus the shortest
    /// speech, so the next part is not shorter than the shortest speech.
    private mutating func cutLongPart(_ events: inout [SpeechEvent]) {
        while reported, let open = start, given >= open + VadRules.maxPart + VadRules.minSilence + VadRules.minSpeech {
            let cut = open + VadRules.maxPart
            events.append(.end(startMs: VadRules.ms(open), endMs: VadRules.ms(cut)))
            events.append(.start(startMs: VadRules.ms(cut)))
            start = cut
            lastEnd = cut
        }
    }
}

/// The VAD of one audio stream: cuts the audio into windows, asks the model for the
/// probability of each window, and gives the events of a `SpeechTracker`.
///
/// The windows start at the first sample, so any chunk size from the client gives the same
/// windows and the same events. One user at a time (the stream queue, or one `vad` request).
final class VadStream: @unchecked Sendable {
    private let score: ([Float]) async throws -> Float
    private var tracker: SpeechTracker
    /// Audio that does not fill a window yet.
    private var pending: [Float] = []

    init(tracker: SpeechTracker, score: @escaping ([Float]) async throws -> Float) {
        self.tracker = tracker
        self.score = score
    }

    /// A stream on the Silero VAD of FluidAudio, with its thresholds.
    static func silero(_ manager: VadManager) async -> VadStream {
        let threshold = await manager.config.defaultThreshold
        let negative = VadSegmentationConfig.default.effectiveNegativeThreshold(baseThreshold: threshold)
        let state = SileroState()
        let tracker = SpeechTracker(window: VadManager.chunkSize, threshold: threshold, negativeThreshold: negative)
        return VadStream(tracker: tracker) { window in
            let result = try await manager.processStreamingChunk(window, state: state.value)
            state.value = result.state
            return result.probability
        }
    }

    /// Hear more audio (16 kHz mono).
    func accept(_ samples: [Float]) async throws -> [SpeechEvent] {
        pending += samples
        var events: [SpeechEvent] = []
        let window = tracker.window
        var offset = 0
        while pending.count - offset >= window {
            let probability = try await score(Array(pending[offset..<(offset + window)]))
            events += tracker.accept(probability)
            offset += window
        }
        pending.removeFirst(offset)
        return events
    }

    /// No more audio comes. The last window is filled with silence. Closes the open speech.
    func finish() async throws -> [SpeechEvent] {
        var events: [SpeechEvent] = []
        if !pending.isEmpty {
            let real = pending.count
            let probability = try await score(pending + [Float](repeating: 0, count: tracker.window - real))
            events += tracker.accept(probability, real: real)
            pending = []
        }
        return events + tracker.finish()
    }
}

/// The recurrent state of Silero between two windows of one stream.
private final class SileroState: @unchecked Sendable {
    var value = VadStreamState.initial()
}
