//! The system audio (`docs/sdk.md`, "System audio"): the sound of other apps
//! and calls, as 16 kHz mono frames of 20 ms on the capture clock.
//!
//! Each system has a [`Backend`], which opens a stream of the audio:
//!
//! - macOS 14.2 and later: a Core Audio process tap through an aggregate device.
//! - Windows: WASAPI loopback of the default output device, with `cpal`.
//! - Linux: the monitor of the default sink, through the PulseAudio protocol
//!   that PulseAudio and PipeWire both serve.
//!
//! The rest is the same on each system:
//!
//! - The callback of a stream queues each frame, as the microphone does. The
//!   **worker** thread gives the frames to your function, in order. When no
//!   audio arrives (Windows gives none while nothing plays), it gives silence,
//!   so the time of the source goes on.
//! - The **supervisor** thread looks at the stream a few times each second. A
//!   change of the output device, or a stream that broke, opens a new stream and
//!   gives [`Event::Gap`]. A permission that the user took back gives
//!   [`Event::Ended`].

use crate::clock;
use crate::mic::{FRAME_SAMPLES, FRAME_TIME, Frame, QUEUE_FRAMES, RawFrame, display_level, rms};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TrySendError, bounded, unbounded};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

/// A process that the capture should leave out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exclude {
    /// The name in [`Capture::excludes`].
    pub name: String,
    pub pid: u32,
}

/// The answer of the system about the permission to capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Granted,
    Denied,
    /// No answer yet, or the system does not say. Windows and Linux have no such permission, and say `Granted`.
    Unknown,
}

/// Look at the permission. This never shows a question to the user.
pub fn access() -> Access {
    #[cfg(target_os = "macos")]
    return macos::preflight();
    #[cfg(not(target_os = "macos"))]
    Access::Granted
}

/// What a capture does.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// The processes to leave out, where the system allows it. See [`Capture::excludes`].
    pub exclude: Vec<Exclude>,
}

/// What a capture gives, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Frame(Frame),
    /// The capture lost this much audio, for example at a change of the output device.
    Gap { ms: u64 },
    /// The capture ended. No more events come. The capture still needs its drop.
    Ended(End),
}

/// Why a capture ended by itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum End {
    /// The user took the permission of the system back.
    Permission,
    /// The audio did not come back after [`GIVE_UP`]. The text is English.
    Failed(String),
}

/// Why a capture did not start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The system refused the permission to capture.
    PermissionDenied,
    /// This system cannot capture its audio. The text is English.
    Unsupported(String),
    /// The text is English.
    Failed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::PermissionDenied => f.write_str("the system audio permission is denied"),
            Error::Unsupported(message) | Error::Failed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

/// How often the supervisor looks at the stream.
const TICK: Duration = Duration::from_millis(250);
/// The wait before another try to open a stream.
const RETRY: Duration = Duration::from_millis(500);
/// A capture that cannot open a stream for this long ends.
pub const GIVE_UP: Duration = Duration::from_secs(60);
/// The worker gives silence when no audio arrived for this long.
const SILENCE_AFTER: Duration = Duration::from_millis(200);
/// How often the worker looks for that.
const IDLE_CHECK: Duration = Duration::from_millis(100);
/// A longer hole is not silence: the computer slept, for example. It is a gap.
const MAX_SILENCE: Duration = Duration::from_secs(10);
/// How long a drop waits for the worker and the supervisor.
const STOP_TIMEOUT: Duration = Duration::from_secs(2);

/// What a stream and the supervisor send to the worker.
// A box would allocate in the callback.
#[allow(clippy::large_enum_variant)]
enum Message {
    Frame(RawFrame, Duration),
    /// A new stream opens: the next frame comes after a gap.
    Reopening,
    Ended(End),
    Stop,
}

/// The callback side of the queue of the worker. As for the microphone, a full
/// queue makes the callback wait: a dropped frame would cut a hole in the recording.
pub(crate) struct FrameSink {
    tx: Sender<Message>,
    lost: bool,
}

