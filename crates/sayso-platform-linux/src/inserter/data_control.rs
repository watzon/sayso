// The data-control client. `clipboard_wayland.rs` includes this file twice,
// once for each protocol, after it names the protocol types `Manager`,
// `Device`, `Source`, and `Offer`, the modules `device`, `source`, and
// `offer`, `MAX_VERSION`, and `PRIMARY_SINCE`.

use crate::inserter::{Clipboard, Content, Selection, SharedReceipts, Snapshot, TEXT_FORMATS, read_counts, wl};
use crate::session::Session;
use crossbeam_channel::{Receiver, Sender, TryRecvError, bounded, unbounded};
use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::{Duration, Instant};
use wayland_client::backend::ObjectId;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_registry::WlRegistry, wl_seat::WlSeat};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop, event_created_child};

/// The longest wait for an answer of the clipboard thread.
const CALL_TIMEOUT: Duration = Duration::from_secs(3);
/// The longest time to read one selection, all formats together.
const READ_TIMEOUT: Duration = Duration::from_millis(1000);
/// The most bytes that the snapshot keeps of one selection.
const MAX_SNAPSHOT_BYTES: usize = 64 << 20;

enum Command {
    Snapshot(Sender<Snapshot>),
    Publish(Selection, Content, Option<SharedReceipts>, Sender<Result<u64, String>>),
    StillOurs(Selection, u64, Sender<bool>),
    Clear(Selection, Sender<()>),
}

/// The handle of the clipboard thread.
pub struct DataControl {
    tx: Sender<Command>,
    wake: UnixStream,
}

impl DataControl {
    fn call<T>(&self, make: impl FnOnce(Sender<T>) -> Command) -> Option<T> {
        let (reply, answer) = bounded(1);
        self.tx.send(make(reply)).ok()?;
        let _ = (&self.wake).write(&[1]);
        answer.recv_timeout(CALL_TIMEOUT).ok()
    }
}

impl Clipboard for DataControl {
    fn snapshot(&self) -> Snapshot {
        self.call(Command::Snapshot).unwrap_or_default()
    }

    fn publish(&self, selection: Selection, content: Content, receipts: Option<SharedReceipts>) -> Result<u64, String> {
        self.call(|reply| Command::Publish(selection, content, receipts, reply))
            .unwrap_or_else(|| Err("the clipboard thread does not answer".into()))
    }

    fn still_ours(&self, selection: Selection, generation: u64) -> bool {
        self.call(|reply| Command::StillOurs(selection, generation, reply)).unwrap_or(false)
    }

    fn clear(&self, selection: Selection) {
        self.call(|reply| Command::Clear(selection, reply));
    }
}

/// Connect, bind the protocol, and start the thread.
pub fn start(session: &Session) -> Result<DataControl, String> {
    let conn = wl::connect(session)?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn).map_err(|e| e.to_string())?;
    let qh = queue.handle();
    let manager: Manager = globals.bind(&qh, 1..=MAX_VERSION, ()).map_err(|e| e.to_string())?;
    let seat: WlSeat = globals.bind(&qh, 1..=1, ()).map_err(|e| e.to_string())?;
    let device = manager.get_data_device(&seat, &qh, ());
    let primary = manager.version() >= PRIMARY_SINCE;
    let mut state = State {
        primary,
        manager,
        device,
        qh,
        offers: HashMap::new(),
        current: [None, None],
        owned: [None, None],
        next: 0,
    };
    queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
    let (tx, rx) = unbounded();
    let (wake, wake_rx) = UnixStream::pair().map_err(|e| e.to_string())?;
    wake_rx.set_nonblocking(true).map_err(|e| e.to_string())?;
    std::thread::Builder::new()
        .name("sayso-data-control".into())
        .spawn(move || run(conn, queue, state, rx, wake_rx))
        .map_err(|e| e.to_string())?;
    Ok(DataControl { tx, wake })
}

struct Owned {
    generation: u64,
    formats: Vec<(String, Arc<Vec<u8>>)>,
    receipts: Option<SharedReceipts>,
    /// Another app took the selection.
    lost: bool,
}

struct State {
    manager: Manager,
    device: Device,
    qh: QueueHandle<State>,
    /// The compositor offers the primary selection.
    primary: bool,
    /// The MIME types of each offer.
    offers: HashMap<ObjectId, Vec<String>>,
    /// The offers of the current clipboard and primary selection.
    current: [Option<Offer>; 2],
    owned: [Option<Owned>; 2],
    next: u64,
}

