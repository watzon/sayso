//! The text of a live preview: committed words and the tentative tail.
//!
//! A streaming model decodes one segment at a time. At an endpoint (a pause)
//! the segment is done and its text does not change again, so it moves to
//! `committed`. The text of the current segment is `tentative`.

/// One update from a streaming model.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Segment {
    /// The text of the current segment.
    pub text: String,
    /// True when the segment ended. The next update starts a new segment.
    pub endpoint: bool,
}

#[derive(Debug, Default)]
pub struct Partials {
    committed: String,
    tentative: String,
    /// The text of the last `partial` event. A new stream starts empty.
    last_sent: (String, String),
}

impl Partials {
    pub fn update(&mut self, segment: &Segment) {
        let text = segment.text.trim();
        if segment.endpoint {
            if !text.is_empty() {
                if !self.committed.is_empty() {
                    self.committed.push(' ');
                }
                self.committed.push_str(text);
            }
            self.tentative.clear();
        } else {
            self.tentative = text.to_string();
        }
    }

    /// The committed and tentative text when it differs from the last call
    /// that returned something. The caller sends it as a `partial` event.
    pub fn changed(&mut self) -> Option<(String, String)> {
        let now = (self.committed.clone(), self.tentative.clone());
        if self.last_sent == now {
            return None;
        }
        self.last_sent = now.clone();
        Some(now)
    }

    /// All text, committed and tentative.
    pub fn text(&self) -> String {
        match (self.committed.is_empty(), self.tentative.is_empty()) {
            (_, true) => self.committed.clone(),
            (true, false) => self.tentative.clone(),
            (false, false) => format!("{} {}", self.committed, self.tentative),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(text: &str, endpoint: bool) -> Segment {
        Segment {
            text: text.into(),
            endpoint,
        }
    }

    #[test]
    fn endpoints_commit_text_and_changes_are_reported_once() {
        let mut p = Partials::default();
        p.update(&seg("", false));
        assert_eq!(p.changed(), None, "an empty stream sends nothing");

        p.update(&seg("hello", false));
        assert_eq!(p.changed(), Some((String::new(), "hello".into())));
        p.update(&seg("hello", false));
        assert_eq!(p.changed(), None, "no change, no event");

        p.update(&seg("hello world ", true));
        assert_eq!(p.changed(), Some(("hello world".into(), String::new())));

        p.update(&seg("again", false));
        assert_eq!(p.changed(), Some(("hello world".into(), "again".into())));
        assert_eq!(p.text(), "hello world again");

        p.update(&seg("again and more", true));
        p.update(&seg("", true)); // An empty segment commits nothing.
        assert_eq!(p.text(), "hello world again and more");
        assert_eq!(
            p.changed(),
            Some(("hello world again and more".into(), String::new()))
        );
    }
}
