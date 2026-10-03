//! [`TextInserter`]: put text into the focused field of the frontmost app.
//!
//! Two methods:
//! - **Paste**: save the clipboard, publish our text with delayed rendering,
//!   send Ctrl+V, and wait until an app reads the text. Then restore the
//!   clipboard if nobody changed it meanwhile. With no read, the paste
//!   failed, and the text stays on the clipboard.
//! - **Type**: send Unicode key events with `SendInput`.
//!
//! Delayed rendering is the read receipt. Sayso puts the text format on the
//! clipboard with no data. When an app reads it, Windows sends
//! `WM_RENDERFORMAT` to the clipboard owner, a message-only window on a
//! thread of its own, and the owner hands over the text. Windows sends the
//! message once: later reads get the stored text without a message.
//!
//! Windows drops synthetic keys sent to an app that runs at a higher
//! integrity level (an app run as administrator), and gives no error. So the
//! check runs first. With no foreground window (the lock screen, a UAC
//! prompt) there is nowhere to type.

use crate::context::{self, Integrity};
use crate::input;
use crate::paste_receipt::{Receipts, Verdict};
use crate::win32::wide;
use crossbeam_channel::Sender;
use parking_lot::Mutex;
use sayso_platform::{InsertError, InsertMethod, InsertResult, TextInserter};
use std::sync::{Arc, OnceLock};
use std::thread::sleep;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData, GetClipboardOwner,
    GetClipboardSequenceNumber, GetOpenClipboardWindow, IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::SystemServices::SECURITY_MANDATORY_HIGH_RID;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetForegroundWindow, GetMessageW, HWND_MESSAGE, MSG,
    PM_NOREMOVE, PeekMessageW, PostThreadMessageW, RegisterClassW, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE,
    WM_APP, WM_DESTROYCLIPBOARD, WM_RENDERALLFORMATS, WM_RENDERFORMAT, WNDCLASSW,
};
use windows::core::PCWSTR;

/// How often the paste looks for a read of the text.
const RECEIPT_POLL: Duration = Duration::from_millis(10);
/// UTF-16 units per `SendInput` call when typing.
pub const MAX_UNITS_PER_EVENT: usize = 20;
const CHUNK_DELAY: Duration = Duration::from_millis(5);
const KEY_GAP: Duration = Duration::from_millis(8);
/// Another app may hold the clipboard open for a moment.
const OPEN_TRIES: u32 = 10;
const OPEN_RETRY: Duration = Duration::from_millis(15);
/// The longest wait for the clipboard thread to answer.
const OWNER_TIMEOUT: Duration = Duration::from_secs(5);

/// `CF_UNICODETEXT`.
pub const CF_UNICODETEXT: u32 = 13;

/// Registered formats that tell clipboard history, the cloud clipboard, and
/// clipboard managers to skip an entry: the Windows version of the macOS
/// transient type. Each holds a DWORD 0.
const SKIP_FORMATS: [&str; 4] = [
    "ExcludeClipboardContentFromMonitorProcessing",
    "CanIncludeInClipboardHistory",
    "CanUploadToCloudClipboard",
    "Clipboard Viewer Ignore",
];

pub struct WinInserter;

