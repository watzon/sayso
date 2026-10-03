//! The clipboard through X11 selections, for X11 sessions and for Wayland
//! sessions without data control (GNOME), where XWayland and Mutter bridge
//! the X11 clipboard to Wayland apps.
//!
//! An X11 selection holds no data: its owner sends the data to each app that
//! asks. So Sayso keeps a hidden window and a thread that answers the
//! requests, also after the paste, until another app takes the selection.
//! Each answered request for a text format is a read for the receipt.
//!
//! Limits: Sayso sends data in one property, so it cannot serve one format
//! larger than the largest X11 request (16 MiB with BIG-REQUESTS). It reads
//! large data in steps (INCR).

use super::{Clipboard, Content, Selection, SharedReceipts, Snapshot, read_counts};
use crate::session::Session;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::os::fd::AsRawFd;
use std::sync::Arc;
use std::time::{Duration, Instant};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, PropMode, Property, SELECTION_NOTIFY_EVENT,
    SelectionNotifyEvent, SelectionRequestEvent, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{CURRENT_TIME, NONE};

/// The longest time to read one selection, all formats together.
const READ_TIMEOUT: Duration = Duration::from_millis(1000);
/// The most bytes that the snapshot keeps of one selection.
const MAX_SNAPSHOT_BYTES: usize = 64 << 20;
/// Targets that describe the selection or act on it, and hold no data.
const META_TARGETS: [&str; 7] =
    ["TARGETS", "MULTIPLE", "TIMESTAMP", "SAVE_TARGETS", "DELETE", "INSERT_SELECTION", "INSERT_PROPERTY"];

struct Format {
    name: String,
    atom: Atom,
    data: Arc<Vec<u8>>,
    counts: bool,
}

struct Owned {
    generation: u64,
    formats: Vec<Format>,
    receipts: Option<SharedReceipts>,
    /// Another app took the selection.
    lost: bool,
}

#[derive(Default)]
struct State {
    owned: [Option<Owned>; 2],
    next: u64,
}

#[derive(Clone, Copy)]
struct Atoms {
    clipboard: Atom,
    primary: Atom,
    targets: Atom,
    text: Atom,
    utf8_string: Atom,
    incr: Atom,
    property: Atom,
}

impl Atoms {
    fn new(conn: &RustConnection) -> Result<Self, String> {
        let atom = |name: &str| -> Result<Atom, String> {
            Ok(conn.intern_atom(false, name.as_bytes()).map_err(err)?.reply().map_err(err)?.atom)
        };
        Ok(Self {
            clipboard: atom("CLIPBOARD")?,
            primary: AtomEnum::PRIMARY.into(),
            targets: atom("TARGETS")?,
            text: atom("TEXT")?,
            utf8_string: atom("UTF8_STRING")?,
            incr: atom("INCR")?,
            property: atom("SAYSO_SELECTION")?,
        })
    }

    fn selection(&self, selection: Selection) -> Atom {
        match selection {
            Selection::Clipboard => self.clipboard,
            Selection::Primary => self.primary,
        }
    }

    fn index_of(&self, atom: Atom) -> Option<usize> {
        if atom == self.clipboard {
            Some(Selection::Clipboard.index())
        } else if atom == self.primary {
            Some(Selection::Primary.index())
        } else {
            None
        }
    }
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn display_name(session: &Session) -> Option<&str> {
    session.x11_display.as_ref().and_then(|d| d.to_str())
}

/// A hidden window that can own selections and receive properties.
fn hidden_window(conn: &RustConnection, screen: usize) -> Result<Window, String> {
    let root = conn.setup().roots.get(screen).ok_or("no X11 screen")?.root;
    let window = conn.generate_id().map_err(err)?;
    conn.create_window(
        0,
        window,
        root,
        -10,
        -10,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        0,
        &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
    )
    .map_err(err)?;
    Ok(window)
}

pub struct X11Clipboard {
    conn: Arc<RustConnection>,
    window: Window,
    atoms: Atoms,
    state: Arc<Mutex<State>>,
    names: Mutex<HashMap<String, Atom>>,
    display: Option<String>,
}

impl X11Clipboard {
    /// Connect and start the thread that serves the selections.
    pub fn start(session: &Session) -> Result<Self, String> {
        let display = display_name(session).map(str::to_string);
        let (conn, screen) = x11rb::connect(display.as_deref()).map_err(err)?;
        let window = hidden_window(&conn, screen)?;
        let atoms = Atoms::new(&conn)?;
        conn.flush().map_err(err)?;
        let conn = Arc::new(conn);
        let state = Arc::new(Mutex::new(State::default()));
        let server = Server { conn: conn.clone(), window, atoms, state: state.clone() };
        std::thread::Builder::new()
            .name("sayso-x11-clipboard".into())
            .spawn(move || server.run())
            .map_err(err)?;
        Ok(Self { conn, window, atoms, state, names: Mutex::new(HashMap::new()), display })
    }

