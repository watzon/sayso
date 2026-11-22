package enhance

import (
	"bufio"
	"bytes"
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net"
	"net/http"
	"net/url"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/providers"
	"github.com/watzon/sayso/internal/store"
)

// The ChatGPT account enhancer (sayso-enhance/src/chatgpt.rs): a PKCE sign-in
// with the Codex client id, tokens in the secret store, and the Responses
// API of the ChatGPT backend with a JSON schema.

const (
	chatgptClientID      = "app_EMoamEEZ73f0CkXaXp7hrann"
	chatgptIssuer        = "https://auth.openai.com"
	chatgptScope         = "openid profile email offline_access"
	chatgptBaseURL       = "https://chatgpt.com/backend-api/codex"
	chatgptOriginator    = "sayso"
	chatgptClientVersion = "0.166.0"
	refreshMargin        = 120 // seconds
	// SignInLimit is how long a sign-in may take.
	SignInLimit = 600 * time.Second
)

var chatgptCallbackPorts = []int{1455, 1457}

// ChatGPTAccountName is the secret store account of a provider's sign-in.
func ChatGPTAccountName(providerID string) string { return "chatgpt." + providerID }

// ChatGPTSignOut deletes the saved sign-in.
func ChatGPTSignOut(secrets providers.SecretStore, providerID string) error {
	if err := secrets.Delete(ChatGPTAccountName(providerID)); err != nil {
		return &Error{Kind: ErrCLI, Message: err.Error()}
	}
	return nil
}

// AuthStatus is the sign-in state of a provider.
type AuthStatus struct {
	State   string // signed_in, signed_out, not_installed, unknown
	Account string // SignedIn: a short label, or ""
	Detail  string // SignedOut, Unknown
}

type tokens struct {
	AccessToken  string  `json:"access_token"`
	RefreshToken string  `json:"refresh_token"`
	IDToken      *string `json:"id_token"`
	ExpiresAt    uint64  `json:"expires_at"`
	AccountID    *string `json:"account_id"`
}

var nowSecs = func() uint64 { return uint64(time.Now().Unix()) }

func strField(m map[string]any, k string) string {
	s, _ := m[k].(string)
	return s
}

func tokensFromReply(reply map[string]any, prev *tokens) (*tokens, error) {
	t := &tokens{AccessToken: strField(reply, "access_token"), RefreshToken: strField(reply, "refresh_token")}
	if t.AccessToken == "" {
		return nil, invalidOutput("the sign-in gave no access token")
	}
	if t.RefreshToken == "" && prev != nil {
		t.RefreshToken = prev.RefreshToken
	}
	if t.RefreshToken == "" {
		return nil, invalidOutput("the sign-in gave no refresh token")
	}
	if id := strField(reply, "id_token"); id != "" {
		t.IDToken = &id
	} else if prev != nil {
		t.IDToken = prev.IDToken
	}
	expiresIn := uint64(3600)
	if f, ok := reply["expires_in"].(float64); ok && f >= 0 {
		expiresIn = uint64(f)
	}
	t.ExpiresAt = nowSecs() + expiresIn
	var acct string
	if t.IDToken != nil {
		acct = accountID(*t.IDToken)
	}
	if acct == "" {
		acct = accountID(t.AccessToken)
	}
	if acct != "" {
		t.AccountID = &acct
	} else if prev != nil {
		t.AccountID = prev.AccountID
	}
	return t, nil
}

func (t *tokens) toJSON() string {
	b, _ := json.Marshal(t)
	return string(b)
}

func tokensFromJSON(text string) *tokens {
	var m map[string]any
	if json.Unmarshal([]byte(text), &m) != nil {
		return nil
	}
	t := &tokens{AccessToken: strField(m, "access_token"), RefreshToken: strField(m, "refresh_token")}
	if t.AccessToken == "" || t.RefreshToken == "" {
		return nil
	}
	if s := strField(m, "id_token"); s != "" {
		t.IDToken = &s
	}
	if s := strField(m, "account_id"); s != "" {
		t.AccountID = &s
	}
	if f, ok := m["expires_at"].(float64); ok && f >= 0 {
		t.ExpiresAt = uint64(f)
	}
	return t
}

func (t *tokens) fresh() bool { return t.ExpiresAt > nowSecs()+refreshMargin }

