import XCTest

@testable import SaysoEngine

final class DiarizationTests: XCTestCase {
    private func raw(_ start: Double, _ end: Double, _ speaker: String) -> Diarization.RawTurn {
        Diarization.RawTurn(start: start, end: end, speaker: speaker)
    }

    private func turn(_ start: Int, _ end: Int, _ speaker: Int) -> Diarization.Turn {
        Diarization.Turn(startMs: start, endMs: end, speaker: speaker)
    }

    func testSpeakersAreNumberedInTheOrderTheyFirstSpeak() {
        let found = Diarization.finish(
            [raw(5, 6, "S1"), raw(1, 2, "S3"), raw(3, 4, "S2"), raw(7, 8, "S3")], audioMs: 10_000)
        XCTAssertEqual(found.turns.map(\.speaker), [0, 1, 2, 0], "S3 spoke first, then S2, then S1")
        XCTAssertEqual(found.speakers, 3)
        XCTAssertEqual(found.turns.map(\.startMs), [1000, 3000, 5000, 7000])
    }

    func testTurnsOfOneSpeakerThatOverlapOrTouchBecomeOne() {
        let found = Diarization.finish(
            [raw(0, 2, "A"), raw(1.5, 3, "A"), raw(3, 3.5, "A"), raw(4, 5, "A"), raw(2, 2.5, "B")], audioMs: 10_000)
        XCTAssertEqual(
            found.turns, [turn(0, 3500, 0), turn(2000, 2500, 1), turn(4000, 5000, 0)],
            "two speakers may overlap each other; one speaker may not overlap itself")
        XCTAssertEqual(found.speakers, 2)
    }

    func testATurnInsideALongerTurnOfTheSameSpeakerDoesNotShortenIt() {
        let found = Diarization.finish([raw(0, 5, "A"), raw(1, 2, "A")], audioMs: 10_000)
        XCTAssertEqual(found.turns, [turn(0, 5000, 0)])
    }

    func testTimesAreWholeMillisecondsInsideTheAudioAndEmptyTurnsGo() {
        let found = Diarization.finish(
            [raw(0.0004, 1.2346, "A"), raw(2, 2, "B"), raw(9.5, 12, "A"), raw(-1, 0.5, "A")], audioMs: 10_000)
        XCTAssertEqual(found.turns.map { [$0.startMs, $0.endMs] }, [[0, 1235], [9500, 10_000]])
        XCTAssertEqual(found.speakers, 1, "a speaker with only an empty turn does not count")
    }

    func testNoTurnsMeansNoSpeakers() throws {
        let found = Diarization.finish([], audioMs: 10_000)
        XCTAssertEqual(found, Diarization.Result(speakers: 0, turns: []))
        let json = try JSONSerialization.data(withJSONObject: found.json, options: [.sortedKeys])
        XCTAssertEqual(String(decoding: json, as: UTF8.self), #"{"speakers":0,"turns":[]}"#)
    }

    func testSpeakersIsAbsentNullOrAnIntegerOfOneOrMore() throws {
        func speakers(_ value: String) throws -> Int? {
            try Request(line: #"{"v":1,"type":"diarize","id":"d","speakers":\#(value)}"#).optionalCount("speakers")
        }
        XCTAssertNil(try Request(line: #"{"v":1,"type":"diarize","id":"d"}"#).optionalCount("speakers"))
        XCTAssertNil(try speakers("null"))
        XCTAssertEqual(try speakers("1"), 1)
        XCTAssertEqual(try speakers("4"), 4)
        for bad in ["0", "-2", "2.5", "true", #""2""#, "[]"] {
            XCTAssertThrowsError(try speakers(bad), "speakers \(bad)")
        }
    }
}