    fn atom(&self, name: &str) -> Result<Atom, String> {
        if let Some(atom) = self.names.lock().get(name) {
            return Ok(*atom);
        }
        let atom = self.conn.intern_atom(false, name.as_bytes()).map_err(err)?.reply().map_err(err)?.atom;
        self.names.lock().insert(name.to_string(), atom);
        Ok(atom)
    }

    fn owner(&self, selection: Selection) -> Option<Window> {
        let cookie = self.conn.get_selection_owner(self.atoms.selection(selection)).ok()?;
        cookie.reply().ok().map(|r| r.owner)
    }

    /// Our content of `selection`, while we own it.
    fn own_content(&self, selection: Selection) -> Option<Content> {
        let content = {
            let state = self.state.lock();
            let owned = state.owned[selection.index()].as_ref().filter(|o| !o.lost)?;
            owned.formats.iter().map(|f| (f.name.clone(), f.data.to_vec())).collect()
        };
        (self.owner(selection) == Some(self.window)).then_some(content)
    }
}

impl Clipboard for X11Clipboard {
    fn snapshot(&self) -> Snapshot {
        let own = [self.own_content(Selection::Clipboard), self.own_content(Selection::Primary)];
        let mut reader = None;
        let mut read = |selection: Selection| -> Option<Content> {
            if let Some(content) = &own[selection.index()] {
                return Some(content.clone());
            }
            if reader.is_none() {
                reader = Reader::connect(self.display.as_deref())
                    .inspect_err(|e| log::warn!("cannot read the clipboard: {e}"))
                    .ok();
            }
            reader.as_ref()?.read(selection)
        };
        Snapshot { clipboard: read(Selection::Clipboard), primary: read(Selection::Primary) }
    }

    fn publish(&self, selection: Selection, content: Content, receipts: Option<SharedReceipts>) -> Result<u64, String> {
        let mut formats = Vec::with_capacity(content.len());
        for (name, data) in content {
            let atom = self.atom(&name)?;
            let counts = read_counts(&name);
            formats.push(Format { name, atom, data: Arc::new(data), counts });
        }
        let generation = {
            let mut state = self.state.lock();
            state.next += 1;
            let generation = state.next;
            state.owned[selection.index()] = Some(Owned { generation, formats, receipts, lost: false });
            generation
        };
        let atom = self.atoms.selection(selection);
        self.conn.set_selection_owner(self.window, atom, CURRENT_TIME).map_err(err)?;
        // The reply also makes sure that the server has the new owner before
        // the paste key, which goes through another connection.
        if self.owner(selection) != Some(self.window) {
            return Err("the X server did not give Sayso the selection".into());
        }
        Ok(generation)
    }

    fn still_ours(&self, selection: Selection, generation: u64) -> bool {
        let current = {
            let state = self.state.lock();
            state.owned[selection.index()].as_ref().is_some_and(|o| o.generation == generation && !o.lost)
        };
        current && self.owner(selection) == Some(self.window)
    }

