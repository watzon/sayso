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

    private static func transcriber(_ locale: Locale) -> SpeechTranscriber {
        SpeechTranscriber(locale: locale, transcriptionOptions: [], reportingOptions: [], attributeOptions: [])
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
    /// does not have them yet.
    static func transcribe(url: URL, language: String, vocabulary: [String]) async throws -> String {
        try await install(language: language) { _ in }
        guard let locale = await locale(for: language) else {
            throw EngineError("Apple Speech does not support the language \(language)")
        }
        let transcriber = transcriber(locale)
        let analyzer = SpeechAnalyzer(modules: [transcriber])
        if !vocabulary.isEmpty {
            let context = AnalysisContext()
            context.contextualStrings[.general] = vocabulary
            try await analyzer.setContext(context)
        }
        let file = try AVAudioFile(forReading: url)
        async let text = transcriber.results.reduce(into: "") { $0 += String($1.text.characters) }
        if let last = try await analyzer.analyzeSequence(from: file) {
            try await analyzer.finalizeAndFinish(through: last)
        } else {
            await analyzer.cancelAndFinishNow()
        }
        return try await text
    }
}