/// Split text into UTF-16 chunks of at most `max_units` units without cutting
/// a surrogate pair. `max_units` must be at least 2, so every char fits.
pub fn utf16_chunks(text: &str, max_units: usize) -> Vec<Vec<u16>> {
    assert!(max_units >= 2, "a chunk must hold one surrogate pair");
    let mut chunks = Vec::new();
    let mut current: Vec<u16> = Vec::new();
    let mut buf = [0u16; 2];
    for ch in text.chars() {
        let units = ch.encode_utf16(&mut buf);
        if current.len() + units.len() > max_units {
            chunks.push(std::mem::take(&mut current));
        }
        current.extend_from_slice(units);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

// ---------------------------------------------------------------------------
// Preconditions
// ---------------------------------------------------------------------------

/// Whether Windows drops keys from a process at `own` to one at `target`.
///
/// A level Windows refuses to show is a higher one. Sayso itself at high
/// level (run as administrator) reaches every normal app.
pub fn integrity_blocks(own: Integrity, target: Integrity) -> bool {
    match (own, target) {
        (Integrity::Level(own), Integrity::Level(target)) => target > own,
        (Integrity::Level(own), Integrity::Hidden) => own < SECURITY_MANDATORY_HIGH_RID as u32,
        _ => false,
    }
}

/// The message for an app that runs as administrator.
pub fn elevated_message(app: &str) -> String {
    format!("{app} runs as administrator, so Windows does not let Sayso type into it.")
}

/// Check that a window is in front and that Windows lets Sayso send keys to it.
fn preflight() -> Result<(), InsertError> {
    // SAFETY: plain query.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_invalid() {
        return Err(InsertError::NoFocusedField);
    }
    let pid = context::window_pid(hwnd);
    if pid == 0 {
        return Err(InsertError::NoFocusedField);
    }
    if integrity_blocks(context::own_integrity(), context::process_integrity(pid)) {
        let name = context::app_name_for_pid(pid).unwrap_or_else(|| "This app".to_string());
        return Err(InsertError::Failed(elevated_message(&name)));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Paste with a read receipt
// ---------------------------------------------------------------------------

/// Every clipboard format that holds plain memory, with its bytes.
pub type Snapshot = Vec<(u32, Vec<u8>)>;

/// Whether a clipboard format holds an `HGLOBAL` that can be copied as bytes.
/// Bitmaps, metafiles, palettes, owner-display, and private formats hold
/// other handles or need the owner, so they are skipped. Windows makes
/// `CF_BITMAP` from the `CF_DIB` copy again.
pub fn is_memory_format(format: u32) -> bool {
    const CF_BITMAP: u32 = 2;
    const CF_METAFILEPICT: u32 = 3;
    const CF_PALETTE: u32 = 9;
    const CF_ENHMETAFILE: u32 = 14;
    match format {
        0 | CF_BITMAP | CF_METAFILEPICT | CF_PALETTE | CF_ENHMETAFILE => false,
        // CF_OWNERDISPLAY, CF_DSPTEXT, CF_DSPBITMAP, CF_DSPMETAFILEPICT, CF_DSPENHMETAFILE.
        0x80..=0x83 | 0x8E => false,
        // CF_PRIVATEFIRST..CF_PRIVATELAST and CF_GDIOBJFIRST..CF_GDIOBJLAST.
        0x200..=0x3FF => false,
        _ => true,
    }
}

/// The clipboard as the paste sees it. The system clipboard is one; the
/// tests use a fake, because the real clipboard belongs to the user.
pub(crate) trait Board {
    /// Copy every memory format on the clipboard.
    fn snapshot(&self) -> Result<Snapshot, InsertError>;
    /// Put `text` on the clipboard with delayed rendering, marked so
    /// clipboard history and managers skip it. Each render is a read in `receipts`.
    fn publish(&self, text: &str, receipts: Arc<Mutex<Receipts>>) -> Result<(), InsertError>;
    /// Whether the clipboard still holds what `publish` put there.
    fn still_ours(&self) -> bool;
    /// Put the snapshot back.
    fn restore(&self, saved: &Snapshot) -> bool;
    /// Replace the clipboard with plain text.
    fn write_text(&self, text: &str) -> bool;
}

/// Paste `text` through `board` and wait for the receipt.
///
/// `send_paste` sends the paste key. `quiet` is how long the reads must have
/// stopped before the old clipboard comes back.
///
/// With no read, the app did not take the text. The old clipboard then stays
/// away and the text stays on the clipboard, for two reasons: the user can
/// paste it by hand, and an app that reads very late cannot get stale content.
pub(crate) fn paste_with_receipt(
    board: &impl Board,
    text: &str,
    restore_clipboard: bool,
    quiet: Duration,
    send_paste: impl FnOnce() -> Result<(), InsertError>,
) -> Result<InsertResult, InsertError> {
    let saved = if restore_clipboard { Some(board.snapshot()?) } else { None };
    let receipts = Arc::new(Mutex::new(Receipts::default()));
    board.publish(text, receipts.clone())?;
    // Before the key: a fast app can read while the key is still down.
    receipts.lock().mark_sent(Instant::now());
    if let Err(e) = send_paste() {
        board.write_text(text);
        return Err(e);
    }
    let verdict = loop {
        match receipts.lock().verdict(Instant::now(), quiet) {
            Verdict::Wait => {}
            done => break done,
        }
        sleep(RECEIPT_POLL);
    };
    // Still our text on the clipboard? If not, the user or another app
    // copied something newer, and that content must stay.
    let still_ours = board.still_ours();
    match verdict {
        Verdict::Taken => {
            let clipboard_restored = match &saved {
                Some(saved) => still_ours && board.restore(saved),
                None => {
                    if still_ours {
                        // Plain text in place of the delayed one, so it can
                        // go into clipboard history now.
                        board.write_text(text);
                    }
                    false
                }
            };
            Ok(InsertResult::Pasted { clipboard_restored })
        }
        Verdict::NotTaken | Verdict::Wait => {
            if still_ours {
                board.write_text(text);
            }
            Err(InsertError::NotTaken)
        }
    }
}

// ---------------------------------------------------------------------------
// The system clipboard
// ---------------------------------------------------------------------------

/// Our text on the clipboard, waiting to be rendered.
struct Promise {
    /// UTF-16 with the closing NUL, as `CF_UNICODETEXT` wants it.
    text: Vec<u16>,
    receipts: Arc<Mutex<Receipts>>,
    /// The clipboard sequence number after our last change.
    seq: u32,
}

/// Only touched on the clipboard thread, in tasks and in the window procedure.
static PROMISE: Mutex<Option<Promise>> = Mutex::new(None);
/// One paste at a time: there is one clipboard.
static PASTE: Mutex<()> = Mutex::new(());

type Task = Box<dyn FnOnce(HWND) + Send>;

/// Thread message: run the queued tasks.
const WM_APP_TASK: u32 = WM_APP + 1;

/// The clipboard thread: a message-only window that owns our clipboard
/// content, and a queue of clipboard tasks that run on its thread.
struct OwnerThread {
    thread_id: u32,
    tasks: Sender<Task>,
}

static OWNER: OnceLock<Result<OwnerThread, String>> = OnceLock::new();

fn owner() -> Result<&'static OwnerThread, InsertError> {
    OWNER.get_or_init(spawn_owner).as_ref().map_err(|e| InsertError::Failed(e.clone()))
}

fn spawn_owner() -> Result<OwnerThread, String> {
    let (tasks, queue) = crossbeam_channel::unbounded::<Task>();
    let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<u32, String>>(1);
    std::thread::Builder::new()
        .name("sayso-clipboard".into())
        .spawn(move || {
            let mut msg = MSG::default();
            // SAFETY: everything runs on this thread with valid pointers. The
            // class name lives in a static, and the window lives as long as the thread.
            unsafe {
                // Create the message queue before anyone posts to it.
                let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
                let hwnd = match create_owner_window() {
                    Ok(hwnd) => hwnd,
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(GetCurrentThreadId()));
                while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                    if msg.message == WM_APP_TASK && msg.hwnd.is_invalid() {
                        while let Ok(task) = queue.try_recv() {
                            task(hwnd);
                        }
                    } else {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
            }
        })
        .map_err(|e| format!("cannot start the clipboard thread: {e}"))?;
    let thread_id = ready_rx.recv().map_err(|_| "the clipboard thread stopped".to_string())??;
    Ok(OwnerThread { thread_id, tasks })
}

/// # Safety
/// Call once, on the clipboard thread.
unsafe fn create_owner_window() -> Result<HWND, String> {
    static CLASS: OnceLock<Vec<u16>> = OnceLock::new();
    let class = CLASS.get_or_init(|| wide("SaysoClipboardOwner"));
    // SAFETY: the caller is the clipboard thread; the class name is static.
    unsafe {
        let instance = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        let wc = WNDCLASSW {
            lpfnWndProc: Some(owner_proc),
            hInstance: instance.into(),
            lpszClassName: PCWSTR(class.as_ptr()),
            ..Default::default()
        };
        RegisterClassW(&wc);
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class.as_ptr()),
            PCWSTR::null(),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance.into()),
            None,
        )
        .map_err(|e| format!("cannot create the clipboard window: {e}"))
    }
}

