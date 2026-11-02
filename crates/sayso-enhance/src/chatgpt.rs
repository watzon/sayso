//! ChatGPT plan provider: Sayso signs in to the user's ChatGPT account and
//! sends each request to the Codex backend of ChatGPT, so the usage counts
//! against the user's plan. No command line tool runs.
//!
//! This is the sign-in of the Codex CLI: its OAuth client, with PKCE and a
//! callback on this computer, and the backend at `chatgpt.com/backend-api/codex`.
//! Other open-source tools do the same. OpenAI does not document it for apps,
//! and it can change or close without notice. OpenAI's own "Sign in with
//! ChatGPT" for open-source apps replaces this when Sayso can use it: only
//! this module knows the client and the backend.
//!
//! The tokens are in the [`SecretStore`] under [`account_name`], as one JSON
//! object. The access token lives about an hour, and the refresh token rotates:
//! each refresh gives a new one, and the old one stops working. A process-wide
//! lock lets only one refresh run at a time.

use crate::auth::{AuthStatus, SignInEvent};
use crate::openai::{agent, map_ureq_error};
use crate::secrets::SecretStore;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use parking_lot::Mutex;
use sayso_core::config::{DEFAULT_CLI_THINKING, sort_thinking};
use sayso_core::enhance::{
    EnhanceError, EnhanceRequest, EnhanceResponse, Enhancer, ModelChoice, ProviderReport, output_schema, parse_output, user_message,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The OAuth client of the Codex CLI.
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const ISSUER: &str = "https://auth.openai.com";
const SCOPE: &str = "openid profile email offline_access";
/// The ports that the client allows for the callback, in order.
const CALLBACK_PORTS: [u16; 2] = [1455, 1457];
const BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
/// Who sends the request, in the `originator` header and the sign-in page.
const ORIGINATOR: &str = "sayso";
/// The backend hides models that need a newer client than this.
const CLIENT_VERSION: &str = "0.166.0";
/// Refresh the access token when it ends sooner than this.
const REFRESH_MARGIN: Duration = Duration::from_secs(120);
const SCHEMA_NAME: &str = "sayso_text";

/// The secret store account that holds the tokens of a provider.
pub fn account_name(provider_id: &str) -> String {
    format!("chatgpt.{provider_id}")
}

/// Remove the saved sign-in of a provider.
pub fn sign_out(secrets: &dyn SecretStore, provider_id: &str) -> Result<(), EnhanceError> {
    secrets.delete(&account_name(provider_id)).map_err(|e| EnhanceError::Cli(e.to_string()))
}

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
struct Tokens {
    access_token: String,
    refresh_token: String,
    id_token: Option<String>,
    /// Seconds since 1970.
    expires_at: u64,
    /// The ChatGPT account, for the `chatgpt-account-id` header.
    account_id: Option<String>,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Tokens {
    /// The tokens of a token endpoint reply. `previous` gives the refresh
    /// token and the account when the reply leaves them out.
    fn from_reply(reply: &Value, previous: Option<&Tokens>) -> Result<Tokens, EnhanceError> {
        let text = |key: &str| reply[key].as_str().filter(|s| !s.is_empty()).map(str::to_string);
        let access_token = text("access_token").ok_or_else(|| EnhanceError::InvalidOutput("the sign-in gave no access token".into()))?;
        let refresh_token = text("refresh_token")
            .or_else(|| previous.map(|p| p.refresh_token.clone()))
            .ok_or_else(|| EnhanceError::InvalidOutput("the sign-in gave no refresh token".into()))?;
        let id_token = text("id_token").or_else(|| previous.and_then(|p| p.id_token.clone()));
        let expires_in = reply["expires_in"].as_u64().unwrap_or(3600);
        let account_id = id_token
            .as_deref()
            .and_then(account_id)
            .or_else(|| account_id(&access_token))
            .or_else(|| previous.and_then(|p| p.account_id.clone()));
        Ok(Tokens { access_token, refresh_token, id_token, expires_at: now_secs() + expires_in, account_id })
    }

    fn to_json(&self) -> String {
        json!({
            "access_token": self.access_token,
            "refresh_token": self.refresh_token,
            "id_token": self.id_token,
            "expires_at": self.expires_at,
            "account_id": self.account_id,
        })
        .to_string()
    }

    fn from_json(text: &str) -> Option<Tokens> {
        let value: Value = serde_json::from_str(text).ok()?;
        let text = |key: &str| value[key].as_str().filter(|s| !s.is_empty()).map(str::to_string);
        Some(Tokens {
            access_token: text("access_token")?,
            refresh_token: text("refresh_token")?,
            id_token: text("id_token"),
            expires_at: value["expires_at"].as_u64().unwrap_or(0),
            account_id: text("account_id"),
        })
    }

    fn fresh(&self) -> bool {
        self.expires_at > now_secs() + REFRESH_MARGIN.as_secs()
    }

    /// "Plus plan · a@b.com", from the claims of the ID token.
    fn label(&self) -> Option<String> {
        let claims = self.id_token.as_deref().and_then(claims)?;
        let auth = &claims["https://api.openai.com/auth"];
        let plan = auth["chatgpt_plan_type"].as_str().filter(|p| !p.is_empty()).map(|kind| {
            let mut chars = kind.chars();
            let first = chars.next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();
            format!("{first}{} plan", chars.as_str())
        });
        let email = claims["email"].as_str().filter(|e| !e.is_empty());
        match (plan, email) {
            (Some(plan), Some(email)) => Some(format!("{plan} · {email}")),
            (plan, email) => plan.or_else(|| email.map(String::from)),
        }
    }
}

/// The payload of a JWT. Sayso reads it for the account and the label only:
/// the server checks the token, so the signature is not checked here.
fn claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn account_id(token: &str) -> Option<String> {
    let claims = claims(token)?;
    let found = claims["chatgpt_account_id"]
        .as_str()
        .or_else(|| claims["https://api.openai.com/auth"]["chatgpt_account_id"].as_str())
        .or_else(|| claims["organizations"][0]["id"].as_str());
    found.filter(|id| !id.is_empty()).map(str::to_string)
}

fn load(secrets: &dyn SecretStore, account: &str) -> Option<Tokens> {
    let text = secrets.get(account)?;
    let tokens = Tokens::from_json(&text);
    if tokens.is_none() {
        log::warn!("the ChatGPT sign-in in \"{account}\" is unreadable");
    }
    tokens
}

fn save(secrets: &dyn SecretStore, account: &str, tokens: &Tokens) -> Result<(), EnhanceError> {
    secrets.set(account, &tokens.to_json()).map_err(|e| EnhanceError::Cli(format!("could not save the ChatGPT sign-in: {e}")))
}

/// Only one refresh runs at a time, because a refresh token works once.
static REFRESH: LazyLock<Mutex<()>> = LazyLock::new(Mutex::default);

/// Tokens with an access token that still works. Refreshes it when it ends
/// soon, or when `refused` is the access token that the server just refused.
fn valid_tokens(secrets: &dyn SecretStore, account: &str, token_url: &str, refused: Option<&str>, timeout: Duration) -> Result<Tokens, EnhanceError> {
    let not_signed_in = || EnhanceError::NotConfigured("not signed in to ChatGPT".into());
    let usable = |tokens: &Tokens| tokens.fresh() && refused != Some(tokens.access_token.as_str());
    let tokens = load(secrets, account).ok_or_else(not_signed_in)?;
    if usable(&tokens) {
        return Ok(tokens);
    }
    let _guard = REFRESH.lock();
    // Another thread may have refreshed while this one waited.
    let tokens = load(secrets, account).ok_or_else(not_signed_in)?;
    if usable(&tokens) {
        return Ok(tokens);
    }
    let reply = post_form(
        token_url,
        &[("grant_type", "refresh_token"), ("refresh_token", &tokens.refresh_token), ("client_id", CLIENT_ID)],
        timeout,
    );
    let reply = match reply {
        Ok(reply) => reply,
        // The refresh token was used, revoked, or ran out: only a new sign-in helps.
        Err(EnhanceError::Http { status: 400 | 401, body }) => {
            let _ = secrets.delete(account);
            return Err(EnhanceError::NotConfigured(format!("not signed in to ChatGPT: the sign-in ended: {body}")));
        }
        Err(other) => return Err(other),
    };
    let next = Tokens::from_reply(&reply, Some(&tokens))?;
    save(secrets, account, &next)?;
    Ok(next)
}

/// POST a form and read the JSON reply. A status of 400 or more is `Http`.
fn post_form(url: &str, form: &[(&str, &str)], timeout: Duration) -> Result<Value, EnhanceError> {
    let timeout_ms = timeout.as_millis() as u64;
    let mut response = agent(url)
        .post(url)
        .config()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build()
        .send_form(form.iter().copied())
        .map_err(|e| map_ureq_error(e, timeout_ms))?;
    let status = response.status().as_u16();
    let text = response.body_mut().read_to_string().map_err(|e| map_ureq_error(e, timeout_ms))?;
    if status >= 400 {
        return Err(EnhanceError::Http { status, body: text.chars().take(500).collect() });
    }
    serde_json::from_str(&text).map_err(|e| EnhanceError::InvalidOutput(format!("the sign-in server reply is not JSON: {e}")))
}

/// The login state of a provider. A saved sign-in counts as signed in: when
/// its refresh token no longer works, the next request says so.
pub fn status(secrets: &dyn SecretStore, provider_id: &str) -> AuthStatus {
    match load(secrets, &account_name(provider_id)) {
        Some(tokens) => AuthStatus::SignedIn { account: tokens.label() },
        None => AuthStatus::SignedOut { detail: "no ChatGPT sign-in is saved for this provider".into() },
    }
}

// ---------------------------------------------------------------------------
// Sign-in
// ---------------------------------------------------------------------------

/// The endpoints of the sign-in. Tests point them at a local server.
#[derive(Debug, Clone)]
pub(crate) struct Endpoints {
    pub issuer: String,
    pub ports: Vec<u16>,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self { issuer: ISSUER.into(), ports: CALLBACK_PORTS.to_vec() }
    }
}

fn random_bytes() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    bytes[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    bytes
}

struct Pkce {
    verifier: String,
    challenge: String,
}

impl Pkce {
    fn new() -> Self {
        let verifier = URL_SAFE_NO_PAD.encode(random_bytes());
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Self { verifier, challenge }
    }
}

fn authorize_url(issuer: &str, redirect_uri: &str, pkce: &Pkce, state: &str) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("response_type", "code")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", SCOPE)
        .append_pair("code_challenge", &pkce.challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("id_token_add_organizations", "true")
        .append_pair("codex_cli_simplified_flow", "true")
        .append_pair("state", state)
        .append_pair("originator", ORIGINATOR)
        .finish();
    format!("{issuer}/oauth/authorize?{query}")
}

/// Ask a sign-in server that holds the port to stop. The Codex CLI and an
/// earlier sign-in of Sayso both answer `/cancel`.
fn ask_to_cancel(port: u16) {
    if let Ok(mut stream) = std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_secs(2)) {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let _ = stream.write_all(format!("GET /cancel HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n").as_bytes());
        let _ = stream.read(&mut [0u8; 64]);
    }
}

