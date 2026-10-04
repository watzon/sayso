// The language model of macOS (Apple Intelligence, macOS 26 and later) for AI enhancement.
// macOS owns the model and its files, so there is no download, load, or unload here.
import Foundation

#if canImport(FoundationModels)
    import FoundationModels

    /// The reply shape. Guided generation binds the model to it.
    @available(macOS 26.0, *)
    @Generable
    struct EditedText {
        @Guide(description: "The edited text")
        var text: String
    }
#endif

/// A failure with a `code` that the client maps to its own error: `unavailable`, `refused`,
/// or `timeout`.
struct LanguageModelError: Error {
    let code: String
    let message: String
}

enum SystemModel {
    /// The name of the model, or the reason why it cannot run.
    static func status() -> Result<String, LanguageModelError> {
        #if canImport(FoundationModels)
            if #available(macOS 26.0, *) {
                let model = SystemLanguageModel.default
                switch model.availability {
                case .available:
                    return .success(name(of: model))
                case .unavailable(.deviceNotEligible):
                    return unavailable("this Mac does not support Apple Intelligence")
                case .unavailable(.appleIntelligenceNotEnabled):
                    return unavailable("Apple Intelligence is off in System Settings")
                case .unavailable(.modelNotReady):
                    return unavailable("macOS still downloads the Apple Intelligence model")
                case .unavailable:
                    return unavailable("Apple Intelligence is not available")
                }
            }
        #endif
        return unavailable("Apple Intelligence needs macOS 26 or later")
    }

    private static func unavailable(_ message: String) -> Result<String, LanguageModelError> {
        .failure(LanguageModelError(code: "unavailable", message: message))
    }

    /// Run the model and return the `text` field of its reply, with the name of the model.
    static func generate(instructions: String, prompt: String, temperature: Double?, timeoutMs: Int)
        async throws -> (text: String, model: String)
    {
        let name = try status().get()
        #if canImport(FoundationModels)
            if #available(macOS 26.0, *) {
                // The default guardrails refuse ordinary text that only gets edited.
                let model = SystemLanguageModel(guardrails: .permissiveContentTransformations)
                let session = LanguageModelSession(model: model, instructions: instructions)
                let work = Task {
                    try await session.respond(
                        to: prompt, generating: EditedText.self,
                        options: GenerationOptions(temperature: temperature)
                    ).content.text
                }
                let deadline = Task {
                    try await Task.sleep(nanoseconds: UInt64(max(timeoutMs, 1)) * 1_000_000)
                    work.cancel()
                }
                defer { deadline.cancel() }
                do {
                    return (try await work.value, name)
                } catch {
                    throw describe(error, timedOut: work.isCancelled, timeoutMs: timeoutMs)
                }
            }
        #endif
        throw LanguageModelError(code: "unavailable", message: "Apple Intelligence needs macOS 26 or later")
    }

    #if canImport(FoundationModels)
        @available(macOS 26.0, *)
        private static func name(of model: SystemLanguageModel) -> String {
            // `variant` came with the macOS 27 SDK (Swift 6.4).
            #if compiler(>=6.4)
                if #available(macOS 27.0, *) { return model.variant.displayName }
            #endif
            return "Apple Intelligence"
        }
    #endif

    private static func describe(_ error: Error, timedOut: Bool, timeoutMs: Int) -> LanguageModelError {
        if timedOut {
            return LanguageModelError(code: "timeout", message: "no reply within \(timeoutMs) ms")
        }
        // The error types differ between the macOS 26 and macOS 27 SDKs, but the case names
        // are the same, so the name decides.
        let name = String(describing: error)
        let refused = name.hasPrefix("guardrailViolation") || name.hasPrefix("refusal")
        return LanguageModelError(code: refused ? "refused" : "failed", message: error.localizedDescription)
    }
}
