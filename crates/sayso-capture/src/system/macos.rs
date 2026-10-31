//! A Core Audio process tap through an aggregate device (macOS 14.2 and later).
//!
//! The tap is a stereo mix of all output, without the excluded processes. The
//! aggregate device is private to this process, and it changes no setting of
//! the user. The tap goes away with the process or with [`TapStream`].
//!
//! The permission: there is no public call for it, and the calls of Core Audio
//! do not fail when it is refused: the buffers are silent. So this uses
//! `TCCAccessPreflight` and `TCCAccessRequest` of the private TCC framework,
//! loaded at run time, as `AudioCap` does. When they are not there, the capture
//! runs, and the system asks at `AudioDeviceStart`. That call does not return
//! until the user answers.

use super::{Access, Backend, Error, FrameSink, Options, Stream};
use crate::mic::{Pipeline, RawFrame};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2::AnyThread;
use objc2_core_audio::{
    AudioDeviceCreateIOProcID, AudioDeviceDestroyIOProcID, AudioDeviceID, AudioDeviceIOProcID, AudioDeviceStart, AudioDeviceStop, AudioHardwareCreateAggregateDevice, AudioHardwareCreateProcessTap,
    AudioHardwareDestroyAggregateDevice, AudioHardwareDestroyProcessTap, AudioObjectGetPropertyData, AudioObjectID, AudioObjectPropertyAddress, AudioObjectSetPropertyData, CATapDescription, CATapMuteBehavior,
    kAudioAggregateDeviceIsPrivateKey, kAudioAggregateDeviceNameKey, kAudioAggregateDeviceTapAutoStartKey, kAudioAggregateDeviceTapListKey, kAudioAggregateDeviceUIDKey, kAudioHardwarePropertyDefaultOutputDevice,
    kAudioHardwarePropertyTranslatePIDToProcessObject, kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject, kAudioObjectUnknown, kAudioSubTapDriftCompensationKey,
    kAudioSubTapUIDKey, kAudioTapPropertyDescription, kAudioTapPropertyFormat,
};
use objc2_core_audio_types::{AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp, kAudioFormatFlagIsFloat, kAudioFormatLinearPCM};
use objc2_core_foundation::{CFDictionary, CFRetained, CFString};
use objc2_foundation::{NSArray, NSBundle, NSDictionary, NSMutableDictionary, NSNumber, NSOperatingSystemVersion, NSProcessInfo, NSString, NSObject};
use std::ffi::{CStr, c_void};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

/// The aggregate devices of this process need names that differ.
static NEXT_DEVICE: AtomicU32 = AtomicU32::new(0);

/// How long the start of a capture waits for the answer of the user.
const QUESTION_WAIT: Duration = Duration::from_secs(120);

/// The first macOS with process taps.
fn tap_supported(version: (isize, isize)) -> bool {
    version >= (14, 2)
}

fn access_from(status: i32) -> Access {
    match status {
        0 => Access::Granted,
        1 => Access::Denied,
        _ => Access::Unknown,
    }
}

type Preflight = unsafe extern "C" fn(*const c_void, *const c_void) -> i32;
type Request = unsafe extern "C" fn(*const c_void, *const c_void, *const c_void);

/// This macOS can make a process tap. It asks nothing of the user.
pub(super) fn supported() -> bool {
    let NSOperatingSystemVersion { majorVersion, minorVersion, .. } = NSProcessInfo::processInfo().operatingSystemVersion();
    tap_supported((majorVersion, minorVersion))
}

/// A symbol of the private TCC framework.
fn tcc_symbol(name: &CStr) -> Option<*mut c_void> {
    const PATH: &CStr = c"/System/Library/PrivateFrameworks/TCC.framework/Versions/A/TCC";
    // SAFETY: both strings end with a NUL. The library stays loaded for the process.
    let symbol = unsafe {
        let handle = libc::dlopen(PATH.as_ptr(), libc::RTLD_NOW);
        if handle.is_null() {
            return None;
        }
        libc::dlsym(handle, name.as_ptr())
    };
    (!symbol.is_null()).then_some(symbol)
}

fn service() -> CFRetained<CFString> {
    CFString::from_str("kTCCServiceAudioCapture")
}