impl FrameSink {
    pub(crate) fn push(&mut self, frame: &RawFrame) {
        let message = Message::Frame(*frame, clock::now().saturating_sub(FRAME_TIME));
        let gone = match self.tx.try_send(message) {
            Ok(()) => false,
            Err(TrySendError::Full(message)) => self.tx.send(message).is_err(),
            Err(TrySendError::Disconnected(_)) => true,
        };
        if gone && !self.lost {
            self.lost = true;
            log::error!("the worker of the system audio ended early");
        }
    }
}

/// A stream of the system audio. Dropping it closes it.
pub(crate) trait Stream: Send {
    /// False when the stream must open again: the output device changed, its
    /// format changed, or it broke.
    fn still_good(&mut self) -> bool;
}

/// The part of each system.
pub(crate) trait Backend: Send + 'static {
    /// Check that the system can capture, and ask for its permission when
    /// that has no answer yet. This can wait for the user.
    fn prepare(&mut self) -> Result<(), Error>;
    /// False when the user took the permission back.
    fn permission_held(&self) -> bool;
    /// Open a stream that gives its frames to `sink`.
    fn open(&mut self, sink: FrameSink) -> Result<Box<dyn Stream>, Error>;
    /// The names of the processes that the streams leave out.
    fn excludes(&self) -> Vec<String>;
}

#[cfg(target_os = "macos")]
fn backend(options: Options) -> Box<dyn Backend> {
    Box::new(macos::Tap::new(options))
}

#[cfg(windows)]
fn backend(_options: Options) -> Box<dyn Backend> {
    Box::new(windows::Loopback)
}

#[cfg(target_os = "linux")]
fn backend(_options: Options) -> Box<dyn Backend> {
    Box::new(linux::Monitor)
}

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
fn backend(_options: Options) -> Box<dyn Backend> {
    struct NoBackend;
    impl Backend for NoBackend {
        fn prepare(&mut self) -> Result<(), Error> {
            Err(Error::Unsupported("this system has no capture of the system audio".into()))
        }
        fn permission_held(&self) -> bool {
            true
        }
        fn open(&mut self, _sink: FrameSink) -> Result<Box<dyn Stream>, Error> {
            Err(Error::Unsupported("this system has no capture of the system audio".into()))
        }
        fn excludes(&self) -> Vec<String> {
            Vec::new()
        }
    }
    Box::new(NoBackend)
}

/// A capture that runs. Dropping it ends the capture, after the last frames
/// reached the function.
pub struct Capture {
    excludes: Vec<String>,
    stop: Sender<()>,
    supervisor: Option<JoinHandle<()>>,
    worker: Option<WorkerHandle>,
}

impl Capture {
    /// The names from [`Options::exclude`] of the processes that this capture
    /// leaves out. On macOS, each of them. On Windows and Linux, none: the
    /// system mixes the output, and the capture cannot take a process out of the mix.
    pub fn excludes(&self) -> &[String] {
        &self.excludes
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(supervisor) = self.supervisor.take() {
            let _ = supervisor.join();
        }
        if let Some(worker) = self.worker.take() {
            worker.finish();
        }
    }
}

/// Start to capture the system audio. `on_event` runs on a worker thread, so a
/// slow function delays frames but does not drop them.
///
/// This can wait for the answer of the user to a question of the system. On
/// macOS the process needs `NSAudioCaptureUsageDescription` in its `Info.plist`.
pub fn start(options: Options, on_event: Box<dyn FnMut(Event) + Send>) -> Result<Capture, Error> {
    start_with(backend(options), Timing::REAL, on_event)
}

/// The waits of the supervisor, which the tests make short.
#[derive(Clone, Copy)]
struct Timing {
    tick: Duration,
    retry: Duration,
    give_up: Duration,
}