fn run(conn: Connection, mut queue: EventQueue<State>, mut state: State, rx: Receiver<Command>, mut wake: UnixStream) {
    loop {
        if let Err(e) = queue.dispatch_pending(&mut state) {
            log::warn!("the data-control connection failed: {e}");
            return;
        }
        loop {
            match rx.try_recv() {
                Ok(command) => state.handle(command, &mut queue, &conn),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        if let Err(e) = conn.flush() {
            log::warn!("the data-control connection failed: {e}");
            return;
        }
        let Some(guard) = queue.prepare_read() else { continue };
        let mut fds = [
            libc::pollfd { fd: guard.connection_fd().as_raw_fd(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: wake.as_raw_fd(), events: libc::POLLIN, revents: 0 },
        ];
        // SAFETY: two valid pollfds for the duration of the call.
        if unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) } < 0 {
            continue;
        }
        if fds[0].revents != 0 {
            if let Err(e) = guard.read()
                && !matches!(&e, wayland_client::backend::WaylandError::Io(io) if io.kind() == std::io::ErrorKind::WouldBlock)
            {
                log::warn!("the data-control connection failed: {e}");
                return;
            }
        } else {
            drop(guard);
        }
        if fds[1].revents != 0 {
            let mut buf = [0u8; 64];
            while matches!(wake.read(&mut buf), Ok(n) if n > 0) {}
        }
    }
}

impl State {
    fn handle(&mut self, command: Command, queue: &mut EventQueue<State>, conn: &Connection) {
        match command {
            Command::Snapshot(reply) => {
                let _ = queue.roundtrip(self);
                let snapshot =
                    Snapshot { clipboard: self.read(Selection::Clipboard, conn), primary: self.read(Selection::Primary, conn) };
                let _ = reply.send(snapshot);
            }
            Command::Publish(selection, content, receipts, reply) => {
                let result = self.publish(selection, content, receipts);
                // The compositor must have the selection before the paste key.
                let result = result.and_then(|g| queue.roundtrip(self).map(|_| g).map_err(|e| e.to_string()));
                let _ = reply.send(result);
            }
            Command::StillOurs(selection, generation, reply) => {
                let _ = queue.roundtrip(self);
                let _ = reply.send(self.owned_now(selection).is_some_and(|o| o.generation == generation));
            }
            Command::Clear(selection, reply) => {
                if self.owned_now(selection).is_some() {
                    self.set(selection, None);
                    self.owned[selection.index()] = None;
                    let _ = queue.roundtrip(self);
                }
                let _ = reply.send(());
            }
        }
    }

    fn owned_now(&self, selection: Selection) -> Option<&Owned> {
        self.owned[selection.index()].as_ref().filter(|o| !o.lost)
    }

    fn set(&self, selection: Selection, source: Option<&Source>) {
        match selection {
            Selection::Clipboard => self.device.set_selection(source),
            Selection::Primary => self.device.set_primary_selection(source),
        }
    }

    fn publish(&mut self, selection: Selection, content: Content, receipts: Option<SharedReceipts>) -> Result<u64, String> {
        if selection == Selection::Primary && !self.primary {
            return Err("the compositor has no primary selection for data control".into());
        }
        self.next += 1;
        let generation = self.next;
        let source = self.manager.create_data_source(&self.qh, (selection, generation));
        for (mime, _) in &content {
            source.offer(mime.clone());
        }
        self.set(selection, Some(&source));
        let formats = content.into_iter().map(|(mime, data)| (mime, Arc::new(data))).collect();
        self.owned[selection.index()] = Some(Owned { generation, formats, receipts, lost: false });
        Ok(generation)
    }

    /// Every format of `selection`. Some(empty) when it is empty, None when
    /// the owner does not answer.
    fn read(&self, selection: Selection, conn: &Connection) -> Option<Content> {
        if let Some(owned) = self.owned_now(selection) {
            return Some(owned.formats.iter().map(|(m, d)| (m.clone(), d.to_vec())).collect());
        }
        let Some(offer) = &self.current[selection.index()] else { return Some(Vec::new()) };
        let mut mimes = self.offers.get(&offer.id()).cloned().unwrap_or_default();
        // Text first: when time runs out, the text is the part to keep.
        mimes.sort_by_key(|m| !TEXT_FORMATS.contains(&m.as_str()));
        let deadline = Instant::now() + READ_TIMEOUT;
        let mut content = Content::new();
        let mut total = 0;
        for mime in mimes {
            let Some(data) = receive(offer, &mime, conn, deadline) else {
                log::warn!("cannot read {mime} of the {selection:?} selection");
                continue;
            };
            total += data.len();
            if total > MAX_SNAPSHOT_BYTES {
                log::warn!("the {selection:?} selection is too large to save in full");
                break;
            }
            content.push((mime, data));
        }
        (!content.is_empty()).then_some(content)
    }
}

/// Read one MIME type of an offer through a pipe.
fn receive(offer: &Offer, mime: &str, conn: &Connection, deadline: Instant) -> Option<Vec<u8>> {
    let mut fds = [0; 2];
    // SAFETY: `fds` has room for the two descriptors.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
        return None;
    }
    // SAFETY: both descriptors are new and owned only here.
    let (read_end, write_end) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    offer.receive(mime.to_string(), write_end.as_fd());
    conn.flush().ok()?;
    drop(write_end);
    let mut file = std::fs::File::from(read_end);
    let mut data = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let left = deadline.checked_duration_since(Instant::now())?;
        let mut pfd = libc::pollfd { fd: file.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        let ms = i32::try_from(left.as_millis()).unwrap_or(i32::MAX).max(1);
        // SAFETY: one valid pollfd for the duration of the call.
        if unsafe { libc::poll(&mut pfd, 1, ms) } <= 0 {
            return None;
        }
        match file.read(&mut buf) {
            Ok(0) => return Some(data),
            Ok(n) => data.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
        if data.len() > MAX_SNAPSHOT_BYTES {
            return None;
        }
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &WlRegistry, _: <WlRegistry as Proxy>::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {
    }
}
delegate_noop!(State: ignore WlSeat);
delegate_noop!(State: Manager);

impl Dispatch<Device, ()> for State {
    fn event(state: &mut Self, _: &Device, event: device::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        let index = match event {
            device::Event::DataOffer { id } => {
                state.offers.insert(id.id(), Vec::new());
                return;
            }
            device::Event::Selection { id } => (Selection::Clipboard.index(), id),
            device::Event::PrimarySelection { id } => (Selection::Primary.index(), id),
            device::Event::Finished => {
                log::warn!("the compositor ended the data-control device");
                return;
            }
            _ => return,
        };
        let (index, offer) = index;
        if let Some(old) = std::mem::replace(&mut state.current[index], offer) {
            let still_used = state.current.iter().flatten().any(|o| o.id() == old.id());
            if !still_used {
                state.offers.remove(&old.id());
                old.destroy();
            }
        }
    }

    event_created_child!(State, Device, [device::EVT_DATA_OFFER_OPCODE => (Offer, ())]);
}

impl Dispatch<Offer, ()> for State {
    fn event(state: &mut Self, offer: &Offer, event: offer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let offer::Event::Offer { mime_type } = event {
            state.offers.entry(offer.id()).or_default().push(mime_type);
        }
    }
}

impl Dispatch<Source, (Selection, u64)> for State {
    fn event(
        state: &mut Self,
        source: &Source,
        event: source::Event,
        data: &(Selection, u64),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let (selection, generation) = *data;
        let owned = state.owned[selection.index()].as_mut().filter(|o| o.generation == generation && !o.lost);
        match event {
            source::Event::Send { mime_type, fd } => {
                let Some(owned) = owned else { return };
                let Some((_, bytes)) = owned.formats.iter().find(|(m, _)| *m == mime_type) else { return };
                if read_counts(&mime_type)
                    && let Some(receipts) = &owned.receipts
                {
                    receipts.lock().record_read(Instant::now());
                }
                // A slow reader must not stop the thread, so each write has its own.
                let bytes = bytes.clone();
                let spawned = std::thread::Builder::new().name("sayso-clipboard-send".into()).spawn(move || {
                    let mut file = std::fs::File::from(fd);
                    if let Err(e) = file.write_all(&bytes) {
                        log::debug!("a clipboard reader closed early: {e}");
                    }
                });
                if let Err(e) = spawned {
                    log::warn!("cannot send the clipboard: {e}");
                }
            }
            source::Event::Cancelled => {
                if let Some(owned) = owned {
                    owned.lost = true;
                    if let Some(receipts) = &owned.receipts {
                        receipts.lock().mark_replaced();
                    }
                }
                source.destroy();
            }
            _ => {}
        }
    }
}
