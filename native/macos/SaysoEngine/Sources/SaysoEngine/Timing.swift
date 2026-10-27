// Times of words and segments for `transcribe` with `segments: true`
// (docs/engine-protocol.md, "Segments"). The rules are the same as in the Rust engine
// (crates/sayso-engine/src/timing.rs), so both engines cut text the same way.
import Foundation

/// One word and its time, in milliseconds from the start of the audio.
struct TimedWord: Equatable {
    var startMs: Int
    var endMs: Int
    var text: String
}

/// A part of the text with its time.
struct TextSegment: Equatable {
    var startMs: Int
    var endMs: Int
    var text: String
    /// The words, when the model gives word times.
    var words: [TimedWord]?
}

/// The timing that a final pass gives with its text.
enum Timing: Equatable {
    /// The model gives no times.
    case none
    /// Word times (timestamp level `word`).
    case words([TimedWord])
    /// Segments of the model (timestamp level `segment`).
    case segments([TextSegment])
}

/// The text of a final pass and its timing.
struct Timed: Equatable {
    var text: String
    var timing: Timing

    static func untimed(_ text: String) -> Timed { Timed(text: text, timing: .none) }
}

/// One piece of the audio that the final pass transcribed alone.
struct AudioPiece {
    var startMs: Int
    var endMs: Int
    var timed: Timed
}

/// The language that a model found in one piece of the audio, and the length of the piece.
struct LanguageVote: Equatable {
    var code: String
    var samples: Int

    /// The language of the most audio, or nil without votes. When two languages have the
    /// same audio, the one that came first wins.
    static func winner(_ votes: [LanguageVote]) -> String? {
        var total: [String: Int] = [:]
        var order: [String] = []
        for vote in votes where !vote.code.isEmpty {
            if total[vote.code] == nil { order.append(vote.code) }
            total[vote.code, default: 0] += vote.samples
        }
        var best: String?
        for code in order where best == nil || total[code]! > total[best!]! { best = code }
        return best
    }
}

enum Segments {
    /// A segment ends before a pause of this length between two words.
    static let pauseMs = 700
    /// A segment ends before it would pass this length.
    static let maxSegmentMs = 20_000
    /// The end of a token without a duration and without a token after it.
    static let lastTokenMs = 400.0

    /// Milliseconds of a time in seconds. Negative times become 0.
    private static func ms(_ seconds: Double) -> Int { Int((max(0, seconds) * 1000).rounded()) }

    /// Words from the tokens of a model and the start time of each token, in seconds.
    /// A token that starts with a space or `▁` starts a word, and so does each CJK
    /// character. Other tokens (word pieces, punctuation) join the word before them.
    /// `durations` gives the length of each token, when the model has it. `audioMs` is
    /// the length of the audio.
    static func words(tokens: [String], starts: [Double], durations: [Double]?, audioMs: Int) -> [TimedWord] {
        let count = min(tokens.count, starts.count)
        var words: [TimedWord] = []
        // A token that is only a space marks the token after it.
        var spaceBefore = false
        for i in 0..<count {
            let token = tokens[i]
            let start = ms(starts[i])
            var end: Int
            if let durations, i < durations.count, durations[i] > 0 {
                end = ms(starts[i] + durations[i])
            } else if i + 1 < count {
                end = ms(starts[i + 1])
            } else {
                end = ms(starts[i]) + Int(lastTokenMs)
            }
            end = min(max(end, start), max(audioMs, start))
            let marked = spaceBefore || token.hasPrefix(" ") || token.hasPrefix("▁")
            spaceBefore = false
            let piece = token.replacingOccurrences(of: "▁", with: " ")
                .trimmingCharacters(in: .whitespacesAndNewlines)
            if piece.isEmpty {
                spaceBefore = marked || !token.isEmpty
                continue
            }
            let cjk = piece.unicodeScalars.first.map(isCJK) ?? false
            if !marked, !cjk, let last = words.indices.last {
                let afterCJK = words[last].text.unicodeScalars.last.map(isCJK) ?? false
                // Punctuation after a CJK character joins it.
                let punctuation = piece.unicodeScalars.allSatisfy { isASCIIPunctuation($0) || isCJKPunctuation($0) }
                if !afterCJK || punctuation {
                    words[last].text += piece
                    words[last].endMs = max(words[last].endMs, end)
                    continue
                }
            }
            words.append(TimedWord(startMs: start, endMs: end, text: piece))
        }
        return words
    }

    /// Cut words into segments: after a word that ends a sentence, before a pause of
    /// 700 ms or more, and before a segment passes 20 s.
    static func fromWords(_ words: [TimedWord]) -> [TextSegment] {
        var segments: [TextSegment] = []
        var current: [TimedWord] = []
        for word in words {
            if let first = current.first, let last = current.last {
                let pause = word.startMs - last.endMs >= pauseMs
                let tooLong = word.endMs - first.startMs > maxSegmentMs
                if pause || tooLong {
                    segments.append(segment(of: current))
                    current = []
                }
            }
            current.append(word)
            if endsSentence(word.text) {
                segments.append(segment(of: current))
                current = []
            }
        }
        if !current.isEmpty { segments.append(segment(of: current)) }
        return segments
    }