/// Listen on the first port that is free. A port in use is asked to cancel once.
fn bind(ports: &[u16]) -> Result<(tiny_http::Server, u16), EnhanceError> {
    for &port in ports {
        for attempt in 0..10 {
            match tiny_http::Server::http(("127.0.0.1", port)) {
                Ok(server) => return Ok((server, port)),
                Err(_) if attempt == 0 => ask_to_cancel(port),
                Err(_) => {}
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    let list = ports.iter().map(u16::to_string).collect::<Vec<_>>().join(" or ");
    Err(EnhanceError::Cli(format!("the ChatGPT sign-in needs port {list} on this computer, and it is in use")))
}

fn page(title: &str, message: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Sayso</title>\
         <style>body{{font:16px -apple-system,system-ui,sans-serif;display:flex;align-items:center;justify-content:center;height:100vh;margin:0}}\
         main{{max-width:28em;text-align:center}}</style></head>\
         <body><main><h1>{title}</h1><p>{message}</p></main></body></html>"
    )
}

fn respond(request: tiny_http::Request, status: u16, body: String) {
    let header = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).expect("a valid header");
    let _ = request.respond(tiny_http::Response::from_string(body).with_status_code(status).with_header(header));
}

/// A running ChatGPT sign-in. The caller opens the page of the `Url` event
/// in the browser. Dropping it stops the sign-in.
pub(crate) struct ChatGptSignIn {
    cancelled: Arc<AtomicBool>,
    events: Mutex<Option<Receiver<SignInEvent>>>,
}

impl ChatGptSignIn {
    pub fn start(secrets: Arc<dyn SecretStore>, provider_id: &str, limit: Duration, endpoints: Endpoints) -> Result<Self, EnhanceError> {
        let (server, port) = bind(&endpoints.ports)?;
        let redirect_uri = format!("http://127.0.0.1:{port}/auth/callback");
        let pkce = Pkce::new();
        let state = URL_SAFE_NO_PAD.encode(random_bytes());
        let url = authorize_url(&endpoints.issuer, &redirect_uri, &pkce, &state);
        let (tx, rx) = channel();
        let _ = tx.send(SignInEvent::Url(url));
        let cancelled = Arc::new(AtomicBool::new(false));
        let flow = Flow {
            server,
            redirect_uri,
            pkce,
            state,
            token_url: format!("{}/oauth/token", endpoints.issuer),
            secrets,
            account: account_name(provider_id),
            provider_id: provider_id.to_string(),
            deadline: Instant::now() + limit,
            cancelled: cancelled.clone(),
            tx,
        };
        std::thread::spawn(move || flow.run());
        Ok(Self { cancelled, events: Mutex::new(Some(rx)) })
    }

    pub fn next_event(&self) -> Option<SignInEvent> {
        let mut events = self.events.lock();
        let event = events.as_ref()?.recv().ok();
        if !matches!(event, Some(SignInEvent::Url(_))) {
            *events = None;
        }
        event
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
}

impl Drop for ChatGptSignIn {
    fn drop(&mut self) {
        self.cancel();
    }
}

struct Flow {
    server: tiny_http::Server,
    redirect_uri: String,
    pkce: Pkce,
    state: String,
    token_url: String,
    secrets: Arc<dyn SecretStore>,
    account: String,
    provider_id: String,
    deadline: Instant,
    cancelled: Arc<AtomicBool>,
    tx: Sender<SignInEvent>,
}

impl Flow {
    /// Serve the callback until the sign-in ends, then send `Finished` once.
    fn run(self) {
        let result = self.wait_for_code().and_then(|code| self.finish(&code));
        let status = match result {
            Ok(()) => status(self.secrets.as_ref(), &self.provider_id),
            Err(e) => {
                log::warn!("the ChatGPT sign-in did not finish: {e}");
                // A sign-in that failed keeps the one that was saved before.
                match status(self.secrets.as_ref(), &self.provider_id) {
                    AuthStatus::SignedOut { .. } => AuthStatus::SignedOut { detail: e.to_string() },
                    other => other,
                }
            }
        };
        let _ = self.tx.send(SignInEvent::Finished(status));
    }

    fn wait_for_code(&self) -> Result<String, EnhanceError> {
        loop {
            if self.cancelled.load(Ordering::SeqCst) {
                return Err(EnhanceError::Cli("the sign-in was cancelled".into()));
            }
            if Instant::now() >= self.deadline {
                return Err(EnhanceError::Timeout(0));
            }
            let request = match self.server.recv_timeout(Duration::from_millis(200)) {
                Ok(Some(request)) => request,
                Ok(None) => continue,
                Err(e) => return Err(EnhanceError::Cli(format!("the sign-in server stopped: {e}"))),
            };
            let (path, query) = request.url().split_once('?').unwrap_or((request.url(), ""));
            match path {
                "/auth/callback" => {
                    let params: HashMap<String, String> = url::form_urlencoded::parse(query.as_bytes()).into_owned().collect();
                    let failed = |request, message: String| {
                        respond(request, 400, page(&sayso_i18n::t!("enhance.sign_in_failed_title"), &sayso_i18n::t!("enhance.sign_in_failed_body")));
                        Err(EnhanceError::Cli(message))
                    };
                    if let Some(error) = params.get("error_description").or_else(|| params.get("error")) {
                        return failed(request, format!("the sign-in page reported: {error}"));
                    }
                    if params.get("state") != Some(&self.state) {
                        // Not this sign-in: an old tab, or another app. Keep waiting.
                        respond(request, 400, page(&sayso_i18n::t!("enhance.sign_in_failed_title"), &sayso_i18n::t!("enhance.sign_in_failed_body")));
                        continue;
                    }
                    let Some(code) = params.get("code").filter(|c| !c.is_empty()).cloned() else {
                        return failed(request, "the sign-in page gave no code".into());
                    };
                    respond(request, 200, page(&sayso_i18n::t!("enhance.sign_in_done_title"), &sayso_i18n::t!("enhance.sign_in_done_body")));
                    return Ok(code);
                }
                "/cancel" => {
                    respond(request, 200, String::new());
                    return Err(EnhanceError::Cli("another sign-in took the port".into()));
                }
                _ => respond(request, 404, String::new()),
            }
        }
    }

    fn finish(&self, code: &str) -> Result<(), EnhanceError> {
        let reply = post_form(
            &self.token_url,
            &[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", &self.redirect_uri),
                ("client_id", CLIENT_ID),
                ("code_verifier", &self.pkce.verifier),
            ],
            Duration::from_secs(30),
        )?;
        let tokens = Tokens::from_reply(&reply, None)?;
        save(self.secrets.as_ref(), &self.account, &tokens)
    }
}

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

/// A random id for this run of Sayso. The backend routes its prompt cache on it.
static SESSION_ID: LazyLock<String> = LazyLock::new(|| uuid::Uuid::new_v4().to_string());

/// The model that each account uses when the provider names none.
static DEFAULT_MODELS: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Mutex::default);

/// A request with the access token, the account, and the deadline.
fn signed<B>(builder: ureq::RequestBuilder<B>, tokens: &Tokens, remaining: Duration) -> ureq::RequestBuilder<B> {
    let mut builder = builder.header("Authorization", format!("Bearer {}", tokens.access_token)).header("originator", ORIGINATOR);
    if let Some(id) = &tokens.account_id {
        builder = builder.header("chatgpt-account-id", id);
    }
    builder.config().timeout_global(Some(remaining)).http_status_as_error(false).build()
}

/// Where the requests of one account go, and where its tokens are.
struct Account<'a> {
    secrets: &'a dyn SecretStore,
    /// The secret store account of the tokens.
    name: &'a str,
    base_url: &'a str,
    token_url: &'a str,
}

