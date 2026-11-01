//! The permission of the system to use the microphone.
//!
//! A denied microphone on macOS gives silence, not an error, so an app asks
//! here before it starts a capture. Windows and Linux tie no permission to the
//! app here: a refused microphone there is an error of [`crate::mic::start`].

/// The answer of the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    Granted,
    /// The system has not asked the user yet.
    NotDetermined,
    Denied,
}

/// The state now. It does not ask the user.
pub fn status() -> Permission {
    imp::status()
}

/// Show the question of the system when it has not asked yet, and give the
/// answer to `on_answer`, on any thread. A process with no
/// `NSMicrophoneUsageDescription` gets no question (macOS would kill it), and
/// `on_answer` gets false.
pub fn request(on_answer: impl FnOnce(bool) + Send + 'static) {
    imp::request(Box::new(on_answer));
}

/// The address of the page of System Settings with this permission, for the
/// app to open. None when the system has no such page.
pub fn settings_url() -> Option<&'static str> {
    cfg!(target_os = "macos")
        .then_some("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
}

#[cfg(target_os = "macos")]
mod imp {
    use super::Permission;
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
    use objc2_foundation::{NSBundle, NSString};
    use std::sync::Mutex;

    pub fn status() -> Permission {
        // SAFETY: `AVMediaTypeAudio` is an AVFoundation constant.
        let status = unsafe {
            let Some(media) = AVMediaTypeAudio else {
                return Permission::NotDetermined;
            };
            AVCaptureDevice::authorizationStatusForMediaType(media)
        };
        match status {
            AVAuthorizationStatus::Authorized => Permission::Granted,
            AVAuthorizationStatus::NotDetermined => Permission::NotDetermined,
            _ => Permission::Denied,
        }
    }

    fn has_usage_text() -> bool {
        NSBundle::mainBundle()
            .objectForInfoDictionaryKey(&NSString::from_str("NSMicrophoneUsageDescription"))
            .is_some()
    }

    pub fn request(on_answer: Box<dyn FnOnce(bool) + Send>) {
        if !has_usage_text() {
            on_answer(false);
            return;
        }
        // The block is an `Fn`, and AVFoundation calls it once.
        let on_answer = Mutex::new(Some(on_answer));
        let handler = RcBlock::new(move |granted: Bool| {
            if let Some(on_answer) = on_answer
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
            {
                on_answer(granted.as_bool());
            }
        });
        // SAFETY: AVFoundation copies the block and calls it once, on an arbitrary queue.
        unsafe {
            let Some(media) = AVMediaTypeAudio else {
                return;
            };
            AVCaptureDevice::requestAccessForMediaType_completionHandler(media, &handler);
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::Permission;

    pub fn status() -> Permission {
        Permission::Granted
    }

    pub fn request(on_answer: Box<dyn FnOnce(bool) + Send>) {
        on_answer(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_settings_page_is_there_on_macos_only() {
        assert_eq!(settings_url().is_some(), cfg!(target_os = "macos"));
    }

    /// `cargo test` has no usage text, so no question shows and the process lives.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_process_with_no_usage_text_gets_no_question() {
        let (tx, rx) = std::sync::mpsc::channel();
        request(move |granted| tx.send(granted).unwrap());
        assert!(!rx.recv().unwrap());
    }
}
