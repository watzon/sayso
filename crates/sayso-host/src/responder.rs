//! How work for a request talks back to its connection.
//!
//! A request that takes time (a download, later a job or a live session) does
//! not run on the reader thread of its connection. It runs on a worker thread
//! and gets a [`Responder`]. The worker sends events with the id of the
//! request, then the one reply. A job that goes on after its reply keeps a
//! [`Notifier`], which sends events without an id.
//!
//! Both put their messages in the outgoing channel of the connection, so one
//! writer thread writes them in order. The engine protocol rule follows from
//! that: the events that a request caused come before its reply, and none
//! come after, because [`Responder::reply`] takes the responder.

use crossbeam_channel::Sender;
use sayso_protocol::{ErrorCode, Event, Reply, encode};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A message for the writer thread of a connection.
pub(crate) enum Outgoing {
    /// One message, without the `\n`.
    Line(String),
    /// Stop writing and close both directions.
    Close,
}

/// Sends events without an id to one connection.
#[derive(Clone)]
pub(crate) struct Notifier {
    outgoing: Sender<Outgoing>,
    open: Arc<AtomicBool>,
}

impl Notifier {
    pub fn new(outgoing: Sender<Outgoing>, open: Arc<AtomicBool>) -> Notifier {
        Notifier { outgoing, open }
    }

    /// Send an event that no request caused, for example `job_progress`.
    pub fn send(&self, event: &Event) {
        let _ = self.outgoing.send(Outgoing::Line(encode(None, event)));
    }

    /// Close the connection after the messages that are in the channel now.
    pub fn close(&self) {
        let _ = self.outgoing.send(Outgoing::Close);
    }

    /// False after the app closed the connection. Long work stops then.
    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::SeqCst)
    }
}

/// The events of a job go to its connection without an id: the job goes on
/// after the `ok {job}` reply of `transcribe_file`.
impl crate::jobs::EventSink for Notifier {
    fn event(&self, event: Event) {
        self.send(&event);
    }
}

/// The answer to one request: events with its id, then one reply.
///
/// A responder that goes away without a reply sends `error`, so a worker that
/// returns early or panics does not leave the app waiting.
pub(crate) struct Responder {
    id: Option<String>,
    notifier: Notifier,
    replied: bool,
}

impl Responder {
    pub fn new(id: Option<String>, notifier: Notifier) -> Responder {
        Responder { id, notifier, replied: false }
    }

    /// Send an event that this request caused. It has the id of the request.
    pub fn event(&self, event: &Event) {
        let _ = self.notifier.outgoing.send(Outgoing::Line(encode(self.id.as_deref(), event)));
    }

    /// Send the reply. No event with this id follows.
    pub fn reply(mut self, reply: Reply) {
        self.replied = true;
        let _ = self.notifier.outgoing.send(Outgoing::Line(encode(self.id.as_deref(), &reply)));
    }

    /// True when the request had an `id`.
    pub fn has_id(&self) -> bool {
        self.id.is_some()
    }

    /// End a request without an `id` that needs no reply (`session_audio`).
    pub fn no_reply(mut self) {
        self.replied = true;
    }

    /// A handle for events without an id, for work that goes on after the reply.
    pub fn notifier(&self) -> Notifier {
        self.notifier.clone()
    }

    pub fn is_open(&self) -> bool {
        self.notifier.is_open()
    }
}

impl Drop for Responder {
    fn drop(&mut self) {
        if !self.replied {
            let reply = Reply::error(Some(ErrorCode::Unsupported), "Sayso stopped this request before it had an answer");
            let _ = self.notifier.outgoing.send(Outgoing::Line(encode(self.id.as_deref(), &reply)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::unbounded;
    use sayso_protocol::{HostMessage, Message, decode, Decoded};

    fn lines(messages: &crossbeam_channel::Receiver<Outgoing>) -> Vec<(Option<String>, HostMessage)> {
        messages
            .try_iter()
            .map(|message| match message {
                Outgoing::Line(line) => match decode::<HostMessage>(&line) {
                    Ok(Decoded::Known(Message { id, body })) => (id, body),
                    other => panic!("not a message of the host: {other:?}"),
                },
                Outgoing::Close => panic!("unexpected close"),
            })
            .collect()
    }

    fn make(id: Option<&str>) -> (Responder, crossbeam_channel::Receiver<Outgoing>) {
        let (outgoing, messages) = unbounded();
        (Responder::new(id.map(str::to_owned), Notifier::new(outgoing, Arc::new(AtomicBool::new(true)))), messages)
    }

    #[test]
    fn events_have_the_id_of_the_request_and_come_before_the_reply() {
        let (responder, messages) = make(Some("r7"));
        responder.event(&Event::JobProgress { job: 1, fraction: 0.5 });
        responder.event(&Event::JobProgress { job: 1, fraction: 1.0 });
        responder.reply(Reply::ok());
        let sent = lines(&messages);
        assert_eq!(sent.len(), 3);
        assert!(sent.iter().all(|(id, _)| id.as_deref() == Some("r7")));
        assert!(matches!(sent[0].1, HostMessage::Event(_)) && matches!(sent[1].1, HostMessage::Event(_)));
        assert!(matches!(sent[2].1, HostMessage::Reply(Reply::Ok(_))));
    }

    #[test]
    fn a_notifier_sends_events_without_an_id_after_the_reply() {
        let (responder, messages) = make(Some("r1"));
        let notifier = responder.notifier();
        responder.reply(Reply::ok());
        notifier.send(&Event::JobProgress { job: 2, fraction: 0.0 });
        let sent = lines(&messages);
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].0.as_deref(), Some("r1"));
        assert_eq!(sent[1].0, None);
    }

    #[test]
    fn a_responder_that_goes_away_without_a_reply_sends_an_error() {
        let (responder, messages) = make(Some("r3"));
        drop(responder);
        let sent = lines(&messages);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0.as_deref(), Some("r3"));
        assert!(matches!(sent[0].1, HostMessage::Reply(Reply::Error(_))));
        // One reply is enough: a responder that replied sends nothing more.
        let (responder, messages) = make(None);
        responder.reply(Reply::ok());
        assert_eq!(lines(&messages).len(), 1);
    }
}