// label is "Plus plan · a@b.com" from the ID token.
func (t *tokens) label() string {
	if t.IDToken == nil {
		return ""
	}
	c := jwtClaims(*t.IDToken)
	if c == nil {
		return ""
	}
	plan := ""
	if auth, ok := c["https://api.openai.com/auth"].(map[string]any); ok {
		if kind := strField(auth, "chatgpt_plan_type"); kind != "" {
			r := []rune(kind)
			plan = strings.ToUpper(string(r[0])) + string(r[1:]) + " plan"
		}
	}
	email := strField(c, "email")
	switch {
	case plan != "" && email != "":
		return plan + " · " + email
	case plan != "":
		return plan
	}
	return email
}

func jwtClaims(token string) map[string]any {
	parts := strings.Split(token, ".")
	if len(parts) < 2 {
		return nil
	}
	data, err := base64.RawURLEncoding.DecodeString(strings.TrimRight(parts[1], "="))
	if err != nil {
		return nil
	}
	var m map[string]any
	if json.Unmarshal(data, &m) != nil {
		return nil
	}
	return m
}

func accountID(token string) string {
	c := jwtClaims(token)
	if c == nil {
		return ""
	}
	if s := strField(c, "chatgpt_account_id"); s != "" {
		return s
	}
	if auth, ok := c["https://api.openai.com/auth"].(map[string]any); ok {
		if s := strField(auth, "chatgpt_account_id"); s != "" {
			return s
		}
	}
	if orgs, ok := c["organizations"].([]any); ok && len(orgs) > 0 {
		if o, ok := orgs[0].(map[string]any); ok {
			return strField(o, "id")
		}
	}
	return ""
}

func loadTokens(secrets providers.SecretStore, account string) *tokens {
	text := secrets.Get(account)
	if text == "" {
		return nil
	}
	t := tokensFromJSON(text)
	if t == nil {
		slog.Warn("the ChatGPT sign-in is unreadable", "account", account)
	}
	return t
}

func saveTokens(secrets providers.SecretStore, account string, t *tokens) error {
	if err := secrets.Set(account, t.toJSON()); err != nil {
		return &Error{Kind: ErrCLI, Message: "could not save the ChatGPT sign-in: " + err.Error()}
	}
	return nil
}

var refreshMu sync.Mutex

func validTokens(secrets providers.SecretStore, account, tokenURL, refused string, timeout time.Duration) (*tokens, error) {
	notSignedIn := notConfigured("not signed in to ChatGPT")
	usable := func(t *tokens) bool { return t.fresh() && t.AccessToken != refused }
	t := loadTokens(secrets, account)
	if t == nil {
		return nil, notSignedIn
	}
	if usable(t) {
		return t, nil
	}
	refreshMu.Lock()
	defer refreshMu.Unlock()
	if t = loadTokens(secrets, account); t == nil {
		return nil, notSignedIn
	}
	if usable(t) {
		return t, nil
	}
	reply, err := postForm(tokenURL, url.Values{"grant_type": {"refresh_token"}, "refresh_token": {t.RefreshToken}, "client_id": {chatgptClientID}}, timeout)
	if err != nil {
		var e *Error
		if errors.As(err, &e) && e.Kind == ErrHTTP && (e.Status == 400 || e.Status == 401) {
			_ = secrets.Delete(account)
			return nil, notConfigured("not signed in to ChatGPT: the sign-in ended: " + e.Message)
		}
		return nil, err
	}
	next, err := tokensFromReply(reply, t)
	if err != nil {
		return nil, err
	}
	return next, saveTokens(secrets, account, next)
}

func postForm(target string, form url.Values, timeout time.Duration) (map[string]any, error) {
	timeoutMs := uint64(timeout.Milliseconds())
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()
	req, _ := http.NewRequestWithContext(ctx, http.MethodPost, target, strings.NewReader(form.Encode()))
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	req.Header.Set("User-Agent", "Sayso")
	resp, err := httpClientFor(target).Do(req)
	if err != nil {
		return nil, mapTransport(err, timeoutMs)
	}
	defer resp.Body.Close()
	text, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, mapTransport(err, timeoutMs)
	}
	if resp.StatusCode >= 400 {
		return nil, &Error{Kind: ErrHTTP, Status: resp.StatusCode, Message: firstRunes(string(text), 500)}
	}
	var m map[string]any
	if err := json.Unmarshal(text, &m); err != nil {
		return nil, invalidOutput("the sign-in server reply is not JSON: " + err.Error())
	}
	return m, nil
}

