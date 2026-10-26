// Apple's on-device speech model (SpeechAnalyzer, macOS 26 and later). macOS downloads
// and stores its assets, one set per language, outside SAYSO_MODELS_DIR.
import AVFoundation
import Foundation
import Speech

@available(macOS 26, *)
enum AppleSpeech {
    /// The supported locale for a language code such as "de", or for "auto" (the language
    /// of this Mac). The region of this Mac wins when the language has several regions.
    static func locale(for language: String) async -> Locale? {
        let supported = await SpeechTranscriber.supportedLocales
        let current = Locale.current
        let wanted = language.isEmpty || language == "auto" ? current.language.languageCode?.identifier ?? "en" : language
        let matches = supported.filter { $0.language.languageCode?.identifier == wanted }
        return matches.first { $0.region == current.region }
            ?? matches.sorted { $0.identifier < $1.identifier }.first
            ?? (language == "auto" ? supported.first { $0.identifier(.bcp47) == "en-US" } : nil)
    }

    /// True when macOS has the assets of at least one language.
    static func anyInstalled() async -> Bool {
        !(await SpeechTranscriber.installedLocales).isEmpty
    }

    /// With `timed`, the results also give the time of each word.
    private static func transcriber(_ locale: Locale, timed: Bool = false) -> SpeechTranscriber {
        SpeechTranscriber(
            locale: locale, transcriptionOptions: [], reportingOptions: [],
            attributeOptions: timed ? [.audioTimeRange] : [])
    }

    /// Ask macOS to download the assets of a language. Returns at once when they are present.
    static func install(language: String, progress: @escaping @Sendable (Double) -> Void) async throws {
        guard let locale = await locale(for: language) else {
            throw EngineError("Apple Speech does not support the language \(language)")
        }
        guard let request = try await AssetInventory.assetInstallationRequest(supporting: [transcriber(locale)])
        else { return }
        let observation = request.progress.observe(\.fractionCompleted) { value, _ in
            progress(value.fractionCompleted)
        }
        defer { observation.invalidate() }
        try await request.downloadAndInstall()
    }

    /// Final pass on an audio file. Downloads the assets of the language first when macOS
    /// does not have them yet. With `timed`, the text comes with word times.
    static func transcribe(
        url: URL, language: String, vocabulary: [String], timed: Bool, audioMs: Int
    ) async throws -> Timed {
        try await install(language: language) { _ in }
        guard let locale = await locale(for: language) else {
            throw EngineError("Apple Speech does not support the language \(language)")
        }
        let transcriber = transcriber(locale, timed: timed)
        let analyzer = SpeechAnalyzer(modules: [transcriber])
        if !vocabulary.isEmpty {
            let context = AnalysisContext()
            context.contextualStrings[.general] = vocabulary
            try await analyzer.setContext(context)
        }
        let file = try AVAudioFile(forReading: url)
        async let results = transcriber.results.reduce(into: [AttributedString]()) { $0.append($1.text) }
        if let last = try await analyzer.analyzeSequence(from: file) {
            try await analyzer.finalizeAndFinish(through: last)
        } else {
            await analyzer.cancelAndFinishNow()
        }
        let parts = try await results
        let text = parts.reduce(into: "") { $0 += String($1.characters) }
        guard timed else { return .untimed(text) }
        // Each run of a result with a time is a word or a part of a word. Text without a
        // time (the spaces) goes in front of the next word, so a space still starts a word.
        var tokens: [String] = []
        var starts: [Double] = []
        var durations: [Double] = []
        for part in parts {
            var before = " "
            for run in part.runs {
                let piece = String(part[run.range].characters)
                guard let range = run.audioTimeRange else {
                    before += piece
                    continue
                }
                tokens.append(before + piece)
                starts.append(range.start.seconds)
                durations.append(range.duration.seconds)
                before = ""
            }
        }
        let words = Segments.words(tokens: tokens, starts: starts, durations: durations, audioMs: audioMs)
        return Timed(text: text, timing: words.isEmpty ? .none : .words(words))
    }
}