/// Run `f` on the clipboard thread and wait for its result.
fn on_owner<R: Send + 'static>(f: impl FnOnce(HWND) -> R + Send + 'static) -> Result<R, InsertError> {
    let owner = owner()?;
    let (tx, rx) = crossbeam_channel::bounded(1);
    owner
        .tasks
        .send(Box::new(move |hwnd| {
            let _ = tx.send(f(hwnd));
        }))
        .map_err(|_| InsertError::Failed("the clipboard thread stopped".into()))?;
    // SAFETY: posting to a thread id is safe.
    unsafe { PostThreadMessageW(owner.thread_id, WM_APP_TASK, WPARAM(0), LPARAM(0)) }
        .map_err(|e| InsertError::Failed(format!("cannot reach the clipboard thread: {e}")))?;
    rx.recv_timeout(OWNER_TIMEOUT).map_err(|_| InsertError::Failed("the clipboard thread did not answer".into()))
}

unsafe extern "system" fn owner_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // An app reads our text: the receipt. Windows has the clipboard open
        // for the reader, so the owner must not open it.
        WM_RENDERFORMAT => {
            if wparam.0 as u32 == CF_UNICODETEXT {
                render(true);
            }
            LRESULT(0)
        }
        // Our window goes away while it still owns a delayed format.
        WM_RENDERALLFORMATS => {
            if open_clipboard(hwnd) {
                // SAFETY: plain query while the clipboard is open.
                if unsafe { GetClipboardOwner() }.is_ok_and(|o| o == hwnd) {
                    render(false);
                }
                // SAFETY: we opened it above.
                let _ = unsafe { CloseClipboard() };
            }
            LRESULT(0)
        }
        // Someone emptied the clipboard: our text is gone.
        WM_DESTROYCLIPBOARD => {
            let gone = PROMISE.lock().take();
            if let Some(promise) = gone {
                promise.receipts.lock().mark_replaced();
            }
            LRESULT(0)
        }
        // SAFETY: default handling for every other message.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Who reads our text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reader {
    /// The app in front: the target of the paste.
    Target,
    /// Another program, for example a clipboard monitor or a sync tool.
    Other,
    /// A reader that opened the clipboard without a window. It cannot be told apart.
    Unknown,
}

/// Classify a reader by its process. `front` are the processes of the
/// window in front (a Store app has two: its frame host and itself).
pub fn classify_reader(reader: Option<u32>, front: &[u32]) -> Reader {
    match reader {
        None => Reader::Unknown,
        Some(pid) if front.contains(&pid) => Reader::Target,
        Some(_) => Reader::Other,
    }
}

/// Whether a reader gets the text.
///
/// Windows asks the owner to render only once. Clipboard monitors and sync
/// tools read every change, many of them without looking at the skip
/// formats. If such a reader got the text, its read would count as the
/// receipt, and the real paste would then read the stored text without a
/// message. So only the app in front gets it. Any other reader gets no data,
/// and the text stays delayed until the target asks.
pub fn serves(reader: Reader) -> bool {
    reader != Reader::Other
}

/// The process that has the clipboard open now (the reader), if it used a window.
fn reader_pid() -> Option<u32> {
    // SAFETY: plain query.
    let hwnd = unsafe { GetOpenClipboardWindow() }.ok().filter(|h| !h.is_invalid())?;
    Some(context::window_pid(hwnd)).filter(|pid| *pid != 0)
}

/// The processes of the window in front.
fn front_pids() -> Vec<u32> {
    // SAFETY: plain query.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_invalid() {
        return Vec::new();
    }
    let mut pids = vec![context::window_pid(hwnd)];
    if let Some((pid, _)) = context::window_process(hwnd)
        && !pids.contains(&pid)
    {
        pids.push(pid);
    }
    pids
}

/// Hand our text to Windows. `is_read` records a read receipt.
fn render(is_read: bool) {
    // Copy out, so no lock is held while Windows works.
    let Some((text, receipts)) = PROMISE.lock().as_ref().map(|p| (p.text.clone(), p.receipts.clone())) else {
        return;
    };
    if is_read {
        let pid = reader_pid();
        let reader = classify_reader(pid, &front_pids());
        log::debug!("clipboard read by {:?} ({reader:?})", pid.and_then(context::app_name_for_pid));
        if !serves(reader) {
            return;
        }
        receipts.lock().record_read(Instant::now());
    }
    if set_data(CF_UNICODETEXT, &units_to_bytes(&text)) {
        // SAFETY: plain query.
        let seq = unsafe { GetClipboardSequenceNumber() };
        if let Some(promise) = PROMISE.lock().as_mut() {
            promise.seq = seq;
        }
    }
}