    /// The segments of a whole recording from its pieces. Times move from the start of
    /// each piece to the start of the recording. A piece without times is one segment.
    static func of(_ pieces: [AudioPiece]) -> [TextSegment] {
        var all: [TextSegment] = []
        for piece in pieces {
            let shift = { (ms: Int) in min(ms + piece.startMs, piece.endMs) }
            switch piece.timed.timing {
            case .words(let words):
                for var segment in fromWords(words) {
                    segment.startMs = shift(segment.startMs)
                    segment.endMs = shift(segment.endMs)
                    segment.words = segment.words?.map {
                        TimedWord(startMs: shift($0.startMs), endMs: shift($0.endMs), text: $0.text)
                    }
                    all.append(segment)
                }
            case .segments(let segments):
                all += segments.map {
                    TextSegment(
                        startMs: shift($0.startMs), endMs: shift($0.endMs),
                        text: $0.text.trimmingCharacters(in: .whitespacesAndNewlines), words: nil)
                }
            case .none:
                all.append(TextSegment(
                    startMs: piece.startMs, endMs: piece.endMs,
                    text: piece.timed.text.trimmingCharacters(in: .whitespacesAndNewlines), words: nil))
            }
        }
        all.removeAll { $0.text.isEmpty }
        // In order and without overlaps, also when a model gives a time twice.
        var lastEnd = 0
        for i in all.indices {
            all[i].startMs = max(all[i].startMs, lastEnd)
            all[i].endMs = max(all[i].endMs, all[i].startMs)
            lastEnd = all[i].endMs
        }
        return all
    }

    /// Words with the text of a final pass that changed some words (dictionary words).
    /// The times stay. Nil when the number of words changed, because then the times of
    /// the new words are not known.
    static func retext(_ words: [TimedWord], to text: String) -> [TimedWord]? {
        let parts = text.split(whereSeparator: { $0.isWhitespace }).map(String.init)
        guard parts.count == words.count else { return nil }
        return zip(words, parts).map { TimedWord(startMs: $0.startMs, endMs: $0.endMs, text: $1) }
    }

    /// The `segments` field of the `final` reply.
    static func json(_ segments: [TextSegment]) -> [[String: Any]] {
        segments.map { segment in
            var value: [String: Any] = ["start_ms": segment.startMs, "end_ms": segment.endMs, "text": segment.text]
            if let words = segment.words {
                value["words"] = words.map { ["start_ms": $0.startMs, "end_ms": $0.endMs, "text": $0.text] }
            }
            return value
        }
    }

    private static func segment(of words: [TimedWord]) -> TextSegment {
        TextSegment(
            startMs: words.first?.startMs ?? 0, endMs: words.last?.endMs ?? 0, text: join(words), words: words)
    }

    /// The text of words: a space between two words, except between CJK text.
    private static func join(_ words: [TimedWord]) -> String {
        var text = ""
        for word in words {
            let cjkJoin = (text.unicodeScalars.last.map { isCJK($0) || isCJKPunctuation($0) } ?? false)
                && (word.text.unicodeScalars.first.map(isCJK) ?? false)
            if !text.isEmpty && !cjkJoin { text += " " }
            text += word.text
        }
        return text
    }

    static func endsSentence(_ text: String) -> Bool {
        let closing: Set<Character> = ["\"", "'", ")", "\u{201D}", "\u{2019}", "\u{300D}", "\u{300F}"]
        var trimmed = Substring(text)
        while let last = trimmed.last, closing.contains(last) { trimmed = trimmed.dropLast() }
        guard let last = trimmed.last else { return false }
        return [".", "?", "!", "\u{3002}", "\u{FF1F}", "\u{FF01}"].contains(last)
    }

    private static func isCJK(_ c: Unicode.Scalar) -> Bool {
        switch c.value {
        case 0x3040...0x30FF,  // Hiragana, Katakana
            0x3400...0x4DBF,  // CJK Extension A
            0x4E00...0x9FFF,  // CJK Unified Ideographs
            0xAC00...0xD7AF,  // Hangul syllables
            0xF900...0xFAFF:  // CJK Compatibility Ideographs
            return true
        default:
            return false
        }
    }

    private static func isCJKPunctuation(_ c: Unicode.Scalar) -> Bool {
        (0x3000...0x303F).contains(c.value) || (0xFF00...0xFFEF).contains(c.value)
    }

    private static func isASCIIPunctuation(_ c: Unicode.Scalar) -> Bool {
        switch c.value {
        case 0x21...0x2F, 0x3A...0x40, 0x5B...0x60, 0x7B...0x7E: return true
        default: return false
        }
    }
}