    fn clear(&self, selection: Selection) {
        if self.owner(selection) != Some(self.window) {
            return;
        }
        self.state.lock().owned[selection.index()] = None;
        let atom = self.atoms.selection(selection);
        if self.conn.set_selection_owner(NONE, atom, CURRENT_TIME).is_err() || self.conn.sync().is_err() {
            log::warn!("cannot clear the {selection:?} selection");
        }
    }
}

// ---------------------------------------------------------------------------
// The thread that serves the selections
// ---------------------------------------------------------------------------

struct Server {
    conn: Arc<RustConnection>,
    window: Window,
    atoms: Atoms,
    state: Arc<Mutex<State>>,
}

impl Server {
    fn run(self) {
        loop {
            match self.conn.wait_for_event() {
                Ok(Event::SelectionRequest(request)) => self.answer(&request),
                Ok(Event::SelectionClear(event)) => self.lost(event.selection),
                Ok(_) => {}
                Err(e) => {
                    log::warn!("the X11 clipboard connection is closed: {e}");
                    return;
                }
            }
        }
    }

    /// Another app may have taken `selection`. Only an owner other than our
    /// window counts: a clear can arrive after Sayso took the selection again.
    fn lost(&self, selection: Atom) {
        let Some(index) = self.atoms.index_of(selection) else { return };
        let owner = self.conn.get_selection_owner(selection).ok().and_then(|c| c.reply().ok()).map(|r| r.owner);
        if owner == Some(self.window) {
            return;
        }
        let mut state = self.state.lock();
        if let Some(owned) = state.owned[index].as_mut() {
            owned.lost = true;
            if let Some(receipts) = &owned.receipts {
                receipts.lock().mark_replaced();
            }
        }
    }

    fn answer(&self, request: &SelectionRequestEvent) {
        // An old client gives no property; it means the target name (ICCCM).
        let property = if request.property == NONE { request.target } else { request.property };
        let served = self.serve(request, property).unwrap_or_else(|| {
            log::debug!("refused a selection request for target {}", request.target);
            false
        });
        let notify = SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: request.time,
            requestor: request.requestor,
            selection: request.selection,
            target: request.target,
            property: if served { property } else { NONE },
        };
        let sent = self.conn.send_event(false, request.requestor, EventMask::NO_EVENT, notify);
        if sent.is_err() || self.conn.flush().is_err() {
            log::warn!("cannot answer a selection request");
        }
    }

    /// Write the requested data to the requestor's property.
    fn serve(&self, request: &SelectionRequestEvent, property: Atom) -> Option<bool> {
        let index = self.atoms.index_of(request.selection)?;
        let state = self.state.lock();
        let owned = state.owned[index].as_ref().filter(|o| !o.lost)?;
        let conn = &*self.conn;
        if request.target == self.atoms.targets {
            let mut targets = vec![self.atoms.targets];
            targets.extend(owned.formats.iter().map(|f| f.atom));
            conn.change_property32(PropMode::REPLACE, request.requestor, property, AtomEnum::ATOM, &targets).ok()?;
            return Some(true);
        }
        let format = owned.formats.iter().find(|f| f.atom == request.target)?;
        if format.data.len() + 64 > conn.maximum_request_bytes() {
            log::warn!("{} bytes of {} are too large for one X11 request", format.data.len(), format.name);
            return None;
        }
        let kind = if request.target == self.atoms.text { self.atoms.utf8_string } else { request.target };
        conn.change_property8(PropMode::REPLACE, request.requestor, property, kind, &format.data).ok()?;
        if format.counts
            && let Some(receipts) = &owned.receipts
        {
            receipts.lock().record_read(Instant::now());
        }
        Some(true)
    }
}

// ---------------------------------------------------------------------------
// Reading a selection that another app owns
// ---------------------------------------------------------------------------

/// A short-lived connection that reads selections, so that the server
/// thread can answer its own requests meanwhile.
struct Reader {
    conn: RustConnection,
    window: Window,
    atoms: Atoms,
}

impl Reader {
    fn connect(display: Option<&str>) -> Result<Self, String> {
        let (conn, screen) = x11rb::connect(display).map_err(err)?;
        let window = hidden_window(&conn, screen)?;
        let atoms = Atoms::new(&conn)?;
        conn.flush().map_err(err)?;
        Ok(Self { conn, window, atoms })
    }

