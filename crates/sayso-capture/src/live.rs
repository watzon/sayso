//! The captures of this crate as the sources of a live session of
//! `sayso-speech`. The local mode of the SDK puts these into `LiveDeps`.

use crate::{mic, system};
use crossbeam_channel::unbounded;
use sayso_speech::live::{AudioFeed, CapturedAudio, FeedItem, Microphone, OpenError, SourceEnd, SystemAudio};

/// The microphone of a live session, with a capture of its own.
pub struct LiveMicrophone {
    /// None: the default input device.
    pub device: Option<String>,
}

impl Microphone for LiveMicrophone {
    fn open(&self) -> Result<AudioFeed, OpenError> {
        let (tx, items) = unbounded();
        let on_frame = Box::new(move |frame: mic::Frame| {
            let _ = tx.send(FeedItem::Audio(CapturedAudio { samples: frame.samples, time: Some(frame.time) }));
        });
        let capture = mic::start(self.device.as_deref(), on_frame).map_err(|e| match e {
            mic::Error::PermissionDenied => OpenError::Permission(e.to_string()),
            mic::Error::Failed(message) => OpenError::Failed(message),
        })?;
        Ok(AudioFeed { items, lease: Box::new(capture), excludes: Vec::new() })
    }
}

/// The system audio of a live session. The capture leaves out this process.
pub struct LiveSystemAudio {
    /// The name of this process in `excludes`: `sayso` in Sayso, `app` in an app.
    pub this_process: &'static str,
}

/// The name in `excludes` of the app that asked Sayso for the session.
const CONNECTED_APP: &str = "app";

impl SystemAudio for LiveSystemAudio {
    fn open(&self, app_pid: Option<u32>) -> Result<AudioFeed, OpenError> {
        let mut exclude = vec![system::Exclude { name: self.this_process.into(), pid: std::process::id() }];
        exclude.extend(app_pid.map(|pid| system::Exclude { name: CONNECTED_APP.into(), pid }));
        let (tx, items) = unbounded();
        let on_event = Box::new(move |event| {
            let _ = tx.send(feed_item(event));
        });
        let capture = system::start(system::Options { exclude }, on_event).map_err(|e| match e {
            system::Error::PermissionDenied => OpenError::Permission(e.to_string()),
            system::Error::Unsupported(message) => OpenError::Unsupported(message),
            system::Error::Failed(message) => OpenError::Failed(message),
        })?;
        let excludes = capture.excludes();
        Ok(AudioFeed { items, lease: Box::new(capture), excludes })
    }
}

fn feed_item(event: system::Event) -> FeedItem {
    match event {
        system::Event::Frame(frame) => FeedItem::Audio(CapturedAudio { samples: frame.samples, time: Some(frame.time) }),
        system::Event::Gap { ms } => FeedItem::Gap { ms },
        system::Event::Ended(system::End::Permission) => FeedItem::Ended(SourceEnd::Permission),
        system::Event::Ended(system::End::Failed(message)) => FeedItem::Ended(SourceEnd::Failed(message)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn events_become_the_items_of_a_session() {
        let frame = mic::Frame { samples: vec![0.5], level: 0.1, time: Duration::from_millis(7) };
        assert_eq!(feed_item(system::Event::Frame(frame)), FeedItem::Audio(CapturedAudio { samples: vec![0.5], time: Some(Duration::from_millis(7)) }));
        assert_eq!(feed_item(system::Event::Gap { ms: 240 }), FeedItem::Gap { ms: 240 });
        assert_eq!(feed_item(system::Event::Ended(system::End::Permission)), FeedItem::Ended(SourceEnd::Permission));
    }
}
