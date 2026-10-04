// Vocabulary boosting for the batch Parakeet pass (CTC 110M keyword spotting plus rescoring).
import FluidAudio
import Foundation

/// The same pipeline as FluidAudio's `VocabularyBoostingSession` (spot terms with the CTC
/// model, then rescore the transcript against the CTC log-probabilities).
///
/// Why not `UnifiedAsrManager.configureVocabularyBoosting`? In FluidAudio 0.17.5 its session
/// reads `tokenizer.json` from `~/Library/Application Support/FluidAudio/Models/...`, which
/// is not under `SAYSO_MODELS_DIR`. Every public piece of the session is available, so this
/// type wires them together with the CTC folder under our models directory.
struct VocabularyBooster {
    let terms: [String]
    private let spotter: CtcKeywordSpotter
    private let rescorer: VocabularyRescorer
    private let vocabulary: CustomVocabularyContext
    private let sizeConfig: ContextBiasingConstants.VocabSizeConfig

    /// The lowest spelling similarity for a replacement. FluidAudio uses 0.50 for a short
    /// dictionary, which lets "window" become "Pindrop". Above 0.60 true matches are lost.
    static let minSimilarity: Float = 0.60

    /// CTC models and tokenizer, loaded once from the models directory. No network.
    struct CtcAssets {
        let models: CtcModels
        let tokenizer: CtcTokenizer
        let directory: URL

        static func load(from directory: URL) async throws -> CtcAssets {
            let models = try await CtcModels.loadDirect(from: directory, variant: .ctc110m)
            let tokenizer = try await CtcTokenizer.load(from: directory)
            return CtcAssets(models: models, tokenizer: tokenizer, directory: directory)
        }
    }

    /// Build a booster for a list of words. Words that produce no CTC tokens are dropped.
    init(words: [String], assets: CtcAssets) async throws {
        var seen = Set<String>()
        let unique = words.map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty && seen.insert($0.lowercased()).inserted }
        let tokenized = unique.compactMap { word -> CustomVocabularyTerm? in
            let ids = assets.tokenizer.encode(word)
            return ids.isEmpty ? nil : CustomVocabularyTerm(text: word, ctcTokenIds: ids)
        }
        guard !tokenized.isEmpty else { throw EngineError("vocabulary has no usable terms") }
        let context = CustomVocabularyContext(terms: tokenized)

        self.terms = unique
        self.vocabulary = context
        self.sizeConfig = ContextBiasingConstants.rescorerConfig(forVocabSize: tokenized.count)
        self.spotter = CtcKeywordSpotter(
            models: assets.models, blankId: assets.models.vocabulary.count)
        self.rescorer = try await VocabularyRescorer.create(
            spotter: spotter,
            vocabulary: context,
            // No acoustic rescue. That pass swaps a word for a term on the CTC score alone,
            // with no spelling check that holds, and with a short dictionary it fires on
            // ordinary words ("macOS" became "MayFirmOS", "distros" became "Pindrop").
            // The similarity pass keeps the true matches ("say so" to "Sayso").
            config: VocabularyRescorer.Config(spotterRescueEnabled: false),
            ctcModelDirectory: assets.directory
        )
    }

    /// Apply the rescorer's replacements to the original transcript.
    ///
    /// The rescorer's own `output.text` is rebuilt from word timings: it drops punctuation
    /// and can swallow neighbours. For example it turns "the GitHub Actions workflow" into
    /// "GitHub Actions workflow" and "for every pull request." into "for pull request".
    /// Applying each replacement to the original text avoids that.
    static func apply(_ replacements: [VocabularyRescorer.RescoringResult], to text: String) -> String {
        var result = text
        var cursor = 0  // Character offset. Replacements arrive in transcript order.
        for item in replacements where item.shouldReplace {
            guard let word = item.replacementWord, !word.isEmpty else { continue }
            let from = result.index(result.startIndex, offsetBy: cursor)
            guard let range = result.range(of: item.originalWord, range: from..<result.endIndex) else {
                continue
            }
            let merged = merge(original: String(result[range]), replacement: word)
            let start = result.distance(from: result.startIndex, to: range.lowerBound)
            result.replaceSubrange(range, with: merged)
            cursor = start + merged.count
        }
        return result
    }

    /// Replace a matched span with a vocabulary term. Keeps words the term already contains
    /// (such as a leading "the") and keeps the punctuation at the end of the span.
    static func merge(original: String, replacement: String) -> String {
        func core(_ token: String) -> String {
            token.lowercased().filter { $0.isLetter || $0.isNumber }
        }
        func edge(_ token: String, leading: Bool) -> String {
            let chars = leading ? Array(token) : Array(token.reversed())
            let punct = String(chars.prefix { !($0.isLetter || $0.isNumber) })
            return leading ? punct : String(punct.reversed())
        }
        let tokens = original.split(separator: " ", omittingEmptySubsequences: true).map(String.init)
        let words = replacement.split(separator: " ").map(String.init)
        guard !tokens.isEmpty, !words.isEmpty else { return replacement }

        // The term sits inside the span: keep the words around it and swap only the term.
        let wanted = words.map(core)
        if tokens.count >= words.count {
            for i in 0...(tokens.count - words.count)
            where Array(tokens[i..<(i + words.count)]).map(core) == wanted {
                var out = Array(tokens[..<i])
                var term = words
                term[0] = edge(tokens[i], leading: true) + term[0]
                term[term.count - 1] += edge(tokens[i + words.count - 1], leading: false)
                out += term
                out += Array(tokens[(i + words.count)...])
                return out.joined(separator: " ")
            }
        }
        // The whole span was misheard: replace it and keep its closing punctuation.
        return replacement + edge(tokens[tokens.count - 1], leading: false)
    }

    /// Return the transcript with vocabulary terms substituted where the audio supports it.
    /// Never throws: a CTC failure keeps the original text.
    func rescore(text: String, tokenTimings: [TokenTiming], samples: [Float]) async -> String {
        guard !tokenTimings.isEmpty, !samples.isEmpty else { return text }
        do {
            let spot = try await spotter.spotKeywordsWithLogProbs(
                audioSamples: samples, customVocabulary: vocabulary, minScore: nil)
            guard !spot.logProbs.isEmpty else { return text }
            let output = rescorer.ctcTokenRescore(
                transcript: text,
                tokenTimings: tokenTimings,
                logProbs: spot.logProbs,
                frameDuration: spot.frameDuration,
                cbw: sizeConfig.cbw,
                marginSeconds: 0.5,
                minSimilarity: max(sizeConfig.minSimilarity, vocabulary.minSimilarity, Self.minSimilarity)
            )
            return output.wasModified ? Self.apply(output.replacements, to: text) : text
        } catch {
            return text
        }
    }
}