fn units_to_bytes(units: &[u16]) -> Vec<u8> {
    units.iter().flat_map(|u| u.to_le_bytes()).collect()
}

/// Open the clipboard for `hwnd`, retrying while another app holds it.
fn open_clipboard(hwnd: HWND) -> bool {
    for attempt in 0..OPEN_TRIES {
        // SAFETY: plain call; the caller closes the clipboard.
        if unsafe { OpenClipboard(Some(hwnd)) }.is_ok() {
            return true;
        }
        if attempt + 1 < OPEN_TRIES {
            sleep(OPEN_RETRY);
        }
    }
    false
}

/// Run `f` with the clipboard open for `hwnd`.
fn with_clipboard<R>(hwnd: HWND, f: impl FnOnce() -> R) -> Result<R, InsertError> {
    if !open_clipboard(hwnd) {
        return Err(InsertError::Failed("another app holds the clipboard".into()));
    }
    let result = f();
    // SAFETY: we opened it above.
    let _ = unsafe { CloseClipboard() };
    Ok(result)
}

/// Put `bytes` on the open clipboard in `format`. Windows owns the memory after.
fn set_data(format: u32, bytes: &[u8]) -> bool {
    // SAFETY: the block is allocated with room for `bytes`, filled while
    // locked, and freed here unless Windows took it.
    unsafe {
        let Ok(block) = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)) else { return false };
        let ptr = GlobalLock(block);
        if ptr.is_null() {
            let _ = GlobalFree(Some(block));
            return false;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr.cast::<u8>(), bytes.len());
        let _ = GlobalUnlock(block);
        if SetClipboardData(format, Some(HANDLE(block.0))).is_ok() {
            true
        } else {
            let _ = GlobalFree(Some(block));
            false
        }
    }
}

/// The bytes of a clipboard handle that is an `HGLOBAL`.
fn global_bytes(handle: HANDLE) -> Option<Vec<u8>> {
    let block = HGLOBAL(handle.0);
    // SAFETY: GlobalSize and GlobalLock fail cleanly for a handle that is no
    // HGLOBAL. The copy reads only `size` bytes of the locked block.
    unsafe {
        let size = GlobalSize(block);
        if size == 0 {
            return None;
        }
        let ptr = GlobalLock(block);
        if ptr.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(ptr.cast::<u8>(), size).to_vec();
        let _ = GlobalUnlock(block);
        Some(bytes)
    }
}

/// The ids of [`SKIP_FORMATS`].
fn skip_formats() -> Vec<u32> {
    // SAFETY: registers (or looks up) a format name; no other effect.
    SKIP_FORMATS.iter().map(|name| unsafe { RegisterClipboardFormatW(PCWSTR(wide(name).as_ptr())) }).collect()
}

/// Mark the open clipboard so history and managers skip it.
fn mark_skipped(except: &[u32]) {
    for format in skip_formats().into_iter().filter(|f| *f != 0 && !except.contains(f)) {
        set_data(format, &0u32.to_le_bytes());
    }
}

/// Copy every memory format on the open clipboard.
fn read_all() -> Snapshot {
    let mut out = Vec::new();
    let mut format = 0;
    loop {
        // SAFETY: the clipboard is open; enumeration ends with 0.
        format = unsafe { EnumClipboardFormats(format) };
        if format == 0 {
            break;
        }
        if !is_memory_format(format) {
            continue;
        }
        // SAFETY: the clipboard is open; the handle stays valid until it closes.
        if let Ok(handle) = unsafe { GetClipboardData(format) }
            && let Some(bytes) = global_bytes(handle)
        {
            out.push((format, bytes));
        }
    }
    out
}

/// The real clipboard, through the clipboard thread.
pub(crate) struct SystemClipboard;

impl Board for SystemClipboard {
    fn snapshot(&self) -> Result<Snapshot, InsertError> {
        on_owner(|hwnd| with_clipboard(hwnd, read_all))?
    }

    fn publish(&self, text: &str, receipts: Arc<Mutex<Receipts>>) -> Result<(), InsertError> {
        let text: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        on_owner(move |hwnd| {
            with_clipboard(hwnd, || {
                // SAFETY: the clipboard is open for our window.
                unsafe { EmptyClipboard() }.map_err(|e| InsertError::Failed(format!("cannot empty the clipboard: {e}")))?;
                // After EmptyClipboard: it tells the old owner (maybe us) that its content is gone.
                *PROMISE.lock() = Some(Promise { text, receipts, seq: 0 });
                // SAFETY: no data: Windows asks our window when an app reads.
                // The call then returns NULL with no error, which the
                // `windows` crate reports as an error with code 0.
                match unsafe { SetClipboardData(CF_UNICODETEXT, None) } {
                    Err(e) if e.code().is_err() => {
                        return Err(InsertError::Failed(format!("cannot publish the text: {e}")));
                    }
                    _ => {}
                }
                mark_skipped(&[]);
                Ok(())
            })??;
            // SAFETY: plain query.
            let seq = unsafe { GetClipboardSequenceNumber() };
            if let Some(promise) = PROMISE.lock().as_mut() {
                promise.seq = seq;
            }
            Ok(())
        })?
    }

    fn still_ours(&self) -> bool {
        on_owner(|hwnd| {
            let seq = PROMISE.lock().as_ref().map(|p| p.seq);
            // SAFETY: plain queries.
            let (owner, now) = unsafe { (GetClipboardOwner(), GetClipboardSequenceNumber()) };
            seq == Some(now) && owner.is_ok_and(|o| o == hwnd)
        })
        .unwrap_or(false)
    }