pub(super) fn preflight() -> Access {
    let Some(symbol) = tcc_symbol(c"TCCAccessPreflight") else { return Access::Unknown };
    // SAFETY: the symbol has this signature in every macOS that has it; the
    // options can be null, and the service lives for the call.
    let status = unsafe {
        let call: Preflight = std::mem::transmute(symbol);
        call(CFRetained::as_ptr(&service()).as_ptr().cast(), std::ptr::null())
    };
    access_from(status)
}

/// Ask the user, and wait for the answer. None when it cannot ask.
fn request() -> Option<bool> {
    let symbol = tcc_symbol(c"TCCAccessRequest")?;
    let (tx, rx) = crossbeam_channel::bounded(1);
    let block = RcBlock::new(move |granted: Bool| {
        let _ = tx.try_send(granted.as_bool());
    });
    // SAFETY: as in `preflight`. TCC copies the block, and it can run on any queue.
    unsafe {
        let call: Request = std::mem::transmute(symbol);
        call(CFRetained::as_ptr(&service()).as_ptr().cast(), std::ptr::null(), RcBlock::as_ptr(&block).cast());
    }
    Some(rx.recv_timeout(QUESTION_WAIT).unwrap_or(false))
}

/// A program without the usage text in its `Info.plist` must not ask: macOS can
/// end the process, as for the microphone.
fn has_usage_text() -> bool {
    NSBundle::mainBundle().objectForInfoDictionaryKey(&NSString::from_str("NSAudioCaptureUsageDescription")).is_some()
}

fn address(selector: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress { mSelector: selector, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain }
}

fn check(status: i32, what: &str) -> Result<(), Error> {
    if status == 0 { Ok(()) } else { Err(Error::Failed(format!("{what}: Core Audio error {status}"))) }
}

/// The default output device.
fn default_output() -> Option<AudioDeviceID> {
    let property = address(kAudioHardwarePropertyDefaultOutputDevice);
    let mut device: AudioDeviceID = kAudioObjectUnknown;
    let mut size = size_of::<AudioDeviceID>() as u32;
    // SAFETY: the pointers live for the call, and the size is the size of `device`.
    let status = unsafe { AudioObjectGetPropertyData(kAudioObjectSystemObject as AudioObjectID, NonNull::from(&property), 0, std::ptr::null(), NonNull::from(&mut size), NonNull::from(&mut device).cast()) };
    (status == 0 && device != kAudioObjectUnknown).then_some(device)
}

/// The audio object of a process. A process has one after its first use of Core Audio.
fn process_object(pid: u32) -> Option<AudioObjectID> {
    let property = address(kAudioHardwarePropertyTranslatePIDToProcessObject);
    let mut object: AudioObjectID = kAudioObjectUnknown;
    let mut size = size_of::<AudioObjectID>() as u32;
    let pid = pid as libc::pid_t;
    // SAFETY: the pointers live for the call, and the sizes are of `pid` and `object`.
    let status = unsafe { AudioObjectGetPropertyData(kAudioObjectSystemObject as AudioObjectID, NonNull::from(&property), size_of::<libc::pid_t>() as u32, (&raw const pid).cast(), NonNull::from(&mut size), NonNull::from(&mut object).cast()) };
    (status == 0 && object != kAudioObjectUnknown).then_some(object)
}

fn tap_format(tap: AudioObjectID) -> Result<AudioStreamBasicDescription, Error> {
    let property = address(kAudioTapPropertyFormat);
    // SAFETY: an all-zero description is valid plain data.
    let mut format: AudioStreamBasicDescription = unsafe { std::mem::zeroed() };
    let mut size = size_of::<AudioStreamBasicDescription>() as u32;
    // SAFETY: the pointers live for the call, and the size is the size of `format`.
    let status = unsafe { AudioObjectGetPropertyData(tap, NonNull::from(&property), 0, std::ptr::null(), NonNull::from(&mut size), NonNull::from(&mut format).cast()) };
    check(status, "the format of the tap")?;
    Ok(format)
}

/// The array of the process objects, in the type of the tap description.
fn process_array(objects: &[AudioObjectID]) -> Retained<NSArray<NSNumber>> {
    let numbers: Vec<Retained<NSNumber>> = objects.iter().map(|&object| NSNumber::new_u32(object)).collect();
    NSArray::from_retained_slice(&numbers)
}

fn key(name: &CStr) -> Retained<NSString> {
    NSString::from_str(&name.to_string_lossy())
}

