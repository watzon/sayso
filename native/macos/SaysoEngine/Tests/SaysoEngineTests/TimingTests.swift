import XCTest

@testable import SaysoEngine

final class TimingTests: XCTestCase {
    private func word(_ start: Int, _ end: Int, _ text: String) -> TimedWord {
        TimedWord(startMs: start, endMs: end, text: text)
    }

    func testWordPiecesAndPunctuationJoinTheirWord() {
        let tokens = [" Hel", "lo", " there", ".", " How", " are", " you", "?"]
        let starts = [0.32, 0.48, 0.70, 1.10, 1.90, 2.30, 2.60, 2.90]
        let words = Segments.words(tokens: tokens, starts: starts, durations: nil, audioMs: 3_500)
        XCTAssertEqual(words.map(\.text), ["Hello", "there.", "How", "are", "you?"])
        XCTAssertEqual(words[0].startMs, 320)
        XCTAssertEqual(words[0].endMs, 700)
        XCTAssertEqual(words[1].endMs, 1_900)
        XCTAssertEqual(words[4].endMs, 3_300, "the last token gets 400 ms")
    }

    func testSentencePieceMarksAndDurationsWork() {
        let words = Segments.words(tokens: ["▁Hi", "▁all"], starts: [0.1, 0.5], durations: [0.2, 0.3], audioMs: 2_000)
        XCTAssertEqual(words, [word(100, 300, "Hi"), word(500, 800, "all")])
    }

    func testALoneSpaceTokenStartsTheNextWord() {
        let words = Segments.words(tokens: ["▁", "Hi", "▁", "all"], starts: [0, 0.1, 0.3, 0.4], durations: nil, audioMs: 1_000)
        XCTAssertEqual(words.map(\.text), ["Hi", "all"])
    }

    func testTimesStayInsideTheAudio() {
        // A token in the silence before the audio, and a last token that passes the end.
        let words = Segments.words(tokens: [" a", " b"], starts: [-0.5, 0.9], durations: nil, audioMs: 1_000)
        XCTAssertEqual(words, [word(0, 900, "a"), word(900, 1_000, "b")])
    }

    func testEachCJKCharacterIsAWordAndJoinsWithoutSpaces() {
        let words = Segments.words(tokens: ["你", "好", "。", "再", "见"], starts: [0, 0.2, 0.4, 0.5, 0.7], durations: nil, audioMs: 1_000)
        XCTAssertEqual(words.map(\.text), ["你", "好。", "再", "见"])
        XCTAssertEqual(Segments.fromWords(words).map(\.text), ["你好。", "再见"])
    }

    func testSegmentsEndAtSentencesPausesAndTwentySeconds() {
        let words = [
            word(0, 300, "One."),
            word(400, 700, "two"),
            word(1_500, 1_800, "three"),
            word(1_900, 21_600, "long"),
            word(21_700, 22_000, "four"),
        ]
        XCTAssertEqual(Segments.fromWords(words).map(\.text), ["One.", "two", "three", "long", "four"])
    }

    func testAPauseBelow700MsKeepsTheSegment() {
        let segments = Segments.fromWords([word(0, 300, "a"), word(999, 1_200, "b"), word(1_900, 2_000, "c")])
        XCTAssertEqual(segments.map(\.text), ["a b", "c"])
        XCTAssertEqual(segments[0].startMs, 0)
        XCTAssertEqual(segments[0].endMs, 1_200)
        XCTAssertEqual(segments[0].words?.count, 2)
    }

    func testClosingQuotesAfterTheEndOfASentence() {
        XCTAssertTrue(Segments.endsSentence("said.\""))
        XCTAssertTrue(Segments.endsSentence("(really?)"))
        XCTAssertTrue(Segments.endsSentence("終わり。」"))
        XCTAssertFalse(Segments.endsSentence("e.g"))
        XCTAssertFalse(Segments.endsSentence("\""))
    }

    func testPiecesMoveToTheTimeOfTheRecording() {
        let pieces = [
            AudioPiece(startMs: 0, endMs: 10_000, timed: Timed(text: "a b.", timing: .words([word(100, 200, "a"), word(300, 400, "b.")]))),
            AudioPiece(startMs: 10_000, endMs: 20_000, timed: .untimed("rest")),
            AudioPiece(startMs: 20_000, endMs: 25_000, timed: .untimed("  ")),
            AudioPiece(
                startMs: 25_000, endMs: 30_000,
                timed: Timed(text: "x", timing: .segments([TextSegment(startMs: 1_000, endMs: 9_000, text: " x ", words: nil)]))),
        ]
        let all = Segments.of(pieces)
        XCTAssertEqual(all.count, 3, "the empty piece is left out")
        XCTAssertEqual(all[0], TextSegment(startMs: 100, endMs: 400, text: "a b.", words: [word(100, 200, "a"), word(300, 400, "b.")]))
        XCTAssertEqual(all[1], TextSegment(startMs: 10_000, endMs: 20_000, text: "rest", words: nil))
        XCTAssertEqual(all[2], TextSegment(startMs: 26_000, endMs: 30_000, text: "x", words: nil), "clamped to the piece")
        let json = Segments.json(all)
        XCTAssertEqual((json[0]["words"] as? [[String: Any]])?[1]["start_ms"] as? Int, 300)
        XCTAssertNil(json[1]["words"])
    }

    func testSegmentsNeverOverlap() {
        let timed = Timed(
            text: "a. b.",
            timing: .segments([
                TextSegment(startMs: 0, endMs: 2_000, text: "a.", words: nil),
                TextSegment(startMs: 1_500, endMs: 1_800, text: "b.", words: nil),
            ]))
        let all = Segments.of([AudioPiece(startMs: 0, endMs: 5_000, timed: timed)])
        XCTAssertEqual(all[1].startMs, 2_000)
        XCTAssertEqual(all[1].endMs, 2_000)
    }

    func testANewTextKeepsTheTimesOnlyWhenTheWordsMatch() {
        let words = [word(0, 100, "say"), word(100, 300, "so,"), word(400, 500, "hi")]
        XCTAssertEqual(Segments.retext(words, to: "say So, hi")?.map(\.text), ["say", "So,", "hi"])
        XCTAssertEqual(Segments.retext(words, to: "say So, hi")?.map(\.endMs), [100, 300, 500])
        XCTAssertNil(Segments.retext(words, to: "Sayso, hi"))
    }

    func testChunkedPassesGiveOneSegmentForEachPiece() async throws {
        // 30 s of audio in pieces of at most 20 s: the second piece starts where the first ends.
        let samples = [Float](repeating: 0.1, count: 30 * AudioChunks.sampleRate)
        var calls = 0
        let result = try await AudioChunks.transcribe(samples, maxSeconds: 20) { chunk in
            calls += 1
            return calls == 1 ? "first" : "second"
        }
        XCTAssertEqual(result.text, "first second")
        let segments = Segments.of(result.pieces)
        XCTAssertEqual(segments.map(\.text), ["first", "second"])
        XCTAssertEqual(segments.first?.startMs, 0)
        XCTAssertEqual(segments.first?.endMs, segments.last?.startMs)
        XCTAssertEqual(segments.last?.endMs, 30_000)
    }
}
