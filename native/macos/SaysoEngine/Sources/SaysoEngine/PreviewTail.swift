// The just-heard tail of a live preview.
import Foundation

/// Splits the text of a live preview into committed words and the just-heard tail.
///
/// The streaming decoders only append: a word that they output does not change
/// during the preview. So the tail is not text that can change. It is the words
/// heard in the last `window` seconds, which the overlay shows in pencil italic.
/// The words of the newest update are always in the tail, so a decoder with long
/// chunks (Nemotron, 1.12 s) still shows one. The final pass can change any word.
struct PreviewTail {
    /// How long a word stays in the tail.
    static let window: TimeInterval = 1.0

    private var text: [Character] = []
    /// Where each update started its new words, and when. Oldest first.
    private var marks: [(start: Int, time: TimeInterval)] = []
    private var sent = (committed: "", tentative: "")

    /// Take the full preview text at `time` (seconds). Returns the split to send, or nil
    /// when it is the same as the last one sent. The same text again can still return a
    /// split, because older words leave the tail.
    mutating func update(_ value: String, at time: TimeInterval) -> (committed: String, tentative: String)? {
        let current = Array(value)
        if current != text {
            var start = zip(text, current).prefix { $0 == $1 }.count
            while start > 0 && !Self.isBoundary(current, start) { start -= 1 }
            // A decoder that changed earlier text moves the start back. Marks after it are gone.
            marks.removeAll { $0.start >= start }
            marks.append((start, time))
            text = current
        }
        while marks.count > 1 && time - marks[0].time > Self.window { marks.removeFirst() }
        let start = marks.first?.start ?? text.count
        let split = (
            committed: String(String(text[..<start]).trimmingSuffix()),
            tentative: String(text[start...]).trimmingCharacters(in: .whitespacesAndNewlines)
        )
        return send(split)
    }

    /// The last text of the stream, all committed. Nil when it is the same as the last one sent.
    mutating func finish(_ value: String) -> String? {
        text = Array(value)
        marks.removeAll()
        return send((value, ""))?.committed
    }

    private mutating func send(_ split: (committed: String, tentative: String)) -> (committed: String, tentative: String)? {
        guard split != sent else { return nil }
        sent = split
        return split
    }

    /// True when a split before `chars[i]` does not cut a word.
    static func isBoundary(_ chars: [Character], _ i: Int) -> Bool {
        if i == 0 || i == chars.count { return true }
        let (before, after) = (chars[i - 1], chars[i])
        return before.isWhitespace || after.isWhitespace || isUnspaced(before) || isUnspaced(after)
    }

    /// True for a character of a script that has no spaces between words (Chinese,
    /// Japanese, Thai, Lao, Myanmar, Khmer). Any point next to one is a word boundary.
    /// Keep the same ranges as `IsUnspaced` in `internal/bridge/protocol.go`.
    static func isUnspaced(_ c: Character) -> Bool {
        guard let scalar = c.unicodeScalars.first?.value else { return false }
        switch scalar {
        case 0x0E00...0x0EFF, 0x1000...0x109F, 0x1780...0x17FF, 0x3000...0x30FF, 0x3400...0x4DBF,
            0x4E00...0x9FFF, 0xF900...0xFAFF, 0xFF00...0xFFEF:
            return true
        default:
            return false
        }
    }
}

private extension String {
    func trimmingSuffix() -> Substring {
        var end = endIndex
        while end > startIndex, self[index(before: end)].isWhitespace { end = index(before: end) }
        return self[..<end]
    }
}