/// The description of an aggregate device with only this tap, private to this process.
fn aggregate_description(tap_uid: &NSString, uid: &str, name: &str) -> CFRetained<CFDictionary> {
    let yes = NSNumber::new_bool(true);
    let tap: Retained<NSDictionary<NSString, NSObject>> = NSDictionary::from_retained_objects(
        &[&*key(kAudioSubTapUIDKey), &*key(kAudioSubTapDriftCompensationKey)],
        &[Retained::into_super(NSString::from_str(&tap_uid.to_string())), Retained::into_super(Retained::into_super(yes.clone()))],
    );
    let taps = NSArray::from_retained_slice(&[tap]);
    let description: Retained<NSMutableDictionary<NSString, NSObject>> = NSMutableDictionary::new();
    let set = |name: &CStr, value: &NSObject| {
        // SAFETY: the key type is `NSString`, which copies safely, and the values are property list objects.
        unsafe { description.setObject_forKey(value, objc2::runtime::ProtocolObject::from_ref(&*key(name))) };
    };
    set(kAudioAggregateDeviceUIDKey, &NSString::from_str(uid));
    set(kAudioAggregateDeviceNameKey, &NSString::from_str(name));
    set(kAudioAggregateDeviceIsPrivateKey, &yes);
    set(kAudioAggregateDeviceTapAutoStartKey, &yes);
    set(kAudioAggregateDeviceTapListKey, &taps);
    // SAFETY: `NSDictionary` and `CFDictionary` are toll-free bridged.
    unsafe { CFRetained::from_raw(NonNull::new_unchecked(Retained::into_raw(description)).cast()) }
}

/// What the IO function owns.
struct IoState<F: FnMut(&RawFrame)> {
    pipeline: Pipeline<F>,
    channels: usize,
    interleaved: Vec<f32>,
}

impl<F: FnMut(&RawFrame)> IoState<F> {
    /// The buffers of one call. One buffer is interleaved; else each buffer has one channel.
    fn buffers(&mut self, buffers: &[&[f32]]) {
        match buffers {
            [] => {}
            [only] => self.pipeline.push(only),
            several => {
                let frames = several.iter().map(|buffer| buffer.len()).min().unwrap_or(0);
                self.interleaved.clear();
                self.interleaved.reserve(frames * self.channels);
                for frame in 0..frames {
                    self.interleaved.extend(several.iter().map(|buffer| buffer[frame]));
                }
                let IoState { pipeline, interleaved, .. } = self;
                pipeline.push(interleaved.as_slice());
            }
        }
    }
}

type SinkFn = Box<dyn FnMut(&RawFrame) + Send>;

/// The audio thread of the aggregate device.
unsafe extern "C-unwind" fn io_proc(_device: AudioObjectID, _now: NonNull<AudioTimeStamp>, input: NonNull<AudioBufferList>, _input_time: NonNull<AudioTimeStamp>, _output: NonNull<AudioBufferList>, _output_time: NonNull<AudioTimeStamp>, client: *mut c_void) -> i32 {
    // SAFETY: `client` is the `IoState` of `TapStream`, which lives until the
    // IO function is destroyed, and Core Audio calls it from one thread at a time.
    let state = unsafe { &mut *client.cast::<IoState<SinkFn>>() };
    // SAFETY: Core Audio gives a valid list with `mNumberBuffers` buffers for this call.
    let list = unsafe { input.as_ref() };
    let first = list.mBuffers.as_ptr();
    let mut slices: [&[f32]; 16] = [&[]; 16];
    let count = (list.mNumberBuffers as usize).min(slices.len());
    for (i, slice) in slices.iter_mut().enumerate().take(count) {
        // SAFETY: the list has at least `count` buffers, and each holds `mDataByteSize`
        // bytes of the float format that `open` checked.
        unsafe {
            let buffer = &*first.add(i);
            if !buffer.mData.is_null() {
                *slice = std::slice::from_raw_parts(buffer.mData.cast::<f32>(), buffer.mDataByteSize as usize / size_of::<f32>());
            }
        }
    }
    state.buffers(&slices[..count]);
    0
}

pub(super) struct Tap {
    options: Options,
}

impl Tap {
    pub(super) fn new(options: Options) -> Tap {
        Tap { options }
    }
}

struct TapStream {
    tap: AudioObjectID,
    aggregate: AudioDeviceID,
    proc_id: AudioDeviceIOProcID,
    state: *mut IoState<SinkFn>,
    description: Retained<CATapDescription>,
    /// The processes to leave out, with the object when they have one.
    processes: Vec<(u32, Option<AudioObjectID>)>,
    device: Option<AudioDeviceID>,
    sample_rate: f64,
}