/// GET or POST with the access token of the account. A refused token is
/// refreshed once, and the request is sent again.
fn send(
    account: &Account,
    path: &str,
    body: Option<&Value>,
    deadline: Instant,
    timeout_ms: u64,
) -> Result<ureq::http::Response<ureq::Body>, EnhanceError> {
    let Account { secrets, name: account, base_url, token_url } = *account;
    let mut refused: Option<String> = None;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(EnhanceError::Timeout(timeout_ms));
        }
        let tokens = valid_tokens(secrets, account, token_url, refused.as_deref(), remaining)?;
        let url = format!("{base_url}{path}");
        let remaining = deadline.saturating_duration_since(Instant::now());
        let sent = match body {
            Some(body) => signed(agent(&url).post(&url), &tokens, remaining)
                .header("session-id", SESSION_ID.as_str())
                .header("Accept", "text/event-stream")
                .send_json(body),
            None => signed(agent(&url).get(&url), &tokens, remaining).call(),
        };
        let mut response = sent.map_err(|e| map_ureq_error(e, timeout_ms))?;
        let status = response.status().as_u16();
        if status == 401 && refused.is_none() {
            refused = Some(tokens.access_token.clone());
            continue;
        }
        if status >= 400 {
            let text = response.body_mut().read_to_string().unwrap_or_default();
            return Err(http_error(status, &text));
        }
        return Ok(response);
    }
}