// ChatGPTStatus is the sign-in state of a provider.
func ChatGPTStatus(secrets providers.SecretStore, providerID string) AuthStatus {
	if t := loadTokens(secrets, ChatGPTAccountName(providerID)); t != nil {
		return AuthStatus{State: "signed_in", Account: t.label()}
	}
	return AuthStatus{State: "signed_out", Detail: "no ChatGPT sign-in is saved for this provider"}
}

// ---- Sign-in ----

func randomToken() string {
	b := make([]byte, 32)
	rand.Read(b)
	return base64.RawURLEncoding.EncodeToString(b)
}

func authorizeURL(issuer, redirect, challenge, state string) string {
	// The order of the Rust form serializer.
	pairs := [][2]string{{"response_type", "code"}, {"client_id", chatgptClientID}, {"redirect_uri", redirect}, {"scope", chatgptScope},
		{"code_challenge", challenge}, {"code_challenge_method", "S256"}, {"id_token_add_organizations", "true"},
		{"codex_cli_simplified_flow", "true"}, {"state", state}, {"originator", chatgptOriginator}}
	parts := make([]string, len(pairs))
	for i, p := range pairs {
		parts[i] = url.QueryEscape(p[0]) + "=" + url.QueryEscape(p[1])
	}
	return issuer + "/oauth/authorize?" + strings.Join(parts, "&")
}

func signInPage(title, message string) string {
	return "<!doctype html><html><head><meta charset=\"utf-8\"><title>Sayso</title>" +
		"<style>body{font:16px -apple-system,system-ui,sans-serif;display:flex;align-items:center;justify-content:center;height:100vh;margin:0}" +
		"main{max-width:28em;text-align:center}</style></head>" +
		"<body><main><h1>" + title + "</h1><p>" + message + "</p></main></body></html>"
}

// SignInTexts are the page texts in the interface language.
type SignInTexts struct{ DoneTitle, DoneBody, FailedTitle, FailedBody string }

// DefaultSignInTexts are the English texts.
var DefaultSignInTexts = SignInTexts{"You are signed in", "You can close this page and go back to Sayso.", "The sign-in did not work", "Close this page and try again in Sayso."}

// SignInEvent is a step of a sign-in: first the URL to open, then the result.
type SignInEvent struct {
	URL      string
	Finished *AuthStatus
}

// ChatGPTSignIn is a running sign-in.
type ChatGPTSignIn struct {
	Events <-chan SignInEvent
	cancel context.CancelFunc
}

// Cancel stops the sign-in.
func (s *ChatGPTSignIn) Cancel() { s.cancel() }

// ChatGPTEndpoints are the servers of a sign-in (tests change them).
type ChatGPTEndpoints struct {
	Issuer string
	Ports  []int
}

func askToCancel(port int) {
	c, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", port), 2*time.Second)
	if err != nil {
		return
	}
	defer c.Close()
	c.SetDeadline(time.Now().Add(2 * time.Second))
	fmt.Fprintf(c, "GET /cancel HTTP/1.1\r\nHost: 127.0.0.1:%d\r\nConnection: close\r\n\r\n", port)
	c.Read(make([]byte, 64))
}

func bindCallback(ports []int) (net.Listener, int, error) {
	for _, port := range ports {
		for attempt := 0; attempt < 10; attempt++ {
			l, err := net.Listen("tcp", fmt.Sprintf("127.0.0.1:%d", port))
			if err == nil {
				return l, l.Addr().(*net.TCPAddr).Port, nil
			}
			if attempt == 0 {
				askToCancel(port)
			}
			time.Sleep(200 * time.Millisecond)
		}
	}
	list := make([]string, len(ports))
	for i, p := range ports {
		list[i] = fmt.Sprint(p)
	}
	return nil, 0, &Error{Kind: ErrCLI, Message: "the ChatGPT sign-in needs port " + strings.Join(list, " or ") + " on this computer, and it is in use"}
}