// SAFETY: the raw pointer is used by the audio thread and by `drop` after the
// IO function ended. `description` is only used on the thread that owns this
// stream, and a `CATapDescription` has no thread affinity.
unsafe impl Send for TapStream {}

impl TapStream {
    /// A process to leave out can get its audio object after the tap started.
    fn update_excludes(&mut self) {
        let mut changed = false;
        for (pid, object) in &mut self.processes {
            if object.is_none() {
                *object = process_object(*pid);
                changed |= object.is_some();
            }
        }
        if !changed {
            return;
        }
        let objects: Vec<AudioObjectID> = self.processes.iter().filter_map(|process| process.1).collect();
        let property = address(kAudioTapPropertyDescription);
        // SAFETY: the description is valid, and the property takes a reference to one.
        let status = unsafe {
            self.description.setProcesses(&process_array(&objects));
            let reference: *const CATapDescription = &*self.description;
            AudioObjectSetPropertyData(self.tap, NonNull::from(&property), 0, std::ptr::null(), size_of::<*const CATapDescription>() as u32, NonNull::from(&reference).cast())
        };
        if status != 0 {
            log::warn!("system audio: cannot change the processes that the tap leaves out: Core Audio error {status}");
        }
    }
}

impl Stream for TapStream {
    fn still_good(&mut self) -> bool {
        self.update_excludes();
        default_output() == self.device && tap_format(self.tap).is_ok_and(|format| format.mSampleRate == self.sample_rate)
    }
}

impl Drop for TapStream {
    fn drop(&mut self) {
        // SAFETY: the ids come from `open`. After `AudioDeviceDestroyIOProcID` no call of
        // the IO function runs, so the state can go. The status is of no use in a drop.
        unsafe {
            AudioDeviceStop(self.aggregate, self.proc_id);
            AudioDeviceDestroyIOProcID(self.aggregate, self.proc_id);
            drop(Box::from_raw(self.state));
            AudioHardwareDestroyAggregateDevice(self.aggregate);
            AudioHardwareDestroyProcessTap(self.tap);
        }
    }
}

/// What `open` made, so that an error on the way closes it.
struct Partial {
    tap: AudioObjectID,
    aggregate: Option<AudioDeviceID>,
}

impl Drop for Partial {
    fn drop(&mut self) {
        // SAFETY: the ids come from `open`, and no IO function runs on them.
        unsafe {
            if let Some(aggregate) = self.aggregate {
                AudioHardwareDestroyAggregateDevice(aggregate);
            }
            AudioHardwareDestroyProcessTap(self.tap);
        }
    }
}

impl Backend for Tap {
    fn prepare(&mut self) -> Result<(), Error> {
        let version = NSProcessInfo::processInfo().operatingSystemVersion();
        let NSOperatingSystemVersion { majorVersion, minorVersion, .. } = version;
        if !tap_supported((majorVersion, minorVersion)) {
            return Err(Error::Unsupported(format!("the system audio needs macOS 14.2 or later, and this is {majorVersion}.{minorVersion}")));
        }
        match preflight() {
            Access::Granted => Ok(()),
            Access::Denied => Err(Error::PermissionDenied),
            Access::Unknown if !has_usage_text() => {
                log::warn!("system audio: this program has no NSAudioCaptureUsageDescription, so it does not ask");
                Ok(())
            }
            Access::Unknown => match request() {
                Some(false) => Err(Error::PermissionDenied),
                Some(true) | None => Ok(()),
            },
        }
    }

    fn permission_held(&self) -> bool {
        preflight() != Access::Denied
    }

