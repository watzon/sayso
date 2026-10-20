//! The sentence for a failure or an unavailable state that a speech engine reports.
//!
//! The engines send a stable reason code and an English detail (see
//! `sayso-engine-client/NOTES.md`). The app translates the code. An older engine,
//! or a code that this version does not know, gives no reason. The sentence is then
//! general and shows the English detail.

use sayso_core::stt::EngineReason;
use sayso_i18n::t;

/// One sentence that says what went wrong. `detail` is the English message of the
/// engine. Some sentences show it, because it names the cause (a network error,
/// for example).
pub fn describe(reason: Option<EngineReason>, detail: &str) -> String {
    let detail = detail.trim().trim_end_matches('.');
    match reason {
        Some(EngineReason::IncompleteDownload) => t!("engine.incomplete_download"),
        Some(EngineReason::DownloadFailed) => t!("engine.download_failed", detail = detail),
        Some(EngineReason::LoadFailed) => t!("engine.load_failed", detail = detail),
        Some(EngineReason::NoGpu) => t!("engine.no_gpu"),
        Some(EngineReason::DeviceNotEligible) => t!("engine.device_not_eligible"),
        Some(EngineReason::IntelligenceOff) => t!("engine.intelligence_off"),
        Some(EngineReason::ModelNotReady) => t!("engine.model_not_ready"),
        Some(EngineReason::NeedsMacos26) => t!("engine.needs_macos_26"),
        Some(EngineReason::Unavailable) => t!("engine.unavailable"),
        Some(EngineReason::Timeout) => t!("engine.timeout"),
        Some(EngineReason::Refused) => t!("engine.refused"),
        Some(EngineReason::Failed) => t!("engine.failed", detail = detail),
        None => t!("engine.general", detail = detail),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_reason_gets_its_own_sentence() {
        assert_eq!(
            describe(Some(EngineReason::IntelligenceOff), "Apple Intelligence is off in System Settings"),
            "Apple Intelligence is off. Turn it on in System Settings."
        );
        assert_eq!(
            describe(Some(EngineReason::DownloadFailed), "HTTP 404."),
            "The download did not finish: HTTP 404."
        );
    }

    #[test]
    fn no_reason_gives_the_general_sentence_with_the_detail() {
        assert_eq!(describe(None, "boom"), "Something went wrong: boom.");
    }
}
