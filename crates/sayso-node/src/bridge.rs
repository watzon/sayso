//! Calls from the threads of Rust into JavaScript: an audio source and a final pass of
//! the app. The thread of Rust waits for the answer; the JavaScript thread never waits
//! for Rust, so this cannot deadlock (`docs/sdk-typescript.md`, "Threads").

use crate::addon_error;
use crossbeam_channel::{Sender, bounded};
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;
use parking_lot::Mutex;
use sayso_protocol::ErrorReply;
use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU32, Ordering};

/// What JavaScript gets: `(call, kind, json, samples)`. `call` is 0 for a call without an answer.
pub(crate) type Args = FnArgs<(u32, String, String, Option<Float32Array>)>;

struct JsCall {
    call: u32,
    kind: &'static str,
    json: String,
    samples: Option<Vec<f32>>,
}

/// The answer of JavaScript: JSON text, or the JSON text of an `ErrorReply`.
type Answer = std::result::Result<String, String>;

static PENDING: LazyLock<Mutex<HashMap<u32, Sender<Answer>>>> = LazyLock::new(Mutex::default);
static NEXT_CALL: AtomicU32 = AtomicU32::new(1);

/// One function of JavaScript that Rust can call from any thread. It does not hold the
/// event loop of Node: the work that uses it does ("Events").
pub(crate) struct Bridge {
    tsfn: ThreadsafeFunction<JsCall, (), Args, Status, false, true>,
}

impl Bridge {
    pub(crate) fn new(function: Function<'_, Args, ()>) -> Result<Bridge> {
        let tsfn = function
            .build_threadsafe_function::<JsCall>()
            .callee_handled::<false>()
            .weak::<true>()
            .build_callback(|ctx| Ok(FnArgs::from((ctx.value.call, ctx.value.kind.to_string(), ctx.value.json, ctx.value.samples.map(Float32Array::new)))))?;
        Ok(Bridge { tsfn })
    }

    /// Tell JavaScript something, and do not wait.
    pub(crate) fn notify(&self, kind: &'static str, json: String) {
        self.tsfn.call(JsCall { call: 0, kind, json, samples: None }, ThreadsafeFunctionCallMode::NonBlocking);
    }

    /// Call JavaScript and wait for its answer: `settleCall` from the TypeScript layer.
    pub(crate) fn call(&self, kind: &'static str, json: String, samples: Option<Vec<f32>>) -> std::result::Result<String, ErrorReply> {
        let call = NEXT_CALL.fetch_add(1, Ordering::Relaxed).max(1);
        let (tx, rx) = bounded(1);
        PENDING.lock().insert(call, tx);
        let status = self.tsfn.call(JsCall { call, kind, json, samples }, ThreadsafeFunctionCallMode::Blocking);
        if status != Status::Ok {
            PENDING.lock().remove(&call);
            return Err(addon_error(format!("cannot call JavaScript for {kind}: {status}")));
        }
        match rx.recv() {
            Ok(Ok(json)) => Ok(json),
            Ok(Err(json)) => Err(serde_json::from_str(&json).unwrap_or_else(|_| addon_error(json))),
            Err(_) => Err(addon_error(format!("JavaScript gave no answer for {kind}"))),
        }
    }
}

/// The answer of JavaScript to a call of a [`Bridge`]. `json` is the result, or with `ok`
/// false an `ErrorReply`.
#[napi]
pub fn settle_call(call: u32, ok: bool, json: String) {
    if let Some(tx) = PENDING.lock().remove(&call) {
        let _ = tx.send(if ok { Ok(json) } else { Err(json) });
    }
}
