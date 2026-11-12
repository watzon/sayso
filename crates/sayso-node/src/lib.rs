//! The native part of `@justsayso/sdk`: `sayso-sdk` for JavaScript (`docs/sdk-typescript.md`).
//!
//! The TypeScript layer of `packages/sdk` is the API for apps. This crate gives it the
//! types of `sayso-sdk` with plain values:
//!
//! - Messages cross as JSON text. JavaScript reads them with `JSON.parse`, so a `u64`
//!   becomes a `number`, and the addon refuses a value above 2^53 ("Values").
//! - A call that can wait runs on a thread of its own and gives a promise of an
//!   [`Outcome`] (decision 10). The libuv pool has four threads, and a call of the SDK can
//!   wait minutes.
//! - Calls from Rust into JavaScript (an audio source, a final pass) go through a
//!   [`bridge::Bridge`], and the thread of Rust waits for the answer. The JavaScript
//!   thread never waits for Rust.

// Public, so the free functions of napi are not dead code in `cargo clippy --all-targets`.
pub mod bridge;
pub mod capture;
pub mod cleanup;
pub mod client;
pub mod embedded;
pub mod helpers;
mod pump;
mod speech;
mod values;

use napi::bindgen_prelude::*;
use napi_derive::napi;
use std::panic::{AssertUnwindSafe, catch_unwind};
pub(crate) use values::{Outcome, addon_error, error, outcome_text, to_value};

/// A number from JavaScript as a `u64`. A bad number is a `TypeError` in JavaScript.
pub(crate) fn id_of(number: f64, what: &str) -> Result<u64> {
    values::integer(number, what).map_err(|e| Error::new(Status::InvalidArg, e))
}

/// Read a request from JavaScript (JSON text) as `T`. A bad request is a `TypeError` in JavaScript.
pub(crate) fn parse_arg<T: serde::de::DeserializeOwned>(text: &str, what: &str) -> Result<T> {
    values::parse(text, what).map_err(|e| Error::new(Status::InvalidArg, e))
}

/// Run `work` on a new thread and give a promise of its [`Outcome`] as JSON text. The
/// promise holds the event loop of Node until it settles, so a process with a call that
/// waits does not end.
pub(crate) fn spawn_promise<'env, F>(env: &'env Env, name: &str, work: F) -> Result<Object<'env>>
where
    F: FnOnce() -> Outcome + Send + 'static,
{
    let (deferred, promise) = env.create_deferred::<String, _>()?;
    std::thread::Builder::new()
        .name(format!("sayso-node-{name}"))
        .spawn(move || {
            let outcome = catch_unwind(AssertUnwindSafe(work)).unwrap_or_else(|panic| Err(addon_error(format!("the addon failed: {}", panic_text(&panic)))));
            let text = outcome_text(outcome);
            deferred.resolve(move |_| Ok(text));
        })
        .map_err(|e| Error::from_reason(format!("cannot start a thread: {e}")))?;
    Ok(promise)
}

fn panic_text(panic: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(text) = panic.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = panic.downcast_ref::<String>() {
        text.clone()
    } else {
        "a panic".to_string()
    }
}

/// The version of the addon, which is the version of the crates. The TypeScript layer
/// checks that it loaded the addon of its own version.
#[napi]
pub fn addon_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// The version of the Host API of this SDK.
#[napi]
pub fn protocol_version() -> u32 {
    sayso_protocol::PROTOCOL
}