    fn open(&mut self, mut sink: FrameSink) -> Result<Box<dyn Stream>, Error> {
        let processes: Vec<(u32, Option<AudioObjectID>)> = self.options.exclude.iter().map(|process| (process.pid, process_object(process.pid))).collect();
        let objects: Vec<AudioObjectID> = processes.iter().filter_map(|process| process.1).collect();
        let device = default_output();
        // SAFETY: a new object with a valid array. `setPrivate` and the others only store their value.
        let description = unsafe {
            let description = CATapDescription::initStereoGlobalTapButExcludeProcesses(CATapDescription::alloc(), &process_array(&objects));
            description.setPrivate(true);
            description.setMuteBehavior(CATapMuteBehavior::Unmuted);
            description.setName(&NSString::from_str("Sayso system audio"));
            description
        };
        let mut tap: AudioObjectID = kAudioObjectUnknown;
        // SAFETY: the description is valid, and `tap` lives for the call.
        check(unsafe { AudioHardwareCreateProcessTap(Some(&description), &raw mut tap) }, "the process tap")?;
        let mut partial = Partial { tap, aggregate: None };

        let number = NEXT_DEVICE.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        // SAFETY: `UUID` and `UUIDString` only read the description.
        let tap_uid = unsafe { description.UUID().UUIDString() };
        let aggregate_description = aggregate_description(&tap_uid, &format!("dev.sayso.system-audio.{pid}.{number}"), &format!("Sayso system audio {pid}.{number}"));
        let mut aggregate: AudioDeviceID = kAudioObjectUnknown;
        // SAFETY: the dictionary is valid, and `aggregate` lives for the call.
        check(unsafe { AudioHardwareCreateAggregateDevice(&aggregate_description, NonNull::from(&mut aggregate)) }, "the aggregate device")?;
        partial.aggregate = Some(aggregate);

        let format = tap_format(tap)?;
        if format.mFormatID != kAudioFormatLinearPCM || format.mFormatFlags & kAudioFormatFlagIsFloat == 0 || format.mBitsPerChannel != 32 {
            return Err(Error::Failed(format!("the tap has the format {} with {} bits, not 32-bit float", format.mFormatID, format.mBitsPerChannel)));
        }
        let channels = format.mChannelsPerFrame.max(1) as u16;
        let on_frame: SinkFn = Box::new(move |frame: &RawFrame| sink.push(frame));
        let state = Box::into_raw(Box::new(IoState { pipeline: Pipeline::new(channels, format.mSampleRate as u32, on_frame), channels: usize::from(channels), interleaved: Vec::new() }));
        let mut proc_id: AudioDeviceIOProcID = None;
        // SAFETY: `state` lives until the IO function is destroyed.
        let created = check(unsafe { AudioDeviceCreateIOProcID(aggregate, Some(io_proc), state.cast(), NonNull::from(&mut proc_id)) }, "the IO function");
        // SAFETY: both ids are valid when the creation worked. The start is the moment when macOS asks for the permission.
        let started = created.and_then(|()| check(unsafe { AudioDeviceStart(aggregate, proc_id) }, "the start of the device"));
        if let Err(error) = started {
            // SAFETY: no IO function ran. Destroying a null one fails harmlessly.
            unsafe {
                AudioDeviceDestroyIOProcID(aggregate, proc_id);
                drop(Box::from_raw(state));
            }
            return Err(error);
        }
        // `TapStream` closes them from now on.
        std::mem::forget(partial);
        Ok(Box::new(TapStream { tap, aggregate, proc_id, state, description, processes, device, sample_rate: format.mSampleRate }))
    }

    fn excludes(&self) -> Vec<String> {
        self.options.exclude.iter().map(|process| process.name.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taps_need_macos_14_2() {
        assert!(!tap_supported((13, 7)));
        assert!(!tap_supported((14, 0)));
        assert!(!tap_supported((14, 1)));
        assert!(tap_supported((14, 2)));
        assert!(tap_supported((15, 0)));
        assert!(tap_supported((27, 0)));
    }

    #[test]
    fn the_answers_of_tcc() {
        assert_eq!(access_from(0), Access::Granted);
        assert_eq!(access_from(1), Access::Denied);
        assert_eq!(access_from(2), Access::Unknown);
    }

    #[test]
    fn deinterleaved_buffers_become_one_interleaved_buffer() {
        let frames = std::sync::Arc::new(parking_lot::Mutex::new(Vec::<f32>::new()));
        let sink = frames.clone();
        let on_frame: SinkFn = Box::new(move |frame: &RawFrame| sink.lock().extend_from_slice(frame));
        let mut state = IoState { pipeline: Pipeline::new(2, crate::mic::TARGET_RATE, on_frame), channels: 2, interleaved: Vec::new() };
        let (left, right) = (vec![1.0f32; 320], vec![0.0f32; 320]);
        state.buffers(&[&left, &right]);
        assert_eq!(frames.lock().as_slice(), &[0.5; 320][..], "the mix of the two channels");
    }
}
