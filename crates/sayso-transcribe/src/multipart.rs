//! A small `multipart/form-data` body builder. The providers take one WAV
//! file and a few text fields, so a crate would be more than we need.

use std::sync::atomic::{AtomicU64, Ordering};

/// Makes each boundary different, so two bodies in one process never share one.
static COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) struct Multipart {
    boundary: String,
    body: Vec<u8>,
}

impl Multipart {
    pub(crate) fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        Self { boundary: format!("----SaysoFormBoundary7d3f9a1c5e{n:08x}"), body: Vec::new() }
    }

    /// The value of the `Content-Type` header for this body.
    pub(crate) fn content_type(&self) -> String {
        format!("multipart/form-data; boundary={}", self.boundary)
    }

    /// A plain text field.
    pub(crate) fn text(&mut self, name: &str, value: &str) {
        self.begin(name, None, None);
        self.body.extend_from_slice(value.as_bytes());
        self.body.extend_from_slice(b"\r\n");
    }

    /// A part with a content type and no file name, for example a JSON config.
    pub(crate) fn part(&mut self, name: &str, content_type: &str, data: &[u8]) {
        self.begin(name, None, Some(content_type));
        self.body.extend_from_slice(data);
        self.body.extend_from_slice(b"\r\n");
    }

    /// A file part.
    pub(crate) fn file(&mut self, name: &str, filename: &str, content_type: &str, data: &[u8]) {
        self.begin(name, Some(filename), Some(content_type));
        self.body.extend_from_slice(data);
        self.body.extend_from_slice(b"\r\n");
    }

    /// Close the body and return the bytes.
    pub(crate) fn finish(mut self) -> Vec<u8> {
        self.body.extend_from_slice(format!("--{}--\r\n", self.boundary).as_bytes());
        self.body
    }

    fn begin(&mut self, name: &str, filename: Option<&str>, content_type: Option<&str>) {
        let mut head = format!("--{}\r\nContent-Disposition: form-data; name=\"{}\"", self.boundary, quoted(name));
        if let Some(filename) = filename {
            head.push_str(&format!("; filename=\"{}\"", quoted(filename)));
        }
        head.push_str("\r\n");
        if let Some(content_type) = content_type {
            head.push_str(&format!("Content-Type: {content_type}\r\n"));
        }
        head.push_str("\r\n");
        self.body.extend_from_slice(head.as_bytes());
    }
}

/// Make a name safe inside a quoted header value: no line breaks, no quote.
fn quoted(value: &str) -> String {
    value.chars().filter(|c| !matches!(c, '\r' | '\n')).collect::<String>().replace('"', "%22")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_text_part_and_file_parts() {
        let mut form = Multipart::new();
        let boundary = form.content_type().split("boundary=").nth(1).unwrap().to_string();
        form.text("model", "whisper-1");
        form.part("config", "application/json", b"{}");
        form.file("file", "audio.wav", "audio/wav", &[0, 159, 146, 150]);
        let body = form.finish();

        let mut expected = Vec::new();
        expected.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nwhisper-1\r\n\
                 --{boundary}\r\nContent-Disposition: form-data; name=\"config\"\r\nContent-Type: application/json\r\n\r\n{{}}\r\n\
                 --{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
            )
            .as_bytes(),
        );
        expected.extend_from_slice(&[0, 159, 146, 150]);
        expected.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        assert_eq!(body, expected);
    }

    #[test]
    fn each_body_has_its_own_boundary() {
        assert_ne!(Multipart::new().content_type(), Multipart::new().content_type());
        assert!(Multipart::new().content_type().starts_with("multipart/form-data; boundary=----"));
    }

    #[test]
    fn names_cannot_break_the_header() {
        let mut form = Multipart::new();
        form.file("fi\"le\r\nX: y", "a\"b\r\n.wav", "audio/wav", b"x");
        let body = String::from_utf8(form.finish()).unwrap();
        assert!(body.contains("name=\"fi%22leX: y\"; filename=\"a%22b.wav\""), "{body}");
    }
}