/// The error of a refused request. A refused sign-in is `NotConfigured`.
fn http_error(status: u16, body: &str) -> EnhanceError {
    let detail: String = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().or_else(|| v["detail"].as_str()).map(str::to_string))
        .unwrap_or_else(|| body.to_string())
        .chars()
        .take(500)
        .collect();
    match status {
        401 | 403 => EnhanceError::NotConfigured(format!("not signed in to ChatGPT: the backend did not take the sign-in: {detail}")),
        _ => EnhanceError::Http { status, body: detail },
    }
}

/// The models of the account, for the picker.
pub fn list_models(secrets: &dyn SecretStore, provider_id: &str, timeout: Duration) -> Result<Vec<ModelChoice>, EnhanceError> {
    let name = account_name(provider_id);
    let token_url = format!("{ISSUER}/oauth/token");
    list_models_at(&Account { secrets, name: &name, base_url: BASE_URL, token_url: &token_url }, timeout)
}

fn list_models_at(account: &Account, timeout: Duration) -> Result<Vec<ModelChoice>, EnhanceError> {
    let timeout_ms = timeout.as_millis() as u64;
    let path = format!("/models?client_version={CLIENT_VERSION}");
    let mut response = send(account, &path, None, Instant::now() + timeout, timeout_ms)?;
    let text = response.body_mut().with_config().limit(16 * 1024 * 1024).read_to_string().map_err(|e| map_ureq_error(e, timeout_ms))?;
    let value: Value = serde_json::from_str(&text).map_err(|e| EnhanceError::InvalidOutput(format!("the model list is not JSON: {e}")))?;
    parse_models(&value)
}

/// Read `{"models": [...]}`. Models that the backend hides are left out. The
/// order is the backend's priority, lowest first.
fn parse_models(value: &Value) -> Result<Vec<ModelChoice>, EnhanceError> {
    let models = value["models"].as_array().ok_or_else(|| EnhanceError::InvalidOutput("the reply has no model list".into()))?;
    let mut listed: Vec<(i64, ModelChoice)> = models
        .iter()
        .filter(|m| m["visibility"].as_str().is_none_or(|v| v == "list"))
        .filter_map(|m| {
            let id = m["slug"].as_str().filter(|s| !s.is_empty())?;
            let name = m["display_name"].as_str().filter(|n| !n.is_empty()).unwrap_or(id);
            let levels = m["supported_reasoning_levels"]
                .as_array()
                .map(|items| items.iter().filter_map(|l| l["effort"].as_str()).map(str::to_string).collect())
                .unwrap_or_default();
            let choice = ModelChoice {
                id: id.to_string(),
                name: name.to_string(),
                description: m["description"].as_str().filter(|d| !d.is_empty()).map(str::to_string),
                thinking: sort_thinking(levels),
            };
            Some((m["priority"].as_i64().unwrap_or(i64::MAX), choice))
        })
        .collect();
    listed.sort_by_key(|(priority, _)| *priority);
    let mut seen = std::collections::HashSet::new();
    Ok(listed.into_iter().map(|(_, c)| c).filter(|c| seen.insert(c.id.clone())).collect())
}