// StartChatGPTSignIn opens the callback server and gives the URL to open.
func StartChatGPTSignIn(secrets providers.SecretStore, providerID string, limit time.Duration, ep *ChatGPTEndpoints, texts SignInTexts) (*ChatGPTSignIn, error) {
	if ep == nil {
		ep = &ChatGPTEndpoints{Issuer: chatgptIssuer, Ports: chatgptCallbackPorts}
	}
	l, port, err := bindCallback(ep.Ports)
	if err != nil {
		return nil, err
	}
	redirect := fmt.Sprintf("http://127.0.0.1:%d/auth/callback", port)
	verifier := randomToken()
	sum := sha256.Sum256([]byte(verifier))
	challenge := base64.RawURLEncoding.EncodeToString(sum[:])
	state := randomToken()
	events := make(chan SignInEvent, 2)
	events <- SignInEvent{URL: authorizeURL(ep.Issuer, redirect, challenge, state)}
	ctx, cancel := context.WithTimeout(context.Background(), limit)
	account := ChatGPTAccountName(providerID)
	codes := make(chan string, 1)
	fails := make(chan error, 1)
	mux := http.NewServeMux()
	page := func(w http.ResponseWriter, status int, title, body string) {
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
		w.WriteHeader(status)
		io.WriteString(w, signInPage(title, body))
	}
	mux.HandleFunc("/auth/callback", func(w http.ResponseWriter, r *http.Request) {
		q := r.URL.Query()
		if e := q.Get("error_description"); e != "" || q.Get("error") != "" {
			if e == "" {
				e = q.Get("error")
			}
			page(w, 400, texts.FailedTitle, texts.FailedBody)
			fails <- &Error{Kind: ErrCLI, Message: "the sign-in page reported: " + e}
			return
		}
		if q.Get("state") != state {
			page(w, 400, texts.FailedTitle, texts.FailedBody)
			return
		}
		code := q.Get("code")
		if code == "" {
			page(w, 400, texts.FailedTitle, texts.FailedBody)
			fails <- &Error{Kind: ErrCLI, Message: "the sign-in page gave no code"}
			return
		}
		page(w, 200, texts.DoneTitle, texts.DoneBody)
		select {
		case codes <- code:
		default:
		}
	})
	mux.HandleFunc("/cancel", func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(200)
		fails <- &Error{Kind: ErrCLI, Message: "another sign-in took the port"}
	})
	srv := &http.Server{Handler: mux}
	go srv.Serve(l)
	go func() {
		defer cancel()
		var err error
		select {
		case code := <-codes:
			var reply map[string]any
			reply, err = postForm(ep.Issuer+"/oauth/token", url.Values{"grant_type": {"authorization_code"}, "code": {code},
				"redirect_uri": {redirect}, "client_id": {chatgptClientID}, "code_verifier": {verifier}}, 30*time.Second)
			if err == nil {
				var t *tokens
				if t, err = tokensFromReply(reply, nil); err == nil {
					err = saveTokens(secrets, account, t)
				}
			}
		case err = <-fails:
		case <-ctx.Done():
			if errors.Is(ctx.Err(), context.DeadlineExceeded) {
				err = &Error{Kind: ErrTimeout}
			} else {
				err = &Error{Kind: ErrCLI, Message: "the sign-in was cancelled"}
			}
		}
		srv.Close()
		st := ChatGPTStatus(secrets, providerID)
		if err != nil {
			slog.Warn("the ChatGPT sign-in did not finish", "error", err)
			if st.State == "signed_out" {
				st.Detail = err.Error()
			}
		}
		events <- SignInEvent{Finished: &st}
		close(events)
	}()
	return &ChatGPTSignIn{Events: events, cancel: cancel}, nil
}

// ---- Requests ----

var (
	sessionID       = randomToken()
	defaultModelsMu sync.Mutex
	defaultModels   = map[string]string{}
)

type chatgptAccount struct {
	secrets                 providers.SecretStore
	name, baseURL, tokenURL string
}