    /// Every data format of `selection`. Some(empty) when nothing owns it,
    /// None when the owner does not answer.
    fn read(&self, selection: Selection) -> Option<Content> {
        let atom = self.atoms.selection(selection);
        let owner = self.conn.get_selection_owner(atom).ok()?.reply().ok()?.owner;
        if owner == NONE {
            return Some(Vec::new());
        }
        let deadline = Instant::now() + READ_TIMEOUT;
        let targets = self.convert(atom, self.atoms.targets, deadline)?;
        let mut names: Vec<(Atom, String)> = targets
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| u32::from_ne_bytes(*b))
            .filter(|a| *a != NONE)
            .filter_map(|a| Some((a, self.atom_name(a)?)))
            .filter(|(_, name)| !META_TARGETS.contains(&name.as_str()))
            .collect();
        // Text first: when time runs out, the text is the part to keep.
        names.sort_by_key(|(_, name)| !super::TEXT_FORMATS.contains(&name.as_str()));
        let mut content = Content::new();
        let mut total = 0;
        for (target, name) in names {
            if Instant::now() >= deadline {
                log::warn!("reading the {selection:?} selection took too long; the snapshot is partial");
                break;
            }
            if let Some(data) = self.convert(atom, target, deadline) {
                total += data.len();
                if total > MAX_SNAPSHOT_BYTES {
                    log::warn!("the {selection:?} selection is too large to save in full");
                    break;
                }
                content.push((name, data));
            }
        }
        (!content.is_empty()).then_some(content)
    }

    fn atom_name(&self, atom: Atom) -> Option<String> {
        let reply = self.conn.get_atom_name(atom).ok()?.reply().ok()?;
        String::from_utf8(reply.name).ok()
    }

    /// Ask the owner for `target` and wait for the data.
    fn convert(&self, selection: Atom, target: Atom, deadline: Instant) -> Option<Vec<u8>> {
        let property = self.atoms.property;
        self.conn.convert_selection(self.window, selection, target, property, CURRENT_TIME).ok()?;
        self.conn.flush().ok()?;
        loop {
            match next_event(&self.conn, deadline)? {
                Event::SelectionNotify(e) if e.selection == selection && e.target == target => {
                    if e.property == NONE {
                        return None;
                    }
                    let reply = self.take_property()?;
                    if reply.0 == self.atoms.incr {
                        return self.read_incr(deadline);
                    }
                    return Some(reply.1);
                }
                _ => {}
            }
        }
    }

    /// Read and delete our property: (type, bytes).
    fn take_property(&self) -> Option<(Atom, Vec<u8>)> {
        let reply = self
            .conn
            .get_property(true, self.window, self.atoms.property, AtomEnum::ANY, 0, u32::MAX / 4)
            .ok()?
            .reply()
            .ok()?;
        Some((reply.type_, reply.value))
    }

    /// The INCR protocol: the owner writes the data in parts, each after we
    /// delete the last one. An empty part ends it.
    fn read_incr(&self, deadline: Instant) -> Option<Vec<u8>> {
        let mut data = Vec::new();
        loop {
            match next_event(&self.conn, deadline)? {
                Event::PropertyNotify(e) if e.atom == self.atoms.property && e.state == Property::NEW_VALUE => {
                    let (_, part) = self.take_property()?;
                    if part.is_empty() {
                        return Some(data);
                    }
                    data.extend_from_slice(&part);
                    if data.len() > MAX_SNAPSHOT_BYTES {
                        return None;
                    }
                }
                _ => {}
            }
        }
    }
}

/// The next event, or None at the deadline.
fn next_event(conn: &RustConnection, deadline: Instant) -> Option<Event> {
    loop {
        if let Some(event) = conn.poll_for_event().ok()? {
            return Some(event);
        }
        let left = deadline.checked_duration_since(Instant::now())?;
        let mut fd = libc::pollfd { fd: conn.stream().as_raw_fd(), events: libc::POLLIN, revents: 0 };
        let ms = i32::try_from(left.as_millis()).unwrap_or(i32::MAX).max(1);
        // SAFETY: one valid pollfd for the duration of the call.
        if unsafe { libc::poll(&mut fd, 1, ms) } < 0 {
            return None;
        }
    }
}
