//! The last lines that an engine wrote to stderr, for the message of a start error.
//!
//! When the engine program cannot start, for example because the system lacks a library,
//! the dynamic loader writes the reason to stderr and the process ends. The protocol has
//! no place for that text, and [`Error::Exited`] says only that the engine has ended.
//! [`StderrTail`] keeps the last lines, so an app can show them with the error.

use crate::{Error, Stderr};
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// How many of the last lines the tail keeps.
const KEPT_LINES: usize = 8;
/// A longer line is cut to this many characters.
const LINE_CHARS: usize = 300;
/// How long [`StderrTail::describe`] waits for the end of stderr after [`Error::Exited`].
const END_WAIT: Duration = Duration::from_millis(500);

#[derive(Default)]
struct State {
    lines: VecDeque<String>,
    /// The reader of stderr is done: the engine closed stderr, and every line is in `lines`.
    ended: bool,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        // A panic in a callback does not make the lines wrong.
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Lives as long as the function that [`StderrTail::stderr`] gives to the reader thread.
/// The thread drops its copy of the function when stderr ends.
struct EndOfReader(Arc<Shared>);

impl Drop for EndOfReader {
    fn drop(&mut self) {
        self.0.lock().ended = true;
        self.0.changed.notify_all();
    }
}

/// The last lines of the stderr of an engine.
///
/// ```no_run
/// use sayso_engine_sdk::{Engine, EngineOptions, StderrTail};
///
/// let tail = StderrTail::new();
/// let options = EngineOptions::new("/opt/myapp/sayso-engine/sayso-engine", "/home/me/.myapp/models")
///     .stderr(tail.stderr(|line| eprintln!("engine: {line}")));
/// match Engine::start(options) {
///     Ok(engine) => println!("engine {}", engine.version()),
///     // For example: "the engine has ended. The engine wrote: error while loading shared libraries: ..."
///     Err(error) => eprintln!("{}", tail.describe(&error)),
/// }
/// ```
#[derive(Clone, Default)]
pub struct StderrTail {
    shared: Arc<Shared>,
}

impl StderrTail {
    pub fn new() -> StderrTail {
        StderrTail::default()
    }

    /// An option for [`EngineOptions::stderr`](crate::EngineOptions::stderr) that keeps the last
    /// lines in this tail and also calls `forward` for each line.
    ///
    /// [`StderrTail::describe`] knows that stderr has ended when the engine closes it and the
    /// options (with this value) are dropped, which [`Engine::start`](crate::Engine::start)
    /// does when it returns. If you keep a copy of the value, `describe` waits its full time.
    pub fn stderr(&self, forward: impl Fn(&str) + Send + Sync + 'static) -> Stderr {
        let shared = self.shared.clone();
        let end = EndOfReader(self.shared.clone());
        Stderr::lines(move |line| {
            // The function owns `end`, so `end` drops with the last copy of the function.
            let _ = &end;
            forward(line);
            let line = line.trim();
            if line.is_empty() {
                return;
            }
            let mut state = shared.lock();
            if state.lines.len() == KEPT_LINES {
                state.lines.pop_front();
            }
            state.lines.push_back(cut(line));
        })
    }

    /// The lines that the engine wrote until now, oldest first.
    pub fn lines(&self) -> Vec<String> {
        self.shared.lock().lines.iter().cloned().collect()
    }

    /// The text of `error`, and the last lines of stderr when there are some:
    /// `the engine has ended. The engine wrote: error while loading shared libraries: ...`.
    ///
    /// After [`Error::Exited`] this waits up to half a second until stderr has ended, so the
    /// last lines are there. For other errors it does not wait.
    pub fn describe(&self, error: &Error) -> String {
        if matches!(error, Error::Exited) {
            self.wait_for_end(END_WAIT);
        }
        let lines = self.lines();
        let message = error.to_string();
        if lines.is_empty() {
            return message;
        }
        format!("{}. The engine wrote: {}", message.trim_end_matches('.'), lines.join(" | "))
    }

    fn wait_for_end(&self, limit: Duration) {
        let deadline = Instant::now() + limit;
        let mut state = self.shared.lock();
        while !state.ended {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return;
            }
            state = self.shared.changed.wait_timeout(state, left).unwrap_or_else(|poisoned| poisoned.into_inner()).0;
        }
    }
}

impl std::fmt::Debug for StderrTail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StderrTail").field("lines", &self.lines()).finish()
    }
}

/// Cut a line to [`LINE_CHARS`] characters.
fn cut(line: &str) -> String {
    match line.char_indices().nth(LINE_CHARS) {
        Some((end, _)) => format!("{}...", &line[..end]),
        None => line.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(tail: &StderrTail, lines: &[&str]) {
        let Stderr::Lines(log) = tail.stderr(|_| {}) else { unreachable!() };
        for line in lines {
            log(line);
        }
    }

    #[test]
    fn it_keeps_the_last_lines_only() {
        let tail = StderrTail::new();
        let numbered: Vec<String> = (1..=12).map(|n| format!("line {n}")).collect();
        feed(&tail, &numbered.iter().map(String::as_str).collect::<Vec<_>>());
        assert_eq!(tail.lines(), numbered[4..]);
    }

    #[test]
    fn it_cuts_a_long_line_and_skips_an_empty_one() {
        let tail = StderrTail::new();
        feed(&tail, &["", "   ", &"é".repeat(1000)]);
        let lines = tail.lines();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], format!("{}...", "é".repeat(LINE_CHARS)));
    }

    #[test]
    fn it_forwards_each_line() {
        let tail = StderrTail::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_in = seen.clone();
        let Stderr::Lines(log) = tail.stderr(move |line| seen_in.lock().unwrap().push(line.to_owned())) else { unreachable!() };
        log("one");
        log("two");
        assert_eq!(*seen.lock().unwrap(), ["one", "two"]);
    }

    #[test]
    fn describe_adds_the_lines_to_the_message() {
        let tail = StderrTail::new();
        assert_eq!(tail.describe(&Error::Protocol("bad".into())), "bad");
        feed(&tail, &["first", "second"]);
        assert_eq!(tail.describe(&Error::Protocol("bad.".into())), "bad. The engine wrote: first | second");
    }

    #[test]
    fn describe_waits_for_the_end_of_stderr_after_exited() {
        let tail = StderrTail::new();
        let stderr = tail.stderr(|_| {});
        let Stderr::Lines(log) = &stderr else { unreachable!() };
        let log = log.clone();
        drop(stderr);
        let reader = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            log("late line");
            // The reader thread ends here and drops its copy.
        });
        let message = tail.describe(&Error::Exited);
        reader.join().unwrap();
        assert_eq!(message, "the engine has ended. The engine wrote: late line");
    }

    #[test]
    fn describe_does_not_wait_for_ever() {
        let tail = StderrTail::new();
        let _kept = tail.stderr(|_| {});
        let start = Instant::now();
        assert_eq!(tail.describe(&Error::Exited), "the engine has ended");
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}