func (a chatgptAccount) send(path string, body any, deadline time.Time, timeoutMs uint64) (*http.Response, context.CancelFunc, error) {
	refused := ""
	for {
		remaining := time.Until(deadline)
		if remaining <= 0 {
			return nil, nil, &Error{Kind: ErrTimeout, Ms: timeoutMs}
		}
		t, err := validTokens(a.secrets, a.name, a.tokenURL, refused, remaining)
		if err != nil {
			return nil, nil, err
		}
		ctx, cancel := context.WithDeadline(context.Background(), deadline)
		target := a.baseURL + path
		var req *http.Request
		if body != nil {
			data, _ := json.Marshal(body)
			req, _ = http.NewRequestWithContext(ctx, http.MethodPost, target, bytes.NewReader(data))
			req.Header.Set("Content-Type", "application/json")
			req.Header.Set("session-id", sessionID)
			req.Header.Set("Accept", "text/event-stream")
		} else {
			req, _ = http.NewRequestWithContext(ctx, http.MethodGet, target, nil)
		}
		req.Header.Set("User-Agent", "Sayso")
		req.Header.Set("Authorization", "Bearer "+t.AccessToken)
		req.Header.Set("originator", chatgptOriginator)
		if t.AccountID != nil {
			req.Header.Set("chatgpt-account-id", *t.AccountID)
		}
		resp, err := httpClientFor(target).Do(req)
		if err != nil {
			cancel()
			return nil, nil, mapTransport(err, timeoutMs)
		}
		if resp.StatusCode == 401 && refused == "" {
			resp.Body.Close()
			cancel()
			refused = t.AccessToken
			continue
		}
		if resp.StatusCode >= 400 {
			text, _ := io.ReadAll(resp.Body)
			resp.Body.Close()
			cancel()
			return nil, nil, chatgptHTTPError(resp.StatusCode, string(text))
		}
		return resp, cancel, nil
	}
}

func chatgptHTTPError(status int, body string) error {
	detail := body
	var v map[string]any
	if json.Unmarshal([]byte(body), &v) == nil {
		if m, ok := pointerStr(v, "error", "message"); ok {
			detail = m
		} else if d, ok := v["detail"].(string); ok {
			detail = d
		}
	}
	detail = firstRunes(detail, 500)
	if status == 401 || status == 403 {
		return notConfigured("not signed in to ChatGPT: the backend did not take the sign-in: " + detail)
	}
	return &Error{Kind: ErrHTTP, Status: status, Message: detail}
}

// ChatGPTListModels lists the models the account offers.
func ChatGPTListModels(secrets providers.SecretStore, providerID string, timeout time.Duration) ([]ModelChoice, error) {
	return chatgptAccount{secrets, ChatGPTAccountName(providerID), chatgptBaseURL, chatgptIssuer + "/oauth/token"}.listModels(timeout)
}

func (a chatgptAccount) listModels(timeout time.Duration) ([]ModelChoice, error) {
	timeoutMs := uint64(timeout.Milliseconds())
	resp, cancel, err := a.send("/models?client_version="+chatgptClientVersion, nil, time.Now().Add(timeout), timeoutMs)
	if err != nil {
		return nil, err
	}
	defer cancel()
	defer resp.Body.Close()
	text, err := io.ReadAll(io.LimitReader(resp.Body, 16<<20))
	if err != nil {
		return nil, mapTransport(err, timeoutMs)
	}
	var v any
	if err := json.Unmarshal(text, &v); err != nil {
		return nil, invalidOutput("the model list is not JSON: " + err.Error())
	}
	return parseChatGPTModels(v)
}

func parseChatGPTModels(v any) ([]ModelChoice, error) {
	obj, _ := v.(map[string]any)
	models, ok := obj["models"].([]any)
	if !ok {
		return nil, invalidOutput("the reply has no model list")
	}
	type listed struct {
		priority int64
		choice   ModelChoice
	}
	var all []listed
	for _, raw := range models {
		m, _ := raw.(map[string]any)
		if vis, ok := m["visibility"].(string); ok && vis != "list" {
			continue
		}
		id := strField(m, "slug")
		if id == "" {
			continue
		}
		name := strField(m, "display_name")
		if name == "" {
			name = id
		}
		var levels []string
		if items, ok := m["supported_reasoning_levels"].([]any); ok {
			for _, l := range items {
				if lm, ok := l.(map[string]any); ok {
					if e := strField(lm, "effort"); e != "" {
						levels = append(levels, e)
					}
				}
			}
		}
		c := ModelChoice{ID: id, Name: name, Thinking: config.SortThinking(levels)}
		if d := strField(m, "description"); d != "" {
			c.Description = &d
		}
		p := int64(1<<63 - 1)
		if f, ok := m["priority"].(float64); ok && f == float64(int64(f)) {
			p = int64(f)
		}
		all = append(all, listed{p, c})
	}
	sort.SliceStable(all, func(i, j int) bool { return all[i].priority < all[j].priority })
	seen := map[string]bool{}
	out := []ModelChoice{}
	for _, l := range all {
		if !seen[l.choice.ID] {
			seen[l.choice.ID] = true
			out = append(out, l.choice)
		}
	}
	return out, nil
}

