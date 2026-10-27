//! One capture of the microphone, shared by a dictation and a live session
//! (`docs/extensions.md`, "Capture").
//!
//! The capture starts when the first user joins and stops when the last one
//! leaves. Each user gets every frame from the moment it joined.
//!
//! Dictation stays first:
//!
//! - A dictation gets its frames on the thread of the capture, in the same
//!   call as before this module ([`SharedMic::start`]). When a live session
//!   already captures, a dictation gets frames at once, without the time to
//!   open the device.
//! - A live session gets its frames through a queue of its own
//!   ([`SharedMic::join_queued`]). Sending to it never waits, so a slow live
//!   session does not delay or drop a frame of the dictation.
//!
//! The device: a user that joins names the device of the setting. When the
//! capture runs on another device (the user changed the microphone in Settings
//! during a live session), the capture moves to the new device for all its
//! users: the new device starts, and then the old one stops. The frames of the
//! old device go to the users until the new device is open, so a live session
//! loses only the frames between the two, and never gets frames of both.
//! A change of the setting alone does not move the capture; the next user does.

use parking_lot::Mutex;
use sayso_platform::{AudioCapture, AudioFrame, CaptureHandle, Result};
use std::sync::Arc;

/// A user of the shared microphone with a queue: the lease, and the frames.
pub type Queued = (Box<dyn CaptureHandle>, crossbeam_channel::Receiver<Vec<f32>>);

pub struct SharedMic {
    audio: Arc<dyn AudioCapture>,
    /// Joins and leaves, one at a time. The capture thread never takes it.
    capture: Mutex<Option<Running>>,
    /// The users. The capture thread takes it for each frame.
    users: Arc<Mutex<Users>>,
}

struct Running {
    handle: Box<dyn CaptureHandle>,
    device: Option<String>,
}

#[derive(Default)]
struct Users {
    list: Vec<(u64, Sink)>,
    next: u64,
    /// Only the capture of this generation gives frames. A capture that is
    /// replaced stops giving frames at once, also before it stops.
    generation: u64,
}

enum Sink {
    /// Runs on the capture thread. For dictation, which must not wait.
    Inline(Box<dyn FnMut(AudioFrame) + Send>),
    /// A queue that never blocks the capture thread. For live sessions.
    Queue(crossbeam_channel::Sender<Vec<f32>>),
}

impl SharedMic {
    pub fn new(audio: Arc<dyn AudioCapture>) -> Arc<SharedMic> {
        Arc::new(SharedMic { audio, capture: Mutex::new(None), users: Arc::default() })
    }

    /// Join with `on_frame`, which runs on the capture thread (dictation).
    /// The handle leaves. Fails when the device does not start.
    pub fn start(self: &Arc<Self>, device: Option<&str>, on_frame: Box<dyn FnMut(AudioFrame) + Send>) -> Result<Box<dyn CaptureHandle>> {
        let id = self.join(device, Sink::Inline(on_frame))?;
        Ok(Box::new(Lease { mic: Some(self.clone()), id }))
    }

    /// Join with a queue (live sessions). The frames come in order and never
    /// wait for the receiver; dropping the lease leaves, and the receiver
    /// ends after the last frame.
    // The live sessions of the host use it when 6d is wired.
    #[allow(dead_code)]
    pub fn join_queued(self: &Arc<Self>, device: Option<&str>) -> Result<Queued> {
        let (tx, rx) = crossbeam_channel::unbounded();
        let id = self.join(device, Sink::Queue(tx))?;
        Ok((Box::new(Lease { mic: Some(self.clone()), id }), rx))
    }

    /// True while the microphone captures for some user.
    #[allow(dead_code)]
    pub fn is_capturing(&self) -> bool {
        self.capture.lock().is_some()
    }

    fn join(&self, device: Option<&str>, sink: Sink) -> Result<u64> {
        let mut capture = self.capture.lock();
        let id = {
            let mut users = self.users.lock();
            users.next += 1;
            let id = users.next;
            users.list.push((id, sink));
            id
        };
        let same_device = capture.as_ref().is_some_and(|running| running.device.as_deref() == device);
        if same_device {
            return Ok(id);
        }
        // A first capture gives frames from its first frame. A capture that
        // replaces one gives frames once it started, and the old one stops then.
        let replacing = capture.is_some();
        let generation = {
            let mut users = self.users.lock();
            let generation = users.generation + 1;
            if !replacing {
                users.generation = generation;
            }
            generation
        };
        match self.audio.start(device, self.fan_out(generation)) {
            Ok(handle) => {
                self.users.lock().generation = generation;
                if let Some(old) = capture.replace(Running { handle, device: device.map(str::to_owned) }) {
                    log::info!("microphone: moved to {}", device.unwrap_or("the default device"));
                    old.handle.stop();
                }
                Ok(id)
            }
            Err(e) => {
                let mut users = self.users.lock();
                users.list.retain(|(user, _)| *user != id);
                if !replacing {
                    users.generation -= 1;
                }
                Err(e)
            }
        }
    }