/// The model to use when the provider names none: the first small model in
/// the list, else the first model. Cleanup needs speed more than depth.
fn pick_default(models: &[ModelChoice]) -> Option<String> {
    models.iter().find(|m| m.id.contains("mini")).or_else(|| models.first()).map(|m| m.id.clone())
}

/// A ChatGPT plan as an [`Enhancer`].
pub struct ChatGpt {
    id: String,
    account: String,
    model: Option<String>,
    thinking: String,
    secrets: Arc<dyn SecretStore>,
    default_timeout: Duration,
    base_url: String,
    token_url: String,
}

impl ChatGpt {
    /// `model` None uses a small model from the account's list.
    pub fn new(id: &str, model: Option<String>, secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            id: id.to_string(),
            account: account_name(id),
            model,
            thinking: DEFAULT_CLI_THINKING.into(),
            secrets,
            default_timeout: Duration::from_secs(8),
            base_url: BASE_URL.into(),
            token_url: format!("{ISSUER}/oauth/token"),
        }
    }

    /// The thinking level. None keeps the default, `low`.
    pub fn with_thinking(mut self, level: Option<&str>) -> Self {
        self.thinking = level.unwrap_or(DEFAULT_CLI_THINKING).into();
        self
    }

    /// The timeout used when a request has a zero timeout.
    pub fn with_default_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// Send the requests to another server. For tests.
    #[doc(hidden)]
    pub fn with_endpoints(mut self, base_url: &str, issuer: &str) -> Self {
        self.base_url = base_url.trim_end_matches('/').to_string();
        self.token_url = format!("{}/oauth/token", issuer.trim_end_matches('/'));
        self
    }

    fn target(&self) -> Account<'_> {
        Account { secrets: self.secrets.as_ref(), name: &self.account, base_url: &self.base_url, token_url: &self.token_url }
    }

    fn model(&self, request: &EnhanceRequest, deadline: Instant) -> Result<String, EnhanceError> {
        if let Some(model) = request.model.clone().or_else(|| self.model.clone()).filter(|m| !m.trim().is_empty()) {
            return Ok(model);
        }
        if let Some(model) = DEFAULT_MODELS.lock().get(&self.account) {
            return Ok(model.clone());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let models = list_models_at(&self.target(), remaining)?;
        let model = pick_default(&models).ok_or_else(|| EnhanceError::NotConfigured("the ChatGPT account offers no models".into()))?;
        DEFAULT_MODELS.lock().insert(self.account.clone(), model.clone());
        Ok(model)
    }

    fn body(&self, model: &str, request: &EnhanceRequest) -> Value {
        json!({
            "model": model,
            "stream": true,
            "store": false,
            "instructions": request.system_prompt,
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": user_message(&request.transcript) }],
            }],
            "tools": [],
            "tool_choice": "auto",
            "parallel_tool_calls": false,
            "reasoning": { "effort": self.thinking },
            "include": [],
            "prompt_cache_key": format!("sayso-{}", self.id),
            "text": { "format": { "type": "json_schema", "name": SCHEMA_NAME, "schema": output_schema(), "strict": true } },
        })
    }
}

impl Enhancer for ChatGpt {
    fn id(&self) -> &str {
        &self.id
    }

    /// The text goes to OpenAI.
    fn is_cloud(&self) -> bool {
        true
    }

    fn enhance(&self, request: &EnhanceRequest) -> Result<EnhanceResponse, EnhanceError> {
        let timeout = if request.timeout.is_zero() { self.default_timeout } else { request.timeout };
        let timeout_ms = timeout.as_millis() as u64;
        let started = Instant::now();
        let deadline = started + timeout;
        let model = self.model(request, deadline)?;
        let body = self.body(&model, request);
        let response = send(&self.target(), "/responses", Some(&body), deadline, timeout_ms)?;
        let reader = BufReader::new(response.into_body().into_reader());
        let (text, report) = read_stream(reader).map_err(|e| match e {
            EnhanceError::Network(m) if m.contains("timed out") || m.contains("Timeout") => EnhanceError::Timeout(timeout_ms),
            other => other,
        })?;
        Ok(EnhanceResponse {
            text: parse_output(&text)?,
            provider_id: self.id.clone(),
            model,
            elapsed_ms: started.elapsed().as_millis() as u64,
            mode: "chatgpt".into(),
            report: Some(report),
        })
    }
}

/// Read the server-sent events of a response until it completes. Returns the
/// output text and the numbers of the run.
fn read_stream(reader: impl BufRead) -> Result<(String, ProviderReport), EnhanceError> {
    let mut text = String::new();
    let mut done_text = None;
    for line in reader.lines() {
        let line = line.map_err(|e| {
            if matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) {
                EnhanceError::Network("timed out while the reply streamed".into())
            } else {
                EnhanceError::Network(format!("the reply stream broke: {e}"))
            }
        })?;
        let Some(data) = line.strip_prefix("data:").map(str::trim) else { continue };
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(data) else { continue };
        match event["type"].as_str().unwrap_or_default() {
            "response.output_text.delta" => text.push_str(event["delta"].as_str().unwrap_or_default()),
            "response.output_text.done" => done_text = event["text"].as_str().map(str::to_string),
            "response.refusal.done" => {
                return Err(EnhanceError::Refused(event["refusal"].as_str().unwrap_or_default().to_string()));
            }
            "response.completed" | "response.done" => {
                let response = &event["response"];
                let text = done_text.or_else(|| output_text(response)).unwrap_or(text);
                if response["status"] == "incomplete" {
                    let reason = response["incomplete_details"]["reason"].as_str().unwrap_or("unknown");
                    return Err(EnhanceError::InvalidOutput(format!("the reply is incomplete: {reason}")));
                }
                return Ok((text, report(&response["usage"])));
            }
            "response.failed" => return Err(stream_error(&event["response"]["error"])),
            "error" => return Err(stream_error(event.get("error").unwrap_or(&event))),
            _ => {}
        }
    }
    Err(EnhanceError::Network("the reply stream ended before the response completed".into()))
}