func pickDefault(models []ModelChoice) string {
	for _, m := range models {
		if strings.Contains(m.ID, "mini") {
			return m.ID
		}
	}
	if len(models) > 0 {
		return models[0].ID
	}
	return ""
}

// ChatGPT runs styles on a ChatGPT account.
type ChatGPT struct {
	id, model, thinking string
	account             chatgptAccount
	DefaultTimeout      time.Duration
}

// NewChatGPT makes the enhancer; model "" picks the account's default.
func NewChatGPT(id, model string, secrets providers.SecretStore) *ChatGPT {
	return &ChatGPT{id: id, model: model, thinking: config.DefaultCLIThinking, DefaultTimeout: 8 * time.Second,
		account: chatgptAccount{secrets, ChatGPTAccountName(id), chatgptBaseURL, chatgptIssuer + "/oauth/token"}}
}

// WithThinking sets the level; "" uses the default.
func (c *ChatGPT) WithThinking(level string) *ChatGPT {
	if level == "" {
		level = config.DefaultCLIThinking
	}
	c.thinking = level
	return c
}

// WithEndpoints points the enhancer at other servers (tests).
func (c *ChatGPT) WithEndpoints(baseURL, issuer string) *ChatGPT {
	c.account.baseURL = strings.TrimRight(baseURL, "/")
	c.account.tokenURL = strings.TrimRight(issuer, "/") + "/oauth/token"
	return c
}

func (c *ChatGPT) ID() string    { return c.id }
func (c *ChatGPT) IsCloud() bool { return true }

func (c *ChatGPT) pickModel(r Request, deadline time.Time) (string, error) {
	for _, m := range []string{r.Model, c.model} {
		if strings.TrimSpace(m) != "" {
			return m, nil
		}
	}
	defaultModelsMu.Lock()
	m, ok := defaultModels[c.account.name]
	defaultModelsMu.Unlock()
	if ok {
		return m, nil
	}
	models, err := c.account.listModels(time.Until(deadline))
	if err != nil {
		return "", err
	}
	m = pickDefault(models)
	if m == "" {
		return "", notConfigured("the ChatGPT account offers no models")
	}
	defaultModelsMu.Lock()
	defaultModels[c.account.name] = m
	defaultModelsMu.Unlock()
	return m, nil
}

func (c *ChatGPT) body(model string, r Request) map[string]any {
	return map[string]any{
		"model": model, "stream": true, "store": false, "instructions": r.SystemPrompt,
		"input": []any{map[string]any{"type": "message", "role": "user",
			"content": []any{map[string]any{"type": "input_text", "text": UserMessage(r.Transcript)}}}},
		"tools": []any{}, "tool_choice": "auto", "parallel_tool_calls": false,
		"reasoning": map[string]any{"effort": c.thinking}, "include": []any{},
		"prompt_cache_key": "sayso-" + c.id,
		"text":             map[string]any{"format": map[string]any{"type": "json_schema", "name": schemaName, "schema": OutputSchema(), "strict": true}},
	}
}

func (c *ChatGPT) Enhance(r Request) (Response, error) {
	timeout := r.Timeout
	if timeout == 0 {
		timeout = c.DefaultTimeout
	}
	timeoutMs := uint64(timeout.Milliseconds())
	started := time.Now()
	deadline := started.Add(timeout)
	model, err := c.pickModel(r, deadline)
	if err != nil {
		return Response{}, err
	}
	resp, cancel, err := c.account.send("/responses", c.body(model, r), deadline, timeoutMs)
	if err != nil {
		return Response{}, err
	}
	defer cancel()
	defer resp.Body.Close()
	text, report, err := readStream(resp.Body)
	if err != nil {
		if e, ok := err.(*Error); ok && e.Kind == ErrNetwork && (time.Now().After(deadline) || strings.Contains(e.Message, "timed out")) {
			return Response{}, &Error{Kind: ErrTimeout, Ms: timeoutMs}
		}
		return Response{}, err
	}
	out, err := ParseOutput(text)
	if err != nil {
		return Response{}, err
	}
	return Response{Text: out, ProviderID: c.id, Model: model, ElapsedMs: uint64(time.Since(started).Milliseconds()), Mode: "chatgpt", Report: report}, nil
}

