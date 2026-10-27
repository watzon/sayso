import XCTest

@testable import SaysoEngine

final class DetectedLanguageTests: XCTestCase {
    func testLanguageOfMostAudioWins() {
        let votes = [
            LanguageVote(code: "en", samples: 10_000), LanguageVote(code: "de", samples: 30_000),
            LanguageVote(code: "en", samples: 15_000),
        ]
        XCTAssertEqual(LanguageVote.winner(votes), "de")
        XCTAssertEqual(LanguageVote.winner(votes + [LanguageVote(code: "en", samples: 10_000)]), "en")
    }

    func testFirstLanguageWinsATieAndNoVotesGiveNil() {
        XCTAssertEqual(
            LanguageVote.winner([LanguageVote(code: "ja", samples: 5), LanguageVote(code: "ko", samples: 5)]), "ja")
        XCTAssertNil(LanguageVote.winner([]))
        XCTAssertNil(LanguageVote.winner([LanguageVote(code: "", samples: 5)]))
    }

    func testSenseVoiceLanguageComesFromTheFirstTag() {
        XCTAssertNil(SenseVoice.languageTag(in: "<|de|><|NEUTRAL|><|Speech|><|withitn|>Hi"))
        XCTAssertEqual(SenseVoice.languageTag(in: "<|yue|><|NEUTRAL|><|Speech|><|withitn|>Hi"), "yue")
        XCTAssertEqual(SenseVoice.languageTag(in: "<|en|><|NEUTRAL|>Hi"), "en")
        XCTAssertNil(SenseVoice.languageTag(in: "<|nospeech|><|Event_UNK|>"))
        XCTAssertNil(SenseVoice.languageTag(in: "Hi"))
    }
}
