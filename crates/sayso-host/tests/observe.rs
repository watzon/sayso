//! `subscribe` and `dictation_done` over a real socket, with the real
//! identity check (see `common`).

use crossbeam_channel::Receiver;
use sayso_extension::protocol::{Capability, DictationDone, ErrorCode, Event as HostEvent, SubscribeReply};
use sayso_extension::{Client, Event};
use serde_json::json;
use std::time::Duration;

mod common;
use common::*;

const OBSERVE: &[&str] = &["dictation.observe"];

fn done(text: &str) -> DictationDone {
    DictationDone {
        text: text.into(),
        transcript: format!("um {text}"),
        app: Some("Mail".into()),
        site: None,
        style: Some("email".into()),
        time: "2026-10-26T09:41:00Z".into(),
    }
}

fn subscribe(client: &Client, events: serde_json::Value) -> Vec<String> {
    client.request("subscribe", fields(json!({"events": events}))).unwrap().parse::<SubscribeReply>().unwrap().events
}

/// The `dictation_done` events that arrive in a short time.
fn heard(events: &Receiver<Event>) -> Vec<(Option<String>, DictationDone)> {
    let mut found = Vec::new();
    while let Ok(event) = events.recv_timeout(Duration::from_millis(200)) {
        if let Event::Host { id, event: HostEvent::DictationDone(done) } = event {
            found.push((id, done));
        }
    }
    found
}

#[test]
fn a_subscriber_gets_each_dictation_without_an_id() {
    let setup = setup();
    write_apps(&setup.apps, OBSERVE);
    let host = start_with(&setup, FakeServices::new(None));
    let client = Client::connect_to(&setup.socket).unwrap();
    let events = client.events();
    let other = Client::connect_to(&setup.socket).unwrap();
    let other_events = other.events();

    // Before `subscribe`, nothing comes.
    host.dictation_done(done("first"), false, false);
    assert!(heard(&events).is_empty());

    // Unknown names are left out of the reply, and are no error.
    assert_eq!(subscribe(&client, json!(["dictation_done", "meeting_started"])), vec!["dictation_done".to_string()]);
    host.dictation_done(done("Hello there."), false, false);
    host.dictation_done(done("Second."), false, false);
    assert_eq!(heard(&events), vec![(None, done("Hello there.")), (None, done("Second."))]);
    assert!(heard(&other_events).is_empty(), "a connection without subscribe gets nothing");

    // An empty list stops the events.
    assert_eq!(subscribe(&client, json!([])), Vec::<String>::new());
    host.dictation_done(done("Third."), false, false);
    assert!(heard(&events).is_empty());
}

#[test]
fn nothing_goes_out_while_incognito_was_on_at_the_start_or_the_end() {
    let setup = setup();
    write_apps(&setup.apps, OBSERVE);
    let host = start_with(&setup, FakeServices::new(None));
    let client = Client::connect_to(&setup.socket).unwrap();
    let events = client.events();
    subscribe(&client, json!(["dictation_done"]));
    host.dictation_done(done("on at the start"), true, false);
    host.dictation_done(done("on at the end"), false, true);
    host.dictation_done(done("on all the time"), true, true);
    assert!(heard(&events).is_empty());
    host.dictation_done(done("off"), false, false);
    assert_eq!(heard(&events), vec![(None, done("off"))]);
}

#[test]
fn a_grant_that_the_user_takes_back_stops_the_next_event() {
    let setup = setup();
    write_apps(&setup.apps, OBSERVE);
    let host = start_with(&setup, FakeServices::new(None));
    let client = Client::connect_to(&setup.socket).unwrap();
    let events = client.events();
    subscribe(&client, json!(["dictation_done"]));
    host.dictation_done(done("granted"), false, false);
    assert_eq!(heard(&events).len(), 1);

    write_apps(&setup.apps, &[]);
    host.dictation_done(done("taken back"), false, false);
    assert!(heard(&events).is_empty(), "the grant is read for each event");
    // The connection stays, and `subscribe` needs the capability now.
    let refusal = refused(client.request("subscribe", fields(json!({"events": ["dictation_done"]}))));
    assert_eq!((refusal.code, refusal.capability), (Some(ErrorCode::Capability), Some(Capability::DictationObserve)));

    // Granted again: the subscription of before counts again.
    write_apps(&setup.apps, OBSERVE);
    host.dictation_done(done("granted again"), false, false);
    assert_eq!(heard(&events), vec![(None, done("granted again"))]);
}

#[test]
fn a_closed_host_sends_no_more_events() {
    let setup = setup();
    write_apps(&setup.apps, OBSERVE);
    let host = start_with(&setup, FakeServices::new(None));
    let client = Client::connect_to(&setup.socket).unwrap();
    let events = client.events();
    subscribe(&client, json!(["dictation_done"]));
    host.close();
    host.dictation_done(done("after the close"), false, false);
    assert!(heard(&events).is_empty());
}