func u64At(v any, keys ...string) *uint64 {
	for _, k := range keys {
		m, ok := v.(map[string]any)
		if !ok {
			return nil
		}
		v = m[k]
	}
	f, ok := v.(float64)
	if !ok || f < 0 {
		return nil
	}
	u := uint64(f)
	return &u
}

func readStream(r io.Reader) (string, *store.ProviderReport, error) {
	sc := bufio.NewScanner(r)
	sc.Buffer(make([]byte, 64*1024), 16<<20)
	var text strings.Builder
	var doneText *string
	for sc.Scan() {
		data, ok := strings.CutPrefix(sc.Text(), "data:")
		if !ok {
			continue
		}
		data = strings.TrimSpace(data)
		if data == "" || data == "[DONE]" {
			continue
		}
		var ev map[string]any
		if json.Unmarshal([]byte(data), &ev) != nil {
			continue
		}
		switch strField(ev, "type") {
		case "response.output_text.delta":
			text.WriteString(strField(ev, "delta"))
		case "response.output_text.done":
			if s, ok := ev["text"].(string); ok {
				doneText = &s
			}
		case "response.refusal.done":
			return "", nil, &Error{Kind: ErrRefused, Message: strField(ev, "refusal")}
		case "response.completed", "response.done":
			resp, _ := ev["response"].(map[string]any)
			final := text.String()
			if doneText != nil {
				final = *doneText
			} else if o, ok := outputText(resp); ok {
				final = o
			}
			if resp["status"] == "incomplete" {
				reason := "unknown"
				if s, ok := pointerStr(resp, "incomplete_details", "reason"); ok {
					reason = s
				}
				return "", nil, invalidOutput("the reply is incomplete: " + reason)
			}
			usage := resp["usage"]
			return final, &store.ProviderReport{
				InputTokens: u64At(usage, "input_tokens"), OutputTokens: u64At(usage, "output_tokens"),
				ThinkingTokens:  u64At(usage, "output_tokens_details", "reasoning_tokens"),
				CacheReadTokens: u64At(usage, "input_tokens_details", "cached_tokens"),
			}, nil
		case "response.failed":
			resp, _ := ev["response"].(map[string]any)
			return "", nil, streamError(resp["error"])
		case "error":
			if e, ok := ev["error"]; ok {
				return "", nil, streamError(e)
			}
			return "", nil, streamError(ev)
		}
	}
	if err := sc.Err(); err != nil {
		if errors.Is(err, context.DeadlineExceeded) {
			return "", nil, &Error{Kind: ErrNetwork, Message: "timed out while the reply streamed"}
		}
		return "", nil, &Error{Kind: ErrNetwork, Message: "the reply stream broke: " + err.Error()}
	}
	return "", nil, &Error{Kind: ErrNetwork, Message: "the reply stream ended before the response completed"}
}

func outputText(resp map[string]any) (string, bool) {
	items, ok := resp["output"].([]any)
	if !ok {
		return "", false
	}
	var parts []string
	for _, it := range items {
		m, _ := it.(map[string]any)
		if m["type"] != "message" {
			continue
		}
		content, _ := m["content"].([]any)
		for _, p := range content {
			pm, _ := p.(map[string]any)
			if pm["type"] == "output_text" {
				if s, ok := pm["text"].(string); ok {
					parts = append(parts, s)
				}
			}
		}
	}
	return strings.Join(parts, ""), len(parts) > 0
}

func streamError(e any) error {
	m, _ := e.(map[string]any)
	code := strField(m, "code")
	if code == "" {
		code = strField(m, "type")
	}
	if code == "" {
		code = "error"
	}
	status := 502
	if strings.Contains(code, "usage_limit") || strings.Contains(code, "rate_limit") {
		status = 429
	}
	return &Error{Kind: ErrHTTP, Status: status, Message: firstRunes(code+": "+strField(m, "message"), 500)}
}
