import XCTest

@testable import SaysoEngine

final class VadTests: XCTestCase {
    /// Audio for the tests: `(milliseconds, loud)` pieces.
    private func audio(_ pieces: [(Int, Bool)]) -> [Float] {
        pieces.flatMap { length, loud in [Float](repeating: loud ? 0.5 : 0, count: length * 16) }
    }

    /// A stream whose model says speech for a loud window: a root mean square above 0.01.
    private func stream(window: Int = 512) -> VadStream {
        VadStream(tracker: SpeechTracker(window: window, threshold: 0.5, negativeThreshold: 0.35)) { samples in
            let power = samples.reduce(0) { $0 + $1 * $1 } / Float(samples.count)
            return power.squareRoot() > 0.01 ? 1 : 0
        }
    }

    /// All events for `samples`, given in chunks of `chunk` samples.
    private func run(_ samples: [Float], chunk: Int = 2560, window: Int = 512) async throws -> [SpeechEvent] {
        let stream = stream(window: window)
        var events: [SpeechEvent] = []
        var offset = 0
        while offset < samples.count {
            let end = min(offset + chunk, samples.count)
            events += try await stream.accept(Array(samples[offset..<end]))
            offset = end
        }
        return events + (try await stream.finish())
    }

    private func parts(_ events: [SpeechEvent]) -> [[Int]] {
        SpeechEvent.parts(events).map { [$0.startMs, $0.endMs] }
    }

    private func assertNear(_ actual: Int, _ expected: Int, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertLessThanOrEqual(abs(actual - expected), 100, "\(actual) is not near \(expected)", file: file, line: line)
    }

    func testSpeechWithALongPauseIsTwoParts() async throws {
        let found = parts(try await run(audio([(1000, false), (2000, true), (1500, false), (1000, true), (1000, false)])))
        XCTAssertEqual(found.count, 2, "\(found)")
        assertNear(found[0][0], 1000)
        assertNear(found[0][1], 3000)
        assertNear(found[1][0], 4500)
        assertNear(found[1][1], 5500)
    }

    func testAShortPauseDoesNotEndSpeechAndShortSpeechIsLeftOut() async throws {
        let samples = audio([(500, false), (1000, true), (300, false), (1000, true), (800, false), (200, true), (1000, false)])
        let found = parts(try await run(samples))
        XCTAssertEqual(found.count, 1, "one part, and the 200 ms are left out: \(found)")
        assertNear(found[0][0], 500)
        assertNear(found[0][1], 2800)
    }

    func testSilenceAndNoiseWithoutSpeechGiveNoPart() async throws {
        let silence = try await run(audio([(3000, false)]))
        let empty = try await run([])
        let click = try await run(audio([(100, false), (100, true), (1000, false)]))
        XCTAssertEqual(silence, [])
        XCTAssertEqual(empty, [])
        XCTAssertEqual(click, [])
    }

    func testEventsDoNotDependOnTheChunkSize() async throws {
        let samples = audio([(700, false), (3000, true), (900, false), (40_000, true), (1000, false), (400, true)])
        let all = try await run(samples, chunk: samples.count)
        XCTAssertFalse(all.isEmpty)
        for chunk in [1, 160, 2560, 3333, 16_000] {
            let events = try await run(samples, chunk: chunk)
            XCTAssertEqual(events, all, "chunk of \(chunk) samples")
        }
    }

    func testLongSpeechIsCutIntoPartsOfAtMost30SecondsThatTouch() async throws {
        let found = parts(try await run(audio([(1000, false), (75_000, true), (1000, false)])))
        XCTAssertEqual(found.count, 3, "\(found)")
        assertNear(found[0][0], 1000)
        XCTAssertEqual(found[0][1] - found[0][0], 30_000)
        XCTAssertEqual(found[1], [found[0][1], found[0][1] + 30_000])
        XCTAssertEqual(found[2][0], found[1][1])
        assertNear(found[2][1], 76_000)
    }

    func testSpeechThatEndsJustAfter30SecondsIsCutAndItsLastPartIsNotTooShort() async throws {
        for length in [30_100, 30_300, 30_600] {
            let found = parts(try await run(audio([(1000, false), (length, true), (1000, false)])))
            XCTAssertEqual(found.count, 2, "\(length) ms: \(found)")
            XCTAssertEqual(found[1][0], found[0][1], "the parts touch")
            for part in found {
                XCTAssertLessThanOrEqual(part[1] - part[0], 30_000, "\(length) ms: \(found)")
                XCTAssertGreaterThanOrEqual(part[1] - part[0], 250, "\(length) ms: \(found)")
            }
        }
    }

    func testASessionReportsEachStartBeforeItsEndAndEveryEndByTheFinish() async throws {
        let samples = audio([(500, false), (2000, true), (1000, false), (1000, true)])
        let stream = stream()
        var events: [SpeechEvent] = []
        var offset = 0
        while offset < samples.count {
            let end = min(offset + 2560, samples.count)
            events += try await stream.accept(Array(samples[offset..<end]))
            offset = end
        }
        // The first part ended, and the second part has begun. Its end waits for a pause.
        XCTAssertEqual(events.count, 3, "\(events)")
        guard case .start = events[0], case .end = events[1], case .start = events[2] else {
            return XCTFail("\(events)")
        }
        let rest = try await stream.finish()
        XCTAssertEqual(rest.count, 1, "\(rest)")
        guard case .end(let startMs, let endMs) = rest.first else { return XCTFail("\(rest)") }
        XCTAssertTrue(endMs > startMs && endMs <= 4500, "the end is not after the audio: \(rest)")
    }

    func testSpeechThatRunsToTheEndOfTheAudioEndsInsideIt() async throws {
        let found = parts(try await run(audio([(500, false), (2000, true)])))
        XCTAssertEqual(found.count, 1)
        XCTAssertEqual(found[0][1], 2500)
    }

    func testEveryEventListIsOrderedAndWithoutOverlap() async throws {
        let samples = audio([(300, true), (900, false), (5000, true), (600, false), (35_000, true), (2000, false), (900, true)])
        var open: Int?
        var lastEnd = 0
        for event in try await run(samples, chunk: 1000) {
            switch event {
            case .start(let startMs):
                XCTAssertTrue(open == nil && startMs >= lastEnd, "\(startMs)")
                open = startMs
            case .end(let startMs, let endMs):
                XCTAssertEqual(open, startMs)
                XCTAssertGreaterThan(endMs, startMs)
                XCTAssertLessThanOrEqual(endMs - startMs, 30_000)
                open = nil
                lastEnd = endMs
            }
        }
        XCTAssertNil(open)
    }

    /// The windows of Silero are 256 ms: one window of speech is long enough, and two
    /// windows of pause end the speech. A probability between the thresholds changes nothing.
    func testTheTrackerUsesBothThresholdsOnWindowsOf256Ms() {
        var tracker = SpeechTracker(window: 4096, threshold: 0.85, negativeThreshold: 0.7)
        var events: [SpeechEvent] = []
        for probability: Float in [0.1, 0.8, 0.9, 0.75, 0.2, 0.9, 0.3, 0.6, 0.1, 0.9] {
            events += tracker.accept(probability)
        }
        events += tracker.finish()
        // Window 1 (0.8) does not start speech. Window 2 starts it, the pause at window 4 is
        // one window, and the pause from window 6 is two windows (0.3, then 0.6 keeps it).
        XCTAssertEqual(
            events,
            [.start(startMs: 512), .end(startMs: 512, endMs: 1536), .start(startMs: 2304), .end(startMs: 2304, endMs: 2560)])
    }
}