    fn restore(&self, saved: &Snapshot) -> bool {
        let saved = saved.clone();
        on_owner(move |hwnd| {
            PROMISE.lock().take();
            with_clipboard(hwnd, || {
                // SAFETY: the clipboard is open for our window.
                if unsafe { EmptyClipboard() }.is_err() {
                    return false;
                }
                let all = saved.iter().all(|(format, bytes)| set_data(*format, bytes));
                // The old content is in clipboard history already; do not add it twice.
                if !saved.is_empty() {
                    let present: Vec<u32> = saved.iter().map(|(f, _)| *f).collect();
                    mark_skipped(&present);
                }
                all
            })
            .unwrap_or(false)
        })
        .unwrap_or(false)
    }

    fn write_text(&self, text: &str) -> bool {
        let bytes = units_to_bytes(&text.encode_utf16().chain(Some(0)).collect::<Vec<u16>>());
        on_owner(move |hwnd| {
            PROMISE.lock().take();
            with_clipboard(hwnd, || {
                // SAFETY: the clipboard is open for our window.
                unsafe { EmptyClipboard() }.is_ok() && set_data(CF_UNICODETEXT, &bytes)
            })
            .unwrap_or(false)
        })
        .unwrap_or(false)
    }
}

/// Whether a format is on the clipboard now. For tests and diagnostics.
pub fn clipboard_has(format_name: &str) -> bool {
    // SAFETY: looks up a format id, then a plain query.
    unsafe {
        let format = RegisterClipboardFormatW(PCWSTR(wide(format_name).as_ptr()));
        format != 0 && IsClipboardFormatAvailable(format).is_ok()
    }
}

// ---------------------------------------------------------------------------
// Synthetic keys
// ---------------------------------------------------------------------------

fn failed(err: String) -> InsertError {
    InsertError::Failed(err)
}

fn post_ctrl_v() -> Result<(), InsertError> {
    let (first, second) = input::paste_strokes(&input::held_modifiers(false));
    input::send(&first).map_err(failed)?;
    sleep(KEY_GAP);
    input::send(&second).map_err(failed)
}

fn type_text(text: &str) -> Result<(), InsertError> {
    input::send(&input::release_strokes(&input::held_modifiers(true))).map_err(failed)?;
    // One Return per line break, also for Windows line ends.
    let text = text.replace("\r\n", "\n");
    for chunk in utf16_chunks(&text, MAX_UNITS_PER_EVENT) {
        input::send(&input::text_strokes(&chunk)).map_err(failed)?;
        sleep(CHUNK_DELAY);
    }
    Ok(())
}

impl TextInserter for WinInserter {
    fn insert(&self, text: &str, method: InsertMethod) -> Result<InsertResult, InsertError> {
        preflight()?;
        match method {
            InsertMethod::Type => {
                type_text(text)?;
                Ok(InsertResult::Typed)
            }
            InsertMethod::Paste { restore_clipboard, restore_delay } => {
                let _one_at_a_time = PASTE.lock();
                paste_with_receipt(&SystemClipboard, text, restore_clipboard, restore_delay, post_ctrl_v)
            }
        }
    }