/// The text of the message items of a completed response.
fn output_text(response: &Value) -> Option<String> {
    let parts: Vec<&str> = response["output"]
        .as_array()?
        .iter()
        .filter(|item| item["type"] == "message")
        .filter_map(|item| item["content"].as_array())
        .flatten()
        .filter(|part| part["type"] == "output_text")
        .filter_map(|part| part["text"].as_str())
        .collect();
    (!parts.is_empty()).then(|| parts.concat())
}

fn stream_error(error: &Value) -> EnhanceError {
    let code = error["code"].as_str().or_else(|| error["type"].as_str()).unwrap_or("error");
    let message = error["message"].as_str().unwrap_or_default();
    let body = format!("{code}: {message}").chars().take(500).collect();
    let status = if code.contains("usage_limit") || code.contains("rate_limit") { 429 } else { 502 };
    EnhanceError::Http { status, body }
}

fn report(usage: &Value) -> ProviderReport {
    ProviderReport {
        input_tokens: usage["input_tokens"].as_u64(),
        output_tokens: usage["output_tokens"].as_u64(),
        thinking_tokens: usage["output_tokens_details"]["reasoning_tokens"].as_u64(),
        cache_read_tokens: usage["input_tokens_details"]["cached_tokens"].as_u64(),
        ..ProviderReport::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::MemoryStore;

    fn jwt(claims: Value) -> String {
        format!("e30.{}.sig", URL_SAFE_NO_PAD.encode(claims.to_string()))
    }

    fn id_token() -> String {
        jwt(json!({
            "email": "a@b.com",
            "https://api.openai.com/auth": { "chatgpt_account_id": "acct-1", "chatgpt_plan_type": "plus" },
        }))
    }

    #[test]
    fn tokens_read_the_account_and_the_label() {
        let reply = json!({ "access_token": "at", "refresh_token": "rt", "id_token": id_token(), "expires_in": 3600 });
        let tokens = Tokens::from_reply(&reply, None).unwrap();
        assert_eq!(tokens.account_id.as_deref(), Some("acct-1"));
        assert_eq!(tokens.label().as_deref(), Some("Plus plan · a@b.com"));
        assert!(tokens.fresh());
        assert_eq!(Tokens::from_json(&tokens.to_json()), Some(tokens));
    }

    #[test]
    fn a_refresh_without_new_tokens_keeps_the_old_ones() {
        let old = Tokens::from_reply(&json!({ "access_token": "a1", "refresh_token": "r1", "id_token": id_token() }), None).unwrap();
        let next = Tokens::from_reply(&json!({ "access_token": "a2" }), Some(&old)).unwrap();
        assert_eq!((next.access_token.as_str(), next.refresh_token.as_str()), ("a2", "r1"));
        assert_eq!(next.account_id.as_deref(), Some("acct-1"));
        assert!(Tokens::from_reply(&json!({ "refresh_token": "r" }), None).is_err());
    }

    #[test]
    fn status_reads_the_saved_sign_in() {
        let store = MemoryStore::new();
        assert!(matches!(status(&store, "p"), AuthStatus::SignedOut { .. }));
        let tokens = Tokens::from_reply(&json!({ "access_token": "a", "refresh_token": "r", "id_token": id_token() }), None).unwrap();
        save(&store, &account_name("p"), &tokens).unwrap();
        assert_eq!(status(&store, "p"), AuthStatus::SignedIn { account: Some("Plus plan · a@b.com".into()) });
        sign_out(&store, "p").unwrap();
        assert!(matches!(status(&store, "p"), AuthStatus::SignedOut { .. }));
    }

    #[test]
    fn authorize_url_has_the_pkce_and_the_flow_parameters() {
        let pkce = Pkce::new();
        assert_eq!(pkce.verifier.len(), 43);
        let url = authorize_url(ISSUER, "http://127.0.0.1:1455/auth/callback", &pkce, "st");
        let (base, query) = url.split_once('?').unwrap();
        assert_eq!(base, "https://auth.openai.com/oauth/authorize");
        let params: HashMap<String, String> = url::form_urlencoded::parse(query.as_bytes()).into_owned().collect();
        assert_eq!(params["client_id"], CLIENT_ID);
        assert_eq!(params["redirect_uri"], "http://127.0.0.1:1455/auth/callback");
        assert_eq!(params["code_challenge"], URL_SAFE_NO_PAD.encode(Sha256::digest(pkce.verifier.as_bytes())));
        assert_eq!(params["code_challenge_method"], "S256");
        assert_eq!(params["scope"], SCOPE);
        assert_eq!(params["state"], "st");
        assert_eq!(params["codex_cli_simplified_flow"], "true");
    }

    #[test]
    fn the_stream_gives_the_text_and_the_numbers() {
        let stream = "event: response.created\ndata: {\"type\":\"response.created\"}\n\n\
            data: {\"type\":\"response.output_text.delta\",\"delta\":\"{\\\"text\\\":\"}\n\
            data: {\"type\":\"response.output_text.delta\",\"delta\":\"\\\"Hi.\\\"}\"}\n\
            data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":30,\"input_tokens_details\":{\"cached_tokens\":20},\"output_tokens\":9,\"output_tokens_details\":{\"reasoning_tokens\":4}}}}\n";
        let (text, report) = read_stream(stream.as_bytes()).unwrap();
        assert_eq!(parse_output(&text).unwrap(), "Hi.");
        assert_eq!((report.input_tokens, report.cache_read_tokens), (Some(30), Some(20)));
        assert_eq!((report.output_tokens, report.thinking_tokens), (Some(9), Some(4)));
    }

    #[test]
    fn the_stream_reports_its_errors() {
        let failed = "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"usage_limit_reached\",\"message\":\"limit\"}}}\n";
        assert!(matches!(read_stream(failed.as_bytes()), Err(EnhanceError::Http { status: 429, .. })));
        let refused = "data: {\"type\":\"response.refusal.done\",\"refusal\":\"no\"}\n";
        assert_eq!(read_stream(refused.as_bytes()), Err(EnhanceError::Refused("no".into())));
        let cut = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"x\"}\n";
        assert!(matches!(read_stream(cut.as_bytes()), Err(EnhanceError::Network(_))));
        let incomplete = "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"incomplete\",\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n";
        assert!(matches!(read_stream(incomplete.as_bytes()), Err(EnhanceError::InvalidOutput(m)) if m.contains("max_output_tokens")));
    }

    #[test]
    fn the_completed_output_is_used_when_no_delta_came() {
        let stream = "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"type\":\"reasoning\"},{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"{\\\"text\\\":\\\"Ok\\\"}\"}]}]}}\n";
        assert_eq!(parse_output(&read_stream(stream.as_bytes()).unwrap().0).unwrap(), "Ok");
    }

    #[test]
    fn the_model_list_skips_hidden_models_and_keeps_the_priority() {
        let list = json!({ "models": [
            { "slug": "gpt-big", "display_name": "GPT Big", "priority": 2, "visibility": "list",
              "supported_reasoning_levels": [{ "effort": "high" }, { "effort": "low" }] },
            { "slug": "gpt-hidden", "priority": 0, "visibility": "hide" },
            { "slug": "gpt-mini", "display_name": "", "priority": 1, "visibility": "list", "description": "Fast" },
        ]});
        let models = parse_models(&list).unwrap();
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["gpt-mini", "gpt-big"]);
        assert_eq!(models[0].name, "gpt-mini");
        assert_eq!(models[1].thinking, ["low", "high"]);
        assert_eq!(pick_default(&models).as_deref(), Some("gpt-mini"));
        assert!(parse_models(&json!({})).is_err());
    }

    /// One request that a fake server got.
    #[derive(Debug, Clone)]
    struct Seen {
        method: String,
        url: String,
        headers: HashMap<String, String>,
        body: String,
    }

    /// A server on a free port. `reply` gives the status, the content type, and the body.
    fn fake_server(reply: impl Fn(&Seen) -> (u16, &'static str, String) + Send + 'static) -> (String, Arc<Mutex<Vec<Seen>>>) {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr().to_ip().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for mut request in server.incoming_requests() {
                let mut body = String::new();
                let _ = request.as_reader().read_to_string(&mut body);
                let headers = request.headers().iter().map(|h| (h.field.as_str().as_str().to_ascii_lowercase(), h.value.as_str().to_string())).collect();
                let entry = Seen { method: request.method().as_str().to_string(), url: request.url().to_string(), headers, body };
                let (status, kind, text) = reply(&entry);
                log.lock().push(entry);
                let header = tiny_http::Header::from_bytes(&b"Content-Type"[..], kind.as_bytes()).unwrap();
                let _ = request.respond(tiny_http::Response::from_string(text).with_status_code(status).with_header(header));
            }
        });
        (base, seen)
    }

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
    }

    fn form(body: &str) -> HashMap<String, String> {
        url::form_urlencoded::parse(body.as_bytes()).into_owned().collect()
    }

    fn token_reply(access: &str, refresh: &str) -> String {
        json!({ "access_token": access, "refresh_token": refresh, "id_token": id_token(), "expires_in": 3600 }).to_string()
    }

    #[test]
    fn the_sign_in_saves_the_tokens_of_the_callback() {
        let (issuer, seen) = fake_server(|_| (200, "application/json", token_reply("at", "rt")));
        let port = free_port();
        let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::new());
        let endpoints = Endpoints { issuer: issuer.clone(), ports: vec![port] };
        let sign_in = ChatGptSignIn::start(store.clone(), "p", Duration::from_secs(20), endpoints).unwrap();
        let Some(SignInEvent::Url(url)) = sign_in.next_event() else { panic!("no URL") };
        assert!(url.starts_with(&format!("{issuer}/oauth/authorize?")), "{url}");
        let params = form(url.split_once('?').unwrap().1);

        // A callback with another state is not this sign-in. It is refused, and the sign-in waits on.
        let callback = format!("http://127.0.0.1:{port}/auth/callback");
        let wrong = agent(&callback).get(&format!("{callback}?code=x&state=other")).config().http_status_as_error(false).build().call().unwrap();
        assert_eq!(wrong.status().as_u16(), 400);
        let done = agent(&callback)
            .get(&format!("{callback}?code=the-code&state={}", params["state"]))
            .config()
            .http_status_as_error(false)
            .build()
            .call()
            .unwrap();
        assert_eq!(done.status().as_u16(), 200);

        assert_eq!(sign_in.next_event(), Some(SignInEvent::Finished(AuthStatus::SignedIn { account: Some("Plus plan · a@b.com".into()) })));
        assert_eq!(sign_in.next_event(), None);
        let exchange = seen.lock()[0].clone();
        assert_eq!((exchange.method.as_str(), exchange.url.as_str()), ("POST", "/oauth/token"));
        let sent = form(&exchange.body);
        assert_eq!(sent["grant_type"], "authorization_code");
        assert_eq!(sent["code"], "the-code");
        assert_eq!(sent["redirect_uri"], params["redirect_uri"]);
        assert_eq!(URL_SAFE_NO_PAD.encode(Sha256::digest(sent["code_verifier"].as_bytes())), params["code_challenge"]);
        let tokens = load(store.as_ref(), &account_name("p")).unwrap();
        assert_eq!((tokens.access_token.as_str(), tokens.refresh_token.as_str()), ("at", "rt"));
    }

    #[test]
    fn a_cancelled_sign_in_ends_signed_out_and_frees_the_port() {
        let port = free_port();
        let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::new());
        let endpoints = Endpoints { issuer: "http://127.0.0.1:9".into(), ports: vec![port] };
        let sign_in = ChatGptSignIn::start(store, "p", Duration::from_secs(20), endpoints).unwrap();
        assert!(matches!(sign_in.next_event(), Some(SignInEvent::Url(_))));
        sign_in.cancel();
        assert!(matches!(sign_in.next_event(), Some(SignInEvent::Finished(AuthStatus::SignedOut { .. }))));
        std::thread::sleep(Duration::from_millis(100));
        assert!(std::net::TcpListener::bind(("127.0.0.1", port)).is_ok(), "the callback port is free again");
    }

    fn request() -> EnhanceRequest {
        EnhanceRequest { system_prompt: "Fix it.".into(), transcript: "me and him".into(), model: None, temperature: None, timeout: Duration::from_secs(10) }
    }

    const STREAM: &str = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"{\\\"text\\\":\\\"He and I.\\\"}\"}\n\n\
        data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":5}}}\n\n";

    #[test]
    fn a_refused_access_token_is_refreshed_once_and_the_request_sent_again() {
        let (issuer, issued) = fake_server(|_| (200, "application/json", token_reply("new", "r2")));
        let (backend, sent) = fake_server(|seen| match seen.headers.get("authorization").map(String::as_str) {
            Some("Bearer new") => (200, "text/event-stream", STREAM.to_string()),
            _ => (401, "application/json", r#"{"error":{"message":"expired"}}"#.to_string()),
        });
        let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::new());
        let old = Tokens::from_reply(&json!({ "access_token": "old", "refresh_token": "r1", "id_token": id_token() }), None).unwrap();
        save(store.as_ref(), &account_name("p"), &old).unwrap();

        let chatgpt = ChatGpt::new("p", Some("gpt-mini".into()), store.clone()).with_thinking(Some("medium")).with_endpoints(&backend, &issuer);
        let done = chatgpt.enhance(&request()).unwrap();
        assert_eq!((done.text.as_str(), done.model.as_str(), done.mode.as_str()), ("He and I.", "gpt-mini", "chatgpt"));
        assert_eq!(done.report.unwrap().input_tokens, Some(5));

        let refresh = form(&issued.lock()[0].body);
        assert_eq!((refresh["grant_type"].as_str(), refresh["refresh_token"].as_str()), ("refresh_token", "r1"));
        assert_eq!(load(store.as_ref(), &account_name("p")).unwrap().refresh_token, "r2", "the new refresh token is saved");

        let sent = sent.lock();
        assert_eq!(sent.len(), 2);
        let last = &sent[1];
        assert_eq!(last.url, "/responses");
        assert_eq!(last.headers.get("chatgpt-account-id").map(String::as_str), Some("acct-1"));
        assert_eq!(last.headers.get("originator").map(String::as_str), Some(ORIGINATOR));
        let body: Value = serde_json::from_str(&last.body).unwrap();
        assert_eq!((body["model"].as_str(), body["store"].as_bool(), body["stream"].as_bool()), (Some("gpt-mini"), Some(false), Some(true)));
        assert_eq!(body["instructions"], "Fix it.");
        assert_eq!(body["reasoning"]["effort"], "medium");
        assert_eq!(body["input"][0]["content"][0]["text"], "<transcript>\nme and him\n</transcript>");
        assert_eq!(body["text"]["format"]["schema"], output_schema());
        assert!(body.get("temperature").is_none() && body.get("max_output_tokens").is_none(), "the backend refuses these");
    }

    #[test]
    fn an_ended_refresh_token_removes_the_sign_in() {
        let (issuer, _) = fake_server(|_| (400, "application/json", r#"{"error":"invalid_grant"}"#.to_string()));
        let (backend, sent) = fake_server(|_| (200, "text/event-stream", STREAM.to_string()));
        let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::new());
        let mut old = Tokens::from_reply(&json!({ "access_token": "old", "refresh_token": "r1" }), None).unwrap();
        old.expires_at = 0;
        save(store.as_ref(), &account_name("p"), &old).unwrap();
        let chatgpt = ChatGpt::new("p", Some("m".into()), store.clone()).with_endpoints(&backend, &issuer);
        assert!(matches!(chatgpt.enhance(&request()), Err(EnhanceError::NotConfigured(m)) if m.contains("sign-in ended")));
        assert!(sent.lock().is_empty(), "no request goes out with an ended token");
        assert!(matches!(status(store.as_ref(), "p"), AuthStatus::SignedOut { .. }));
    }

    #[test]
    fn without_a_model_a_small_model_of_the_account_is_used() {
        let (backend, _) = fake_server(|seen| {
            if seen.url.starts_with("/models") {
                assert!(seen.url.contains("client_version="), "{}", seen.url);
                let list = json!({ "models": [
                    { "slug": "gpt-big", "priority": 0, "visibility": "list" },
                    { "slug": "gpt-small-mini", "priority": 1, "visibility": "list" },
                ]});
                (200, "application/json", list.to_string())
            } else {
                (200, "text/event-stream", STREAM.to_string())
            }
        });
        let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::new());
        let tokens = Tokens::from_reply(&json!({ "access_token": "a", "refresh_token": "r" }), None).unwrap();
        save(store.as_ref(), &account_name("default-model"), &tokens).unwrap();
        let chatgpt = ChatGpt::new("default-model", None, store).with_endpoints(&backend, "http://127.0.0.1:9");
        assert_eq!(chatgpt.enhance(&request()).unwrap().model, "gpt-small-mini");
    }

    #[test]
    fn a_refused_sign_in_is_not_configured() {
        assert!(matches!(http_error(401, r#"{"error":{"message":"bad token"}}"#), EnhanceError::NotConfigured(m) if m.contains("bad token")));
        assert!(matches!(http_error(500, "boom"), EnhanceError::Http { status: 500, body } if body == "boom"));
    }
}