impl Timing {
    const REAL: Timing = Timing { tick: TICK, retry: RETRY, give_up: GIVE_UP };
}

fn start_with(mut backend: Box<dyn Backend>, timing: Timing, on_event: Box<dyn FnMut(Event) + Send>) -> Result<Capture, Error> {
    backend.prepare()?;
    let (worker, tx) = WorkerHandle::start(on_event)?;
    let stream = backend.open(FrameSink { tx: tx.clone(), lost: false })?;
    let excludes = backend.excludes();
    let (stop, stopped) = bounded(1);
    let supervisor = std::thread::Builder::new()
        .name("sayso-system-audio".into())
        .spawn(move || supervise(backend, stream, &tx, &stopped, timing))
        .map_err(|e| Error::Failed(format!("the thread of the system audio: {e}")))?;
    Ok(Capture { excludes, stop, supervisor: Some(supervisor), worker: Some(worker) })
}

fn supervise(mut backend: Box<dyn Backend>, stream: Box<dyn Stream>, tx: &Sender<Message>, stopped: &Receiver<()>, timing: Timing) {
    let mut stream = Some(stream);
    let mut failing_since: Option<Instant> = None;
    loop {
        let wait = if stream.is_some() { timing.tick } else { timing.retry };
        if !matches!(stopped.recv_timeout(wait), Err(RecvTimeoutError::Timeout)) {
            return;
        }
        if !backend.permission_held() {
            log::info!("system audio: the permission was taken back");
            drop(stream);
            let _ = tx.send(Message::Ended(End::Permission));
            let _ = stopped.recv();
            return;
        }
        if let Some(open) = &mut stream {
            if open.still_good() {
                continue;
            }
            log::info!("system audio: the stream opens again");
            stream = None;
            let _ = tx.send(Message::Reopening);
        }
        match backend.open(FrameSink { tx: tx.clone(), lost: false }) {
            Ok(opened) => {
                stream = Some(opened);
                failing_since = None;
            }
            Err(error) => {
                let since = *failing_since.get_or_insert_with(Instant::now);
                let end = match error {
                    Error::PermissionDenied => Some(End::Permission),
                    Error::Unsupported(message) => Some(End::Failed(message)),
                    Error::Failed(message) if since.elapsed() >= timing.give_up => Some(End::Failed(message)),
                    Error::Failed(message) => {
                        log::debug!("system audio: no stream yet: {message}");
                        None
                    }
                };
                if let Some(end) = end {
                    let _ = tx.send(Message::Ended(end));
                    let _ = stopped.recv();
                    return;
                }
            }
        }
    }
}

/// What the worker keeps of the time of the source. It has no clock of its
/// own, so the tests give it the times.
#[derive(Debug, Default)]
struct Timeline {
    /// The end of the last frame.
    end: Duration,
    /// A new stream opens.
    reopening: bool,
    ended: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Idle {
    Nothing,
    /// Silent frames with these times.
    Silence(Vec<Duration>),
    Gap(u64),
}

fn gap_ms(hole: Duration) -> u64 {
    u64::try_from(hole.as_millis()).unwrap_or(u64::MAX)
}

impl Timeline {
    fn new(now: Duration) -> Timeline {
        Timeline { end: now, reopening: false, ended: false }
    }

    /// A frame with this time arrived. Gives the gap that comes before it.
    fn frame(&mut self, time: Duration) -> Option<u64> {
        let gap = self.reopening.then(|| gap_ms(time.saturating_sub(self.end)));
        self.reopening = false;
        self.end = self.end.max(time + FRAME_TIME);
        gap
    }