    fn copy_to_clipboard(&self, text: &str) {
        let _one_at_a_time = PASTE.lock();
        if !SystemClipboard.write_text(text) {
            log::warn!("could not copy to the clipboard");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rejoin(chunks: &[Vec<u16>]) -> String {
        String::from_utf16(&chunks.concat()).unwrap()
    }

    // -- Preconditions ------------------------------------------------------

    const MEDIUM: u32 = 0x2000;
    const HIGH: u32 = 0x3000;

    #[test]
    fn a_higher_target_blocks_the_keys() {
        assert!(integrity_blocks(Integrity::Level(MEDIUM), Integrity::Level(HIGH)));
        assert!(!integrity_blocks(Integrity::Level(MEDIUM), Integrity::Level(MEDIUM)));
        assert!(!integrity_blocks(Integrity::Level(HIGH), Integrity::Level(MEDIUM)));
    }

    #[test]
    fn a_hidden_level_counts_as_higher_unless_sayso_is_elevated() {
        assert!(integrity_blocks(Integrity::Level(MEDIUM), Integrity::Hidden));
        assert!(!integrity_blocks(Integrity::Level(HIGH), Integrity::Hidden));
    }

    #[test]
    fn an_unknown_level_does_not_block() {
        assert!(!integrity_blocks(Integrity::Unknown, Integrity::Level(HIGH)));
        assert!(!integrity_blocks(Integrity::Level(MEDIUM), Integrity::Unknown));
    }

    #[test]
    fn the_elevated_message_names_the_app() {
        assert_eq!(
            elevated_message("Task Manager"),
            "Task Manager runs as administrator, so Windows does not let Sayso type into it."
        );
    }

    #[test]
    fn only_the_app_in_front_and_unknown_readers_get_the_text() {
        assert_eq!(classify_reader(Some(10), &[10]), Reader::Target);
        // A Store app: the frame host and the app are both in front.
        assert_eq!(classify_reader(Some(20), &[10, 20]), Reader::Target);
        assert_eq!(classify_reader(Some(30), &[10, 20]), Reader::Other);
        assert_eq!(classify_reader(None, &[10]), Reader::Unknown);
        assert_eq!(classify_reader(Some(10), &[]), Reader::Other, "nothing in front: nobody is the target");
        assert!(serves(Reader::Target));
        assert!(serves(Reader::Unknown));
        assert!(!serves(Reader::Other));
    }

    #[test]
    fn only_memory_formats_are_saved() {
        for format in [1, 7, 8, 13, 15, 16, 17, 0xC000, 0xC123, 0xFFFF] {
            assert!(is_memory_format(format), "{format:#x}");
        }
        for format in [0, 2, 3, 9, 14, 0x80, 0x81, 0x82, 0x83, 0x8E, 0x200, 0x2FF, 0x300, 0x3FF] {
            assert!(!is_memory_format(format), "{format:#x}");
        }
    }

    // -- Paste receipts with a fake clipboard --------------------------------

    /// A clipboard that behaves like the Windows one: a delayed format is
    /// rendered on the first read only, and any write replaces everything.
    #[derive(Default)]
    struct FakeBoard(Mutex<FakeState>);

    #[derive(Default)]
    struct FakeState {
        formats: Snapshot,
        promise: Option<(String, Arc<Mutex<Receipts>>)>,
        /// Bumped on every change, like the clipboard sequence number.
        seq: u32,
        published: u32,
    }

    impl FakeBoard {
        fn with_text(text: &str) -> Self {
            let board = Self::default();
            board.write_text(text);
            board.0.lock().formats.push((7, b"oem".to_vec()));
            board
        }

        /// What an app gets when it reads the text.
        fn read(&self) -> Option<String> {
            let mut s = self.0.lock();
            if let Some((text, receipts)) = s.promise.take() {
                receipts.lock().record_read(Instant::now());
                s.formats.push((CF_UNICODETEXT, text.into_bytes()));
            }
            let (_, bytes) = s.formats.iter().find(|(f, _)| *f == CF_UNICODETEXT)?;
            Some(String::from_utf8(bytes.clone()).unwrap())
        }

        /// Another app copies something.
        fn copy(&self, text: &str) {
            let mut s = self.0.lock();
            if let Some((_, receipts)) = s.promise.take() {
                receipts.lock().mark_replaced();
            }
            s.formats = vec![(CF_UNICODETEXT, text.as_bytes().to_vec())];
            s.seq += 1;
        }

        fn formats(&self) -> Vec<u32> {
            self.0.lock().formats.iter().map(|(f, _)| *f).collect()
        }
    }

    impl Board for FakeBoard {
        fn snapshot(&self) -> Result<Snapshot, InsertError> {
            Ok(self.0.lock().formats.clone())
        }

        fn publish(&self, text: &str, receipts: Arc<Mutex<Receipts>>) -> Result<(), InsertError> {
            let mut s = self.0.lock();
            s.formats.clear();
            s.promise = Some((text.to_string(), receipts));
            s.seq += 1;
            s.published = s.seq;
            Ok(())
        }

        fn still_ours(&self) -> bool {
            let s = self.0.lock();
            s.seq == s.published
        }

        fn restore(&self, saved: &Snapshot) -> bool {
            let mut s = self.0.lock();
            s.promise = None;
            s.formats = saved.clone();
            s.seq += 1;
            true
        }

        fn write_text(&self, text: &str) -> bool {
            let mut s = self.0.lock();
            s.promise = None;
            s.formats = vec![(CF_UNICODETEXT, text.as_bytes().to_vec())];
            s.seq += 1;
            true
        }
    }

    const QUIET: Duration = Duration::from_millis(30);

    #[test]
    fn a_paste_that_an_app_reads_succeeds_and_restores_the_clipboard() {
        let board = FakeBoard::with_text("what the user copied");
        let mut read = None;
        let result = paste_with_receipt(&board, "dictated text", true, QUIET, || {
            // The target app reads the clipboard when it gets Ctrl+V.
            read = board.read();
            Ok(())
        });
        assert_eq!(read.as_deref(), Some("dictated text"), "the render gives the text to the reader");
        assert_eq!(result, Ok(InsertResult::Pasted { clipboard_restored: true }));
        assert_eq!(board.read().as_deref(), Some("what the user copied"));
        assert_eq!(board.formats(), [CF_UNICODETEXT, 7], "every saved format comes back");
    }

    #[test]
    fn a_paste_that_no_app_reads_fails_and_leaves_the_text_on_the_clipboard() {
        let board = FakeBoard::with_text("what the user copied");
        let started = Instant::now();
        let result = paste_with_receipt(&board, "dictated text", true, QUIET, || Ok(()));
        assert_eq!(result, Err(InsertError::NotTaken));
        assert!(started.elapsed() >= crate::paste_receipt::NO_READ_TIMEOUT);
        assert_eq!(board.read().as_deref(), Some("dictated text"), "the user can paste it by hand");
    }

    #[test]
    fn without_restore_the_text_stays_on_the_clipboard_after_the_paste() {
        let board = FakeBoard::with_text("what the user copied");
        let result = paste_with_receipt(&board, "dictated text", false, QUIET, || {
            board.read();
            Ok(())
        });
        assert_eq!(result, Ok(InsertResult::Pasted { clipboard_restored: false }));
        assert_eq!(board.read().as_deref(), Some("dictated text"));
    }

    #[test]
    fn a_newer_copy_during_the_paste_is_not_overwritten() {
        let board = FakeBoard::with_text("what the user copied");
        let result = paste_with_receipt(&board, "dictated text", true, QUIET, || {
            board.read();
            board.copy("copied during the paste");
            Ok(())
        });
        assert_eq!(result, Ok(InsertResult::Pasted { clipboard_restored: false }));
        assert_eq!(board.read().as_deref(), Some("copied during the paste"));
    }

    #[test]
    fn a_copy_before_any_read_ends_the_wait_at_once() {
        let board = FakeBoard::with_text("what the user copied");
        let started = Instant::now();
        let result = paste_with_receipt(&board, "dictated text", true, QUIET, || {
            board.copy("copied during the paste");
            Ok(())
        });
        assert_eq!(result, Err(InsertError::NotTaken));
        assert!(started.elapsed() < crate::paste_receipt::NO_READ_TIMEOUT);
        assert_eq!(board.read().as_deref(), Some("copied during the paste"));
    }

    #[test]
    fn a_paste_key_that_cannot_be_sent_leaves_the_text_on_the_clipboard() {
        let board = FakeBoard::with_text("what the user copied");
        let result = paste_with_receipt(&board, "dictated text", true, QUIET, || Err(InsertError::Failed("no key".into())));
        assert_eq!(result, Err(InsertError::Failed("no key".into())));
        assert_eq!(board.read().as_deref(), Some("dictated text"));
    }

    // -- Chunks ---------------------------------------------------------------

    #[test]
    fn short_text_is_one_chunk() {
        let chunks = utf16_chunks("hello", 20);
        assert_eq!(chunks.len(), 1);
        assert_eq!(rejoin(&chunks), "hello");
    }

    #[test]
    fn empty_text_has_no_chunks() {
        assert!(utf16_chunks("", 20).is_empty());
    }

    #[test]
    fn long_ascii_splits_at_the_limit() {
        let text = "a".repeat(45);
        let chunks = utf16_chunks(&text, 20);
        assert_eq!(chunks.iter().map(Vec::len).collect::<Vec<_>>(), [20, 20, 5]);
        assert_eq!(rejoin(&chunks), text);
    }

    #[test]
    fn emoji_pairs_are_never_split() {
        // 19 units, then a 2-unit emoji: it must start a new chunk.
        let text = format!("{}😀tail", "a".repeat(19));
        let chunks = utf16_chunks(&text, 20);
        assert_eq!(chunks[0].len(), 19);
        assert_eq!(chunks[1][0], 0xD83D, "the second chunk starts with the high surrogate");
        assert_eq!(rejoin(&chunks), text);
    }

    #[test]
    fn every_chunk_is_valid_utf16_and_within_the_limit() {
        let text = "héllo 👩‍👩‍👧 wörld 😀😀😀😀😀😀😀😀😀😀😀 日本語のテキスト 🎉";
        for max in [2, 3, 7, 20] {
            let chunks = utf16_chunks(text, max);
            for chunk in &chunks {
                assert!(chunk.len() <= max);
                assert!(String::from_utf16(chunk).is_ok(), "chunk split a surrogate pair at max {max}");
            }
            assert_eq!(rejoin(&chunks), text);
        }
    }

    #[test]
    fn an_all_emoji_text_fills_chunks_exactly() {
        let chunks = utf16_chunks(&"😀".repeat(25), 20);
        assert_eq!(chunks.iter().map(Vec::len).collect::<Vec<_>>(), [20, 20, 10]);
    }

    #[test]
    #[should_panic(expected = "surrogate pair")]
    fn a_limit_below_two_is_rejected() {
        utf16_chunks("a", 1);
    }

    // -- The real clipboard ---------------------------------------------------
    //
    // These touch the user's clipboard, so they are ignored by default. Each
    // saves the clipboard first and puts it back at the end.

    /// The real clipboard tests must not run at the same time.
    static REAL: Mutex<()> = Mutex::new(());

    /// Read the text the way another app does: open the clipboard with no
    /// window and ask for CF_UNICODETEXT.
    fn read_like_an_app() -> Option<String> {
        // SAFETY: the clipboard is opened and closed here; the data is copied
        // while it is open.
        unsafe {
            let mut opened = false;
            for _ in 0..OPEN_TRIES {
                if OpenClipboard(None).is_ok() {
                    opened = true;
                    break;
                }
                sleep(OPEN_RETRY);
            }
            if !opened {
                return None;
            }
            let text = GetClipboardData(CF_UNICODETEXT)
                .ok()
                .and_then(global_bytes)
                .map(|bytes| crate::win32::from_wide(&crate::win32::units_from_bytes(&bytes)));
            let _ = CloseClipboard();
            text
        }
    }

    /// Saves the user's clipboard and puts it back when dropped.
    struct SavedClipboard(Snapshot, Option<String>);

    impl SavedClipboard {
        fn take() -> Self {
            let snapshot = SystemClipboard.snapshot().expect("the clipboard can be read");
            Self(snapshot, read_like_an_app())
        }
    }

    impl Drop for SavedClipboard {
        fn drop(&mut self) {
            assert!(SystemClipboard.restore(&self.0), "the user's clipboard is back");
            assert_eq!(read_like_an_app(), self.1, "the user's clipboard text is back");
        }
    }

    #[test]
    #[ignore = "uses the real clipboard (saved and restored)"]
    fn the_system_clipboard_gives_a_receipt_and_restores() {
        let _one = REAL.lock();
        let _saved = SavedClipboard::take();
        SystemClipboard.write_text("before the paste");
        let mut read = None;
        let mut skipped = false;
        let result = paste_with_receipt(&SystemClipboard, "dictated text ✓", true, QUIET, || {
            skipped = clipboard_has("ExcludeClipboardContentFromMonitorProcessing")
                && clipboard_has("CanIncludeInClipboardHistory");
            read = read_like_an_app();
            Ok(())
        });
        assert!(skipped, "clipboard history is told to skip the dictated text");
        assert_eq!(read.as_deref(), Some("dictated text ✓"));
        assert_eq!(result, Ok(InsertResult::Pasted { clipboard_restored: true }));
        assert_eq!(read_like_an_app().as_deref(), Some("before the paste"));
    }

    #[test]
    #[ignore = "uses the real clipboard (saved and restored)"]
    fn a_reader_that_is_not_in_front_gets_nothing_and_leaves_the_text_for_the_target() {
        let _one = REAL.lock();
        let _ = env_logger::builder().is_test(true).try_init();
        if front_pids().contains(&std::process::id()) {
            println!("skipped: the test process is in front");
            return;
        }
        let _saved = SavedClipboard::take();
        SystemClipboard.write_text("before the paste");
        let mut monitor_read = Some(String::new());
        let mut target_read = None;
        let result = paste_with_receipt(&SystemClipboard, "dictated text", true, QUIET, || {
            // A clipboard monitor: a window of a process that is not in front.
            monitor_read = read_with_window();
            // Then the target, which cannot be told apart, so it is served.
            target_read = read_like_an_app();
            Ok(())
        });
        assert_eq!(monitor_read, None, "the monitor gets no data");
        assert_eq!(target_read.as_deref(), Some("dictated text"), "the text stayed delayed for the next reader");
        assert_eq!(result, Ok(InsertResult::Pasted { clipboard_restored: true }));
        assert_eq!(read_like_an_app().as_deref(), Some("before the paste"));
    }

    /// Read the clipboard with a message-only window of this process.
    fn read_with_window() -> Option<String> {
        use windows::Win32::UI::WindowsAndMessaging::DestroyWindow;
        // SAFETY: the window is created, used, and destroyed on this thread.
        unsafe {
            let class = wide("STATIC");
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(class.as_ptr()),
                PCWSTR::null(),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
            .ok()?;
            let text = if OpenClipboard(Some(hwnd)).is_ok() {
                let text = GetClipboardData(CF_UNICODETEXT)
                    .ok()
                    .and_then(global_bytes)
                    .map(|bytes| crate::win32::from_wide(&crate::win32::units_from_bytes(&bytes)));
                let _ = CloseClipboard();
                text
            } else {
                Some("could not open".into())
            };
            let _ = DestroyWindow(hwnd);
            text
        }
    }

    #[test]
    #[ignore = "uses the real clipboard (saved and restored)"]
    fn the_system_clipboard_reports_no_read() {
        let _one = REAL.lock();
        let _ = env_logger::builder().is_test(true).try_init();
        let _saved = SavedClipboard::take();
        SystemClipboard.write_text("before the paste");
        let result = paste_with_receipt(&SystemClipboard, "dictated text", true, QUIET, || Ok(()));
        assert_eq!(result, Err(InsertError::NotTaken));
        assert_eq!(read_like_an_app().as_deref(), Some("dictated text"));
        assert!(!clipboard_has("ExcludeClipboardContentFromMonitorProcessing"), "the left text is plain");
    }

    #[test]
    #[ignore = "uses the real clipboard (saved and restored)"]
    fn the_system_clipboard_keeps_a_newer_copy() {
        let _one = REAL.lock();
        let _saved = SavedClipboard::take();
        SystemClipboard.write_text("before the paste");
        let result = paste_with_receipt(&SystemClipboard, "dictated text", true, QUIET, || {
            read_like_an_app();
            // Another app copies: it opens the clipboard with no window and empties it.
            // SAFETY: open, empty, set, close, all here.
            unsafe {
                OpenClipboard(None).unwrap();
                EmptyClipboard().unwrap();
                assert!(set_data(CF_UNICODETEXT, &units_to_bytes(&wide("copied during the paste"))));
                CloseClipboard().unwrap();
            }
            Ok(())
        });
        assert_eq!(result, Ok(InsertResult::Pasted { clipboard_restored: false }));
        assert_eq!(read_like_an_app().as_deref(), Some("copied during the paste"));
    }

    #[test]
    #[ignore = "uses the real clipboard (saved and restored)"]
    fn snapshots_keep_every_memory_format() {
        let _one = REAL.lock();
        let _saved = SavedClipboard::take();
        SystemClipboard.write_text("hello");
        let snapshot = SystemClipboard.snapshot().unwrap();
        let formats: Vec<u32> = snapshot.iter().map(|(f, _)| *f).collect();
        assert!(formats.contains(&CF_UNICODETEXT));
        assert!(SystemClipboard.restore(&snapshot));
        assert_eq!(read_like_an_app().as_deref(), Some("hello"));
    }

    // -- End to end in a window of our own -------------------------------------

    #[test]
    #[ignore = "sends keys to a window of its own and uses the real clipboard (saved and restored)"]
    fn pastes_and_types_into_an_edit_control() {
        let _one = REAL.lock();
        let _saved = SavedClipboard::take();
        let window = crate::test_window::TestWindow::open();
        // Never send keys unless our own window is in front.
        assert!(window.wait_in_front(), "the test window could not come to the front; no keys were sent");

        let paste = InsertMethod::Paste { restore_clipboard: true, restore_delay: Duration::from_millis(200) };
        SystemClipboard.write_text("before the paste");
        assert!(window.is_in_front());
        let pasted = WinInserter.insert("pasted ✓ ", paste);
        assert_eq!(pasted, Ok(InsertResult::Pasted { clipboard_restored: true }));
        assert_eq!(window.wait_for("pasted ✓ "), "pasted ✓ ");
        assert_eq!(read_like_an_app().as_deref(), Some("before the paste"));

        assert!(window.is_in_front());
        let typed = WinInserter.insert("typed 😀\nline two", InsertMethod::Type);
        assert_eq!(typed, Ok(InsertResult::Typed));
        let want = "pasted ✓ typed 😀\r\nline two";
        assert_eq!(window.wait_for(want), want);
    }
}