    fn leave(&self, id: u64) {
        let mut capture = self.capture.lock();
        let empty = {
            let mut users = self.users.lock();
            users.list.retain(|(user, _)| *user != id);
            users.list.is_empty()
        };
        if empty && let Some(running) = capture.take() {
            running.handle.stop();
        }
    }

    /// The callback of the capture of `generation`: each user gets the frame,
    /// dictation first.
    fn fan_out(&self, generation: u64) -> Box<dyn FnMut(AudioFrame) + Send> {
        let users = self.users.clone();
        Box::new(move |frame: AudioFrame| {
            let mut users = users.lock();
            if users.generation != generation {
                return;
            }
            for (_, sink) in users.list.iter_mut() {
                if let Sink::Inline(on_frame) = sink {
                    on_frame(frame.clone());
                }
            }
            for (_, sink) in users.list.iter() {
                if let Sink::Queue(queue) = sink {
                    // A user that dropped its receiver leaves with its lease.
                    let _ = queue.send(frame.samples.clone());
                }
            }
        })
    }
}

/// One user of the shared microphone. Stopping or dropping it leaves.
struct Lease {
    mic: Option<Arc<SharedMic>>,
    id: u64,
}

impl CaptureHandle for Lease {
    fn stop(mut self: Box<Self>) {
        if let Some(mic) = self.mic.take() {
            mic.leave(self.id);
        }
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(mic) = self.mic.take() {
            mic.leave(self.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sayso_platform::{AudioDevice, PlatformError};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    type Callback = Box<dyn FnMut(AudioFrame) + Send>;

    /// A microphone whose frames the test sends, as the capture thread does.
    #[derive(Default)]
    struct FakeAudio {
        running: Mutex<Vec<Capture>>,
        starts: Mutex<Vec<Option<String>>>,
        stops: Mutex<usize>,
        fail: AtomicBool,
    }

    /// A capture that started: its number from 1, its callback, and its stop.
    struct Capture(usize, Arc<Mutex<Callback>>, Arc<AtomicBool>);

    struct FakeHandle(Arc<AtomicBool>, Arc<FakeAudio>);

    impl CaptureHandle for FakeHandle {
        fn stop(self: Box<Self>) {
            self.0.store(true, Ordering::SeqCst);
            *self.1.stops.lock() += 1;
        }
    }

    struct Audio(Arc<FakeAudio>);

    impl AudioCapture for Audio {
        fn devices(&self) -> Vec<AudioDevice> {
            Vec::new()
        }

        fn start(&self, device_id: Option<&str>, on_frame: Callback) -> Result<Box<dyn CaptureHandle>> {
            if self.0.fail.load(Ordering::SeqCst) {
                return Err(PlatformError::Failed("no device".into()));
            }
            let stopped = Arc::new(AtomicBool::new(false));
            let mut starts = self.0.starts.lock();
            starts.push(device_id.map(str::to_owned));
            self.0.running.lock().push(Capture(starts.len(), Arc::new(Mutex::new(on_frame)), stopped.clone()));
            Ok(Box::new(FakeHandle(stopped, self.0.clone())))
        }
    }

    impl FakeAudio {
        /// One frame from each capture that did not stop, with the value `value`.
        fn frame(&self, value: f32) {
            let running: Vec<_> = self.running.lock().iter().filter(|r| !r.2.load(Ordering::SeqCst)).map(|r| r.1.clone()).collect();
            for callback in running {
                (callback.lock())(AudioFrame { samples: vec![value; 320], level: 0.0 });
            }
        }

        /// A frame from the capture that started as number `start` (from 1), also after its stop.
        fn frame_from(&self, start: usize, value: f32) {
            let callback = self.running.lock().iter().find(|r| r.0 == start).map(|r| r.1.clone()).unwrap();
            (callback.lock())(AudioFrame { samples: vec![value; 320], level: 0.0 });
        }

        fn live(&self) -> usize {
            self.running.lock().iter().filter(|r| !r.2.load(Ordering::SeqCst)).count()
        }
    }

    fn setup() -> (Arc<FakeAudio>, Arc<SharedMic>) {
        let fake = Arc::new(FakeAudio::default());
        let mic = SharedMic::new(Arc::new(Audio(fake.clone())));
        (fake, mic)
    }

    /// A dictation user that keeps the first sample of each frame.
    fn dictation() -> (Arc<Mutex<Vec<f32>>>, Callback) {
        let got = Arc::new(Mutex::new(Vec::new()));
        let sink = got.clone();
        (got, Box::new(move |frame: AudioFrame| sink.lock().push(frame.samples[0])))
    }

    fn firsts(rx: &crossbeam_channel::Receiver<Vec<f32>>) -> Vec<f32> {
        rx.try_iter().map(|samples| samples[0]).collect()
    }

    #[test]
    fn two_users_share_one_capture_and_each_gets_the_frames_from_its_join() {
        let (fake, mic) = setup();
        let (live, frames) = mic.join_queued(None).unwrap();
        fake.frame(1.0);
        let (got, on_frame) = dictation();
        let dictating = mic.start(None, on_frame).unwrap();
        assert_eq!(fake.starts.lock().len(), 1, "the dictation joins the capture that runs");
        fake.frame(2.0);
        fake.frame(3.0);
        assert_eq!(*got.lock(), vec![2.0, 3.0], "the late user gets the frames from its join");
        dictating.stop();
        fake.frame(4.0);
        assert_eq!(*got.lock(), vec![2.0, 3.0], "no frame after the leave");
        assert_eq!(fake.live(), 1, "the live session still captures");
        assert_eq!(firsts(&frames), vec![1.0, 2.0, 3.0, 4.0]);
        live.stop();
        assert_eq!(fake.live(), 0, "the last leave stops the capture");
        assert!(frames.recv_timeout(Duration::from_millis(10)).is_err(), "the queue ends");
        assert!(!mic.is_capturing());
    }

    #[test]
    fn the_capture_stops_when_the_last_user_leaves_and_starts_again() {
        let (fake, mic) = setup();
        let (_, on_frame) = dictation();
        let first = mic.start(None, on_frame).unwrap();
        let (second, _frames) = mic.join_queued(None).unwrap();
        drop(first);
        assert_eq!(*fake.stops.lock(), 0);
        drop(second);
        assert_eq!(*fake.stops.lock(), 1);
        let (got, on_frame) = dictation();
        let _third = mic.start(None, on_frame).unwrap();
        assert_eq!(fake.starts.lock().len(), 2);
        fake.frame(5.0);
        assert_eq!(*got.lock(), vec![5.0]);
    }

    #[test]
    fn a_slow_live_session_does_not_block_the_dictation() {
        let (fake, mic) = setup();
        // The live session never reads its queue.
        let (_live, _frames) = mic.join_queued(None).unwrap();
        let (got, on_frame) = dictation();
        let _dictating = mic.start(None, on_frame).unwrap();
        let started = Instant::now();
        for i in 0..5_000 {
            fake.frame(i as f32);
        }
        assert_eq!(got.lock().len(), 5_000, "the dictation got every frame");
        assert!(started.elapsed() < Duration::from_secs(2), "the frames did not wait: {:?}", started.elapsed());
    }

    #[test]
    fn a_device_that_does_not_start_leaves_no_user() {
        let (fake, mic) = setup();
        fake.fail.store(true, Ordering::SeqCst);
        let (_, on_frame) = dictation();
        assert!(mic.start(None, on_frame).is_err());
        assert!(mic.users.lock().list.is_empty());
        fake.fail.store(false, Ordering::SeqCst);
        let (got, on_frame) = dictation();
        let _dictating = mic.start(None, on_frame).unwrap();
        fake.frame(1.0);
        assert_eq!(*got.lock(), vec![1.0]);
    }

    #[test]
    fn a_new_device_moves_the_capture_of_every_user() {
        let (fake, mic) = setup();
        let (_live, frames) = mic.join_queued(Some("built-in")).unwrap();
        fake.frame(1.0);
        // The user chose a headset in Settings; the next dictation names it.
        let (got, on_frame) = dictation();
        let _dictating = mic.start(Some("headset"), on_frame).unwrap();
        assert_eq!(*fake.starts.lock(), vec![Some("built-in".to_string()), Some("headset".to_string())]);
        assert_eq!(fake.live(), 1, "the old device stopped");
        // A late frame of the old device goes to nobody.
        fake.frame_from(1, 9.0);
        fake.frame(2.0);
        assert_eq!(*got.lock(), vec![2.0]);
        assert_eq!(firsts(&frames), vec![1.0, 2.0], "the live session moved too");

        // A new device that does not start keeps the old one.
        fake.fail.store(true, Ordering::SeqCst);
        let (_, on_frame) = dictation();
        assert!(mic.start(Some("usb"), on_frame).is_err());
        fake.frame(3.0);
        assert_eq!(*got.lock(), vec![2.0, 3.0]);
        assert_eq!(firsts(&frames), vec![3.0]);
    }
}