    /// No frame arrived for a while, and the time is now `now`.
    fn idle(&mut self, now: Duration) -> Idle {
        if self.reopening || self.ended {
            return Idle::Nothing;
        }
        let hole = now.saturating_sub(self.end);
        if hole < SILENCE_AFTER {
            return Idle::Nothing;
        }
        let start = self.end;
        self.end = now;
        if hole > MAX_SILENCE {
            return Idle::Gap(gap_ms(hole));
        }
        let frames = (hole.as_millis() / FRAME_TIME.as_millis()) as u32;
        self.end = start + FRAME_TIME * frames;
        Idle::Silence((0..frames).map(|frame| start + FRAME_TIME * frame).collect())
    }
}

/// The worker thread: it turns the messages into `on_event` calls, in order.
struct WorkerHandle {
    tx: Sender<Message>,
    done: Receiver<()>,
}

impl WorkerHandle {
    fn start(mut on_event: Box<dyn FnMut(Event) + Send>) -> Result<(WorkerHandle, Sender<Message>), Error> {
        let (tx, rx) = bounded::<Message>(QUEUE_FRAMES);
        let (done_tx, done) = unbounded();
        std::thread::Builder::new()
            .name("sayso-system-audio-worker".into())
            .spawn(move || {
                let mut timeline = Timeline::new(clock::now());
                loop {
                    match rx.recv_timeout(IDLE_CHECK) {
                        Ok(Message::Frame(samples, time)) => {
                            if let Some(ms) = timeline.frame(time) {
                                on_event(Event::Gap { ms });
                            }
                            let level = display_level(rms(&samples));
                            on_event(Event::Frame(Frame { samples: samples.to_vec(), level, time }));
                        }
                        Ok(Message::Reopening) => timeline.reopening = true,
                        Ok(Message::Ended(end)) => {
                            timeline.ended = true;
                            on_event(Event::Ended(end));
                        }
                        Ok(Message::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                        Err(RecvTimeoutError::Timeout) => match timeline.idle(clock::now()) {
                            Idle::Nothing => {}
                            Idle::Gap(ms) => on_event(Event::Gap { ms }),
                            Idle::Silence(times) => {
                                for time in times {
                                    on_event(Event::Frame(Frame { samples: vec![0.0; FRAME_SAMPLES], level: 0.0, time }));
                                }
                            }
                        },
                    }
                }
                // Dropping the function closes what it sends to.
                drop(on_event);
                let _ = done_tx.send(());
            })
            .map_err(|e| Error::Failed(format!("the worker of the system audio: {e}")))?;
        Ok((WorkerHandle { tx: tx.clone(), done }, tx))
    }

    /// Wait until the function has seen every queued message.
    fn finish(self) {
        let _ = self.tx.send(Message::Stop);
        if self.done.recv_timeout(STOP_TIMEOUT) == Err(RecvTimeoutError::Timeout) {
            log::error!("the worker of the system audio did not finish in {} s", STOP_TIMEOUT.as_secs());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    const MS: Duration = Duration::from_millis(1);

    /// A backend that the test steers.
    #[derive(Default)]
    struct Fake {
        permission_gone: AtomicBool,
        refuse_at_prepare: AtomicBool,
        /// `still_good` of each open stream says false, once.
        device_changed: AtomicBool,
        /// `open` fails while this is true.
        fail_open: AtomicBool,
        /// `open` says that the permission is gone.
        refuse_open: AtomicBool,
        opened: AtomicUsize,
        closed: AtomicUsize,
        sink: Mutex<Option<FrameSink>>,
    }

    struct FakeBackend(Arc<Fake>);
    struct FakeStream(Arc<Fake>);

    impl Stream for FakeStream {
        fn still_good(&mut self) -> bool {
            !self.0.device_changed.swap(false, Ordering::SeqCst)
        }
    }

    impl Drop for FakeStream {
        fn drop(&mut self) {
            self.0.closed.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl Backend for FakeBackend {
        fn prepare(&mut self) -> Result<(), Error> {
            if self.0.refuse_at_prepare.load(Ordering::SeqCst) { Err(Error::PermissionDenied) } else { Ok(()) }
        }
        fn permission_held(&self) -> bool {
            !self.0.permission_gone.load(Ordering::SeqCst)
        }
        fn open(&mut self, sink: FrameSink) -> Result<Box<dyn Stream>, Error> {
            if self.0.refuse_open.load(Ordering::SeqCst) {
                return Err(Error::PermissionDenied);
            }
            if self.0.fail_open.load(Ordering::SeqCst) {
                return Err(Error::Failed("no device".into()));
            }
            *self.0.sink.lock() = Some(sink);
            self.0.opened.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(FakeStream(self.0.clone())))
        }
        fn excludes(&self) -> Vec<String> {
            vec!["app".into()]
        }
    }

    impl Fake {
        fn frame(&self) {
            self.sink.lock().as_mut().expect("a stream is open").push(&[0.25; FRAME_SAMPLES]);
        }
    }

    const FAST: Timing = Timing { tick: Duration::from_millis(2), retry: Duration::from_millis(2), give_up: Duration::from_millis(40) };
    const NEVER_GIVE_UP: Timing = Timing { give_up: Duration::from_secs(3600), ..FAST };

    fn capture(fake: &Arc<Fake>, timing: Timing) -> (Capture, Receiver<Event>) {
        let (tx, rx) = unbounded();
        let capture = start_with(Box::new(FakeBackend(fake.clone())), timing, Box::new(move |event| drop(tx.send(event)))).unwrap();
        (capture, rx)
    }

    /// The next event that is not a frame of silence from the worker.
    fn next(rx: &Receiver<Event>) -> Event {
        loop {
            let event = rx.recv_timeout(Duration::from_secs(30)).expect("an event comes");
            if !matches!(&event, Event::Frame(frame) if frame.level == 0.0) {
                return event;
            }
        }
    }

    fn eventually(what: &str, check: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !check() {
            assert!(Instant::now() < deadline, "{what}");
            std::thread::sleep(MS);
        }
    }

    #[test]
    fn frames_arrive_in_the_format_of_the_engines_and_the_drop_closes_the_stream() {
        let fake = Arc::new(Fake::default());
        let (capture, rx) = capture(&fake, NEVER_GIVE_UP);
        assert_eq!(capture.excludes(), ["app"]);
        fake.frame();
        let Event::Frame(frame) = next(&rx) else { panic!("a frame") };
        assert_eq!(frame.samples.len(), FRAME_SAMPLES);
        drop(capture);
        assert_eq!(fake.closed.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_refused_permission_stops_the_start() {
        let fake = Arc::new(Fake::default());
        fake.refuse_at_prepare.store(true, Ordering::SeqCst);
        let started = start_with(Box::new(FakeBackend(fake.clone())), FAST, Box::new(|_| {}));
        assert_eq!(started.err(), Some(Error::PermissionDenied));
        assert_eq!(fake.opened.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_change_of_the_device_opens_a_new_stream_and_gives_a_gap() {
        let fake = Arc::new(Fake::default());
        let (_capture, rx) = capture(&fake, NEVER_GIVE_UP);
        fake.frame();
        assert!(matches!(next(&rx), Event::Frame(_)));
        fake.device_changed.store(true, Ordering::SeqCst);
        eventually("the second stream opens", || fake.opened.load(Ordering::SeqCst) == 2);
        assert_eq!(fake.closed.load(Ordering::SeqCst), 1, "the first stream closed");
        fake.frame();
        assert!(matches!(next(&rx), Event::Gap { .. }), "a gap comes before the frame");
        assert!(matches!(next(&rx), Event::Frame(_)));
    }

    #[test]
    fn a_device_that_comes_back_gives_one_gap() {
        let fake = Arc::new(Fake::default());
        let (_capture, rx) = capture(&fake, NEVER_GIVE_UP);
        fake.fail_open.store(true, Ordering::SeqCst);
        fake.device_changed.store(true, Ordering::SeqCst);
        eventually("the stream closed", || fake.closed.load(Ordering::SeqCst) == 1);
        fake.fail_open.store(false, Ordering::SeqCst);
        eventually("the stream opens again", || fake.opened.load(Ordering::SeqCst) == 2);
        fake.frame();
        assert!(matches!(next(&rx), Event::Gap { .. }));
        assert!(matches!(next(&rx), Event::Frame(_)));
    }

    #[test]
    fn a_stream_that_never_comes_back_ends_the_capture() {
        let fake = Arc::new(Fake::default());
        let (capture, rx) = capture(&fake, FAST);
        fake.fail_open.store(true, Ordering::SeqCst);
        fake.device_changed.store(true, Ordering::SeqCst);
        assert_eq!(next(&rx), Event::Ended(End::Failed("no device".into())));
        drop(capture);
    }

    #[test]
    fn a_permission_taken_back_ends_the_capture_and_closes_the_stream() {
        let fake = Arc::new(Fake::default());
        let (capture, rx) = capture(&fake, NEVER_GIVE_UP);
        fake.frame();
        assert!(matches!(next(&rx), Event::Frame(_)));
        fake.permission_gone.store(true, Ordering::SeqCst);
        assert_eq!(next(&rx), Event::Ended(End::Permission));
        assert_eq!(fake.closed.load(Ordering::SeqCst), 1);
        drop(capture);
    }

    #[test]
    fn a_new_stream_that_is_refused_ends_the_capture_with_the_permission() {
        let fake = Arc::new(Fake::default());
        let (_capture, rx) = capture(&fake, NEVER_GIVE_UP);
        fake.refuse_open.store(true, Ordering::SeqCst);
        fake.device_changed.store(true, Ordering::SeqCst);
        assert_eq!(next(&rx), Event::Ended(End::Permission));
    }

    #[test]
    fn the_timeline_fills_a_quiet_time_with_silence() {
        let mut timeline = Timeline::new(Duration::ZERO);
        assert_eq!(timeline.frame(Duration::ZERO), None);
        assert_eq!(timeline.idle(MS * 100), Idle::Nothing, "too short to say");
        let Idle::Silence(times) = timeline.idle(MS * 300) else { panic!("silence") };
        assert_eq!(times.len(), 14, "from 20 ms to 300 ms");
        assert_eq!((times[0], times[13]), (MS * 20, MS * 280));
        assert_eq!(timeline.idle(MS * 310), Idle::Nothing, "the silence reaches to now");
        assert_eq!(timeline.frame(MS * 320), None, "a frame after silence is not a gap");
    }

    #[test]
    fn the_timeline_calls_a_long_hole_a_gap_and_gives_no_silence_for_it() {
        let mut timeline = Timeline::new(Duration::ZERO);
        assert_eq!(timeline.idle(Duration::from_secs(3600)), Idle::Gap(3_600_000));
        assert_eq!(timeline.idle(Duration::from_secs(3600) + MS), Idle::Nothing);
    }

    #[test]
    fn the_timeline_gives_no_silence_while_a_stream_opens_and_the_gap_after_it() {
        let mut timeline = Timeline::new(Duration::ZERO);
        timeline.frame(Duration::ZERO);
        timeline.reopening = true;
        assert_eq!(timeline.idle(MS * 500), Idle::Nothing);
        assert_eq!(timeline.frame(MS * 520), Some(500));
        assert_eq!(timeline.frame(MS * 540), None);
    }

    #[test]
    fn the_timeline_gives_nothing_after_the_end() {
        let mut timeline = Timeline::new(Duration::ZERO);
        timeline.ended = true;
        assert_eq!(timeline.idle(MS * 500), Idle::Nothing);
    }

    #[test]
    fn a_frame_late_after_silence_does_not_move_the_end_back() {
        let mut timeline = Timeline::new(MS * 1000);
        assert_eq!(timeline.frame(MS * 500), None);
        assert_eq!(timeline.end, MS * 1000);
    }
}
