//! The settings of the selected provider, on the right of the page: its
//! mark and status, a sign-in or a problem, the rows of its kind, the styles
//! that use it, and Remove.

use super::*;
use sayso_core::config::is_local_url;
use sayso_core::models::format_size;

impl ProviderSettings {
    pub(super) fn detail(&self, p: &Provider, default: bool, cx: &mut Context<Self>) -> AnyElement {
        let login = self.model.read(cx).logins.get(&p.id).cloned();
        let status = self.status(p, login.as_ref(), cx);
        let open = self.open.as_ref().filter(|o| o.id == p.id);
        let mut col = div().id(SharedString::from(format!("detail-{}", p.id))).flex().flex_col().w_full().min_w_0().pt(px(26.));
        col = col.child(self.detail_header(p, default, &status, login.as_ref(), cx));
        let has_problem = status.problem.is_some();

        if let (Some(sign_in), Some(open)) = (login.as_ref().and_then(|l| l.sign_in.as_ref()), open) {
            col = col.child(self.sign_in_panel(p, open, sign_in.takes_code(), login.as_ref().and_then(|l| l.url.clone()), cx));
        } else if let Some(problem) = status.problem {
            col = col.child(self.problem(&p.id, problem, cx));
        }
        if let Some(error) = login.as_ref().and_then(|l| l.error.as_ref()) {
            col = col.child(div().pb(px(12.)).child(kit::banner(BannerKind::Warning, error.text(), cx)));
        }

        let mut rows = div().flex().flex_col().w_full().min_w_0().border_t_1().border_color(cx.paper().colors.rule);
        if let Some(info) = cleanup_info(p) {
            col = col.child(div().pb(px(16.)).child(self.cleanup_summary(p, cx)));
            rows = rows
                .child(kit::row_s(t!("settings.providers.language"), cleanup_language(info), div(), cx))
                .child(self.cleanup_file(info, cx))
                .child(self.default_row(p, default, cx));
            return col.child(rows).child(self.footer(p, cx)).into_any_element();
        }
        if matches!(p.kind, ProviderKind::Acp { .. }) && !has_problem && login.as_ref().is_none_or(|l| l.sign_in.is_none()) {
            col = col.child(div().pb(px(16.)).child(Banner::new(BannerKind::Info, t!("settings.providers.agent_notice"))));
        }
        match &p.kind {
            ProviderKind::ChatGpt { .. } => {
                rows = rows.child(self.chatgpt_account(p, login.as_ref(), cx));
            }
            ProviderKind::Acp { .. } => {
                if let Some(open) = open {
                    rows = rows.children(self.agent_rows(p, open, login.as_ref(), cx));
                }
            }
            ProviderKind::ClaudeCli { .. } => {
                if let Some(open) = open {
                    rows = rows.children(self.claude_login(p, open, cx));
                }
            }
            ProviderKind::OpenAiCompatible { .. } => {
                if let Some(open) = open {
                    rows = rows.children(self.server_rows(p, open, cx));
                }
            }
            _ => {}
        }
        if p.kind == ProviderKind::AppleIntelligence {
            let name = self.summary(p, cx);
            rows = rows.child(kit::row_s(t!("settings.providers.model"), t!("settings.providers.apple_model_detail"), plain(name, 13., 16., FontWeight::MEDIUM, cx.paper().colors.ink), cx));
        } else {
            rows = rows.child(kit::row(&t!("settings.providers.model"), &t!("settings.providers.model_detail"), self.model_picker(p, cx), cx));
            rows = rows.children(self.thinking_row(p, cx));
        }
        if let ProviderKind::OpenAiCompatible { base_url, zero_data_retention, .. } = &p.kind
            && base_url.contains("openrouter.ai")
        {
            let (id, on) = (p.id.clone(), *zero_data_retention);
            let model = self.model.clone();
            let switch = Switch::new(SharedString::from(format!("zdr-{}", p.id)), on).label(t!("settings.providers.zero_retention")).on_toggle(move |on, _, cx| {
                let id = id.clone();
                model.update(cx, |m, cx| {
                    m.edit_config(cx, |c| {
                        if let Some(ProviderKind::OpenAiCompatible { zero_data_retention, .. }) = c.ai.providers.iter_mut().find(|p| p.id == id).map(|p| &mut p.kind) {
                            *zero_data_retention = on;
                        }
                    })
                })
            });
            rows = rows.child(kit::row(&t!("settings.providers.zero_retention"), &t!("settings.providers.zero_retention_detail"), switch, cx));
        }
        rows = rows.child(self.default_row(p, default, cx)).child(self.used_by_row(p, default, cx));
        col.child(rows).child(self.footer(p, cx)).into_any_element()
    }

    /// The mark, the name with its badges, the status line, and the main action.
    fn detail_header(&self, p: &Provider, default: bool, status: &Status, login: Option<&ProviderLogin>, cx: &mut Context<Self>) -> Div {
        let c = cx.paper().colors;
        let cleanup = cleanup_info(p);
        let name = cleanup.map_or_else(|| p.name.clone(), |i| i.name.to_string());
        let badge = |label: String, accent: bool| {
            let (bg, fg) = if accent { (c.accent_wash, c.accent) } else { (c.deboss, c.graphite) };
            div().flex_none().px(px(6.)).py(px(2.)).rounded(px(5.)).bg(bg).child(pkit::caps(label, 11., fg))
        };
        let mut title = div().flex().items_center().gap(px(10.)).child(text::display(name.clone(), 28., &c));
        if cleanup.is_some() {
            title = title.child(badge(t!("settings.providers.no_prompts"), true));
        }
        if matches!(p.kind, ProviderKind::Acp { .. }) {
            title = title.child(badge(t!("settings.providers.agent_badge"), true));
        }
        if default {
            title = title.child(badge(t!("settings.providers.default_badge"), false));
        }
        let status_line = div()
            .flex()
            .items_center()
            .gap(px(7.))
            .child(div().flex_none().size(px(8.)).rounded_full().bg(status.dot))
            .child(LiveStatus::new(SharedString::from(format!("detail-status-{}", p.id)), status.line.clone()).min_w_0().child(plain(status.line.clone(), 13., 16., FontWeight::MEDIUM, status.color).truncate()))
            .when_some(cleanup.map(|i| t!("settings.providers.by", publisher = i.publisher)), |d, by| d.child(plain(format!("· {by}"), 13., 16., FontWeight::NORMAL, c.graphite)));
        div()
            .flex()
            .items_start()
            .gap(px(16.))
            .pb(px(22.))
            .child(logo::tile(self.mark(p, cx), 52., cx))
            .child(div().flex().flex_col().flex_1().min_w_0().gap(px(6.)).child(title).child(status_line))
            .children(self.main_action(p, login, cx))
    }

    /// Sign in, Cancel, a download action, or Test connection.
    fn main_action(&self, p: &Provider, login: Option<&ProviderLogin>, cx: &mut Context<Self>) -> Option<AnyElement> {
        let id = p.id.clone();
        if login.is_some_and(|l| l.sign_in.is_some()) {
            return Some(
                Button::new(SharedString::from(format!("cancel-sign-in-{}", p.id)), t!("settings.providers.cancel"))
                    .a11y_label(format!("{}, {}", t!("settings.providers.cancel"), p.name))
                    .on_click(cx.listener(move |this, _, _, cx| this.model.update(cx, |m, cx| m.cancel_sign_in(&id, cx))))
                    .into_any_element(),
            );
        }
        if matches!(login.and_then(|l| l.status.as_ref()), Some(AuthStatus::SignedOut { .. })) {
            return Some(
                Button::new(SharedString::from(format!("sign-in-{}", p.id)), t!("settings.providers.sign_in"))
                    .a11y_label(format!("{}, {}", t!("settings.providers.sign_in"), p.name))
                    .primary()
                    .on_click(cx.listener(move |this, _, _, cx| this.model.update(cx, |m, cx| m.sign_in_provider(&id, cx))))
                    .into_any_element(),
            );
        }
        if let Some(info) = cleanup_info(p) {
            if let Some(action) = self.cleanup_head_action(p, info, cx) {
                return Some(action.into_any_element());
            }
            let ready = self.model.read(cx).cleanup_model_state(info.id) == CleanupModelState::Ready;
            let running = matches!(self.tests.get(&p.id), Some(Test::Running));
            return Some(
                Button::new("test-cleanup", t!("settings.providers.run_test"))
                    .a11y_label(format!("{}, {}", t!("settings.providers.run_test"), info.name))
                    .disabled(running || !ready)
                    .on_click(cx.listener(move |this, _, _, cx| this.test(id.clone(), cx)))
                    .into_any_element(),
            );
        }
        let running = matches!(self.tests.get(&p.id), Some(Test::Running));
        Some(
            Button::new("test-provider", t!("settings.providers.test_connection"))
                .a11y_label(format!("{}, {}", t!("settings.providers.test_connection"), p.name))
                .disabled(running || p.needs_model())
                .on_click(cx.listener(move |this, _, _, cx| this.test(id.clone(), cx)))
                .into_any_element(),
        )
    }

    /// The ChatGPT account, with Sign out.
    fn chatgpt_account(&self, p: &Provider, login: Option<&ProviderLogin>, cx: &mut Context<Self>) -> Div {
        let signed_in = match login.and_then(|l| l.status.as_ref()) {
            Some(AuthStatus::SignedIn { account }) => Some(account.clone()),
            _ => None,
        };
        let about = match &signed_in {
            Some(Some(account)) => account.clone(),
            Some(None) => t!("settings.providers.signed_in"),
            None => t!("settings.providers.chatgpt_account_detail"),
        };
        let control = signed_in.map(|_| {
            let id = p.id.clone();
            Button::new(SharedString::from(format!("sign-out-{}", p.id)), t!("settings.providers.sign_out"))
                .a11y_label(format!("{}, {}", t!("settings.providers.sign_out"), p.name))
                .ghost()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tests.remove(&id);
                    this.model.update(cx, |m, cx| m.sign_out_provider(&id, cx));
                }))
        });
        kit::row_s(t!("settings.providers.chatgpt_account"), about, div().children(control), cx)
    }

    /// The login of the Claude CLI: the user's own, or one for Sayso only, and its folder.
    fn claude_login(&self, p: &Provider, open: &Open, cx: &mut Context<Self>) -> Vec<Div> {
        let c = cx.paper().colors;
        let (name, _) = tool(p);
        let sayso = self.sayso_login(p, cx);
        let entity = cx.entity();
        let id = p.id.clone();
        let choice = Segmented::new(SharedString::from(format!("login-{}", p.id)), [t!("settings.providers.login_mine", tool = name), t!("settings.providers.login_sayso")], usize::from(sayso))
            .label(t!("settings.providers.login"))
            .on_select(move |i, window, cx| entity.update(cx, |this, cx| this.set_sayso_login(&id, i == 1, window, cx)));
        let about = if sayso { t!("settings.providers.login_sayso_detail", tool = name) } else { t!("settings.providers.login_mine_detail", tool = name) };
        let mut rows = vec![kit::row_s(t!("settings.providers.login"), about, choice, cx)];
        if !sayso {
            let field = div()
                .flex()
                .flex_none()
                .items_center()
                .w(px(240.))
                .h(px(34.))
                .px(px(12.))
                .rounded(px(9.))
                .debossed(&c)
                .font_family(sayso_ui::fonts::MONO)
                .text_size(px(12.))
                .child(gpui_kit::component::input::Input::new(&open.folder).aria_label(t!("settings.providers.config_folder")).appearance(false).w_full());
            rows.push(kit::row(&t!("settings.providers.config_folder"), &t!("settings.providers.config_folder_detail"), field, cx));
        }
        rows
    }

    /// The address and the API key of a server. Edit opens a field in place.
    fn server_rows(&self, p: &Provider, open: &Open, cx: &mut Context<Self>) -> Vec<Div> {
        let c = cx.paper().colors;
        let ProviderKind::OpenAiCompatible { base_url, api_key_account, .. } = &p.kind else { return Vec::new() };
        let place = if is_local_url(base_url) { t!("settings.providers.address_local", computer = crate::shell::computer()) } else { t!("settings.providers.address_cloud") };
        let editing = open.edit.as_ref().map(|(what, _)| *what);
        let address = match (editing, &open.edit) {
            (Some(Edit::Address), Some((_, field))) => self.edit_field(field, false, cx).into_any_element(),
            _ => div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(text::mono(base_url.clone(), 12., c.graphite).font_weight(FontWeight::NORMAL))
                .child(self.edit_button(&p.id, Edit::Address, base_url.clone(), cx))
                .into_any_element(),
        };
        let mut rows = vec![kit::row_s(t!("settings.providers.address"), place, address, cx)];
        if api_key_account.is_some() || !is_local_url(base_url) {
            let saved = api_key_account.as_deref().and_then(|a| self.model.read(cx).services.secrets.get(a));
            let control = match (editing, &open.edit) {
                (Some(Edit::ApiKey), Some((_, field))) => self.edit_field(field, true, cx).into_any_element(),
                _ => {
                    let shown = saved.as_deref().map(mask).unwrap_or_else(|| t!("settings.providers.no_key"));
                    let label = if saved.is_some() { t!("settings.providers.replace") } else { t!("settings.providers.add_key") };
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(div().px(px(10.)).py(px(6.)).rounded(px(9.)).debossed(&c).child(text::mono(shown, 12., c.graphite).font_weight(FontWeight::NORMAL)))
                        .child(self.edit_button_labeled(&p.id, Edit::ApiKey, String::new(), label, cx))
                        .into_any_element()
                }
            };
            let detail = t!(crate::shell::os_text!("settings.providers.key_saved_mac", "settings.providers.key_saved_windows", "settings.providers.key_saved_other"));
            rows.push(kit::row_s(t!("settings.providers.api_key"), detail, control, cx));
        }
        rows
    }

    /// The command, the sign-in, and Keep running of an ACP agent.
    fn agent_rows(&self, p: &Provider, open: &Open, login: Option<&ProviderLogin>, cx: &mut Context<Self>) -> Vec<Div> {
        let c = cx.paper().colors;
        let ProviderKind::Acp { command, args, keep_running, .. } = &p.kind else { return Vec::new() };
        let line = crate::agents::join_command(command, args);
        let control = match &open.edit {
            Some((Edit::Command, field)) => self.edit_field(field, false, cx).into_any_element(),
            _ => div()
                .flex()
                .items_center()
                .gap(px(10.))
                .min_w_0()
                .child(text::mono(line.clone(), 12., c.graphite).font_weight(FontWeight::NORMAL).truncate())
                .child(self.edit_button(&p.id, Edit::Command, line.clone(), cx))
                .into_any_element(),
        };
        let mut rows = vec![kit::row_s(t!("settings.providers.command"), t!("settings.providers.command_detail"), control, cx)];
        let account = match login.and_then(|l| l.status.as_ref()) {
            Some(AuthStatus::SignedIn { account: Some(account) }) => account.clone(),
            Some(AuthStatus::SignedIn { account: None }) => t!("settings.providers.agent_signed_in"),
            Some(AuthStatus::SignedOut { .. }) => t!("settings.providers.not_signed_in"),
            _ => t!("settings.providers.checking_login"),
        };
        let id = p.id.clone();
        let again = Button::new(SharedString::from(format!("agent-sign-in-{}", p.id)), t!("settings.providers.sign_in_again"))
            .a11y_label(format!("{}, {}", t!("settings.providers.sign_in_again"), p.name))
            .small()
            .on_click(cx.listener(move |this, _, _, cx| this.model.update(cx, |m, cx| m.sign_in_provider(&id, cx))));
        rows.push(kit::row_s(t!("settings.providers.chatgpt_account"), account, again, cx));
        let (id, on) = (p.id.clone(), *keep_running);
        let model = self.model.clone();
        let switch = Switch::new(SharedString::from(format!("keep-running-{}", p.id)), on).label(t!("settings.providers.keep_running")).on_toggle(move |on, _, cx| {
            let id = id.clone();
            if !on {
                sayso_enhance::stop_acp_agent(&id);
            }
            model.update(cx, |m, cx| {
                m.edit_config(cx, |c| {
                    if let Some(ProviderKind::Acp { keep_running, .. }) = c.ai.providers.iter_mut().find(|p| p.id == id).map(|p| &mut p.kind) {
                        *keep_running = on;
                    }
                })
            });
        });
        rows.push(kit::row(&t!("settings.providers.keep_running"), &t!("settings.providers.keep_running_detail"), switch, cx));
        rows
    }

    fn edit_button(&self, id: &str, what: Edit, value: String, cx: &mut Context<Self>) -> Button {
        self.edit_button_labeled(id, what, value, t!("settings.providers.edit"), cx)
    }

    fn edit_button_labeled(&self, id: &str, what: Edit, value: String, label: String, cx: &mut Context<Self>) -> Button {
        let key = match what {
            Edit::Address => "edit-address",
            Edit::ApiKey => "edit-key",
            Edit::Command => "edit-command",
        };
        Button::new(SharedString::from(format!("{key}-{id}")), label.clone()).a11y_label(label).small().on_click(cx.listener(move |this, _, window, cx| this.start_edit(what, &value, window, cx)))
    }

    /// Open a field in place for the address or the key of the selected provider.
    fn start_edit(&mut self, what: Edit, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.open.as_mut() else { return };
        let field = cx.new(|cx| {
            let state = InputState::new(window, cx).default_value(SharedString::from(value.to_string()));
            if what == Edit::ApiKey { state.masked(true) } else { state }
        });
        field.update(cx, |s, cx| s.focus(window, cx));
        open.edit = Some((what, field));
        cx.notify();
    }

    /// The field of an edit, with Save and Cancel.
    fn edit_field(&self, field: &Entity<InputState>, secret: bool, cx: &mut Context<Self>) -> Div {
        let well = if secret {
            pkit::secret_well_named(field, t!("settings.providers.api_key").into(), cx)
        } else {
            pkit::input_well_named(field, Some(t!("settings.providers.address").into()), cx).font_family(sayso_ui::fonts::MONO)
        };
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(div().w(px(260.)).child(well))
            .child(Button::new("save-edit", t!("settings.providers.save")).primary().small().on_click(cx.listener(|this, _, _, cx| this.save_edit(cx))))
            .child(Button::new("cancel-edit", t!("settings.providers.cancel")).ghost().small().on_click(cx.listener(|this, _, _, cx| {
                if let Some(open) = this.open.as_mut() {
                    open.edit = None;
                }
                cx.notify();
            })))
    }

    /// Save the field of an edit: the address of the server, or its API key.
    fn save_edit(&mut self, cx: &mut Context<Self>) {
        let Some(open) = self.open.as_mut() else { return };
        let Some((what, field)) = open.edit.take() else { return };
        let value = field.read(cx).value().trim().to_string();
        let id = open.id.clone();
        let Some(mut p) = self.provider(&id, cx) else { return };
        if what == Edit::Command {
            let ProviderKind::Acp { command, args, .. } = &mut p.kind else { return };
            let Some((program, words)) = crate::agents::split_command(&value) else { return };
            *command = program;
            *args = words;
            // The agent starts again with the new command at its next request.
            sayso_enhance::stop_acp_agent(&id);
            self.tests.remove(&id);
            self.model.update(cx, |m, cx| {
                m.replace_provider(p, None, cx);
                m.check_provider_login(&id, cx);
                m.load_provider_models(&id, true, cx);
            });
            cx.notify();
            return;
        }
        let ProviderKind::OpenAiCompatible { base_url, api_key_account, .. } = &mut p.kind else { return };
        let mut key = None;
        match what {
            Edit::Address => {
                if !(value.starts_with("http://") || value.starts_with("https://")) {
                    return;
                }
                *base_url = value.trim_end_matches('/').to_string();
            }
            Edit::ApiKey => {
                if value.is_empty() {
                    return;
                }
                api_key_account.get_or_insert_with(|| format!("provider.{id}"));
                key = Some(value);
            }
            Edit::Command => return,
        }
        self.tests.remove(&id);
        self.model.update(cx, |m, cx| {
            m.replace_provider(p, key, cx);
            m.load_provider_models(&id, true, cx);
        });
        cx.notify();
    }

    /// The thinking levels of the model, when it has some.
    fn thinking_row(&self, p: &Provider, cx: &mut Context<Self>) -> Option<Div> {
        let levels = self.model.read(cx).thinking_choices(p);
        if levels.is_empty() {
            return None;
        }
        let selected = p.thinking_sent().and_then(|sent| levels.iter().position(|l| l == sent)).unwrap_or(0);
        let labels: Vec<String> = levels.iter().map(|l| thinking_label(l)).collect();
        let model = self.model.clone();
        let id = p.id.clone();
        let control = Segmented::new(SharedString::from(format!("thinking-{}", p.id)), labels, selected).label(t!("settings.providers.thinking")).on_select(move |i, _, cx| {
            if let Some(level) = levels.get(i).cloned() {
                model.update(cx, |m, cx| m.set_provider_thinking(&id, level, cx));
            }
        });
        Some(kit::row(&t!("settings.providers.thinking"), &t!("settings.providers.thinking_detail"), control, cx))
    }

    /// Default provider: on makes it the default. Off moves the default to the next provider.
    fn default_row(&self, p: &Provider, default: bool, cx: &mut Context<Self>) -> Div {
        let id = p.id.clone();
        let model = self.model.clone();
        let switch = Switch::new(SharedString::from(format!("default-{}", p.id)), default).label(t!("settings.providers.default_provider")).on_toggle(move |on, _, cx| {
            let id = id.clone();
            model.update(cx, |m, cx| {
                m.edit_config(cx, |c| {
                    if on {
                        c.ai.default_provider = Some(id);
                    } else if c.ai.default_provider.as_deref() == Some(id.as_str())
                        && let Some(next) = c.ai.providers.iter().find(|p| p.id != id)
                    {
                        c.ai.default_provider = Some(next.id.clone());
                    }
                })
            });
        });
        kit::row(&t!("settings.providers.default_provider"), &t!("settings.providers.default_provider_detail"), switch, cx)
    }

    /// The styles that run on this provider.
    fn used_by_row(&self, p: &Provider, default: bool, cx: &mut Context<Self>) -> Div {
        let c = cx.paper().colors;
        let names: Vec<String> = {
            let m = self.model.read(cx);
            m.styles
                .entries
                .iter()
                .filter(|e| match e.style.provider.as_deref() {
                    Some(id) => id == p.id,
                    None => default,
                })
                .filter(|e| e.style.run_plan(p) != RunPlan::Nothing)
                .map(|e| e.style.display_name())
                .collect()
        };
        let about = if names.is_empty() { t!("settings.providers.used_by_none") } else { t!("settings.providers.used_by_count", count = names.len()) };
        let mut chips = div().flex().flex_none().gap(px(6.));
        for name in names.iter().take(3) {
            chips = chips.child(div().px(px(8.)).py(px(3.)).rounded(px(6.)).bg(c.deboss).child(plain(name.clone(), 12., 16., FontWeight::MEDIUM, c.ink)));
        }
        if names.len() > 3 {
            chips = chips.child(div().px(px(8.)).py(px(3.)).rounded(px(6.)).bg(c.deboss).child(plain(format!("+{}", names.len() - 3), 12., 16., FontWeight::MEDIUM, c.ink)));
        }
        kit::row_s(t!("settings.providers.used_by"), about, chips, cx)
    }

    /// A note about the provider, and Remove (or Delete download), which asks first.
    fn footer(&self, p: &Provider, cx: &mut Context<Self>) -> Div {
        let c = cx.paper().colors;
        let cleanup = cleanup_info(p);
        let note = match &p.kind {
            ProviderKind::ChatGpt { .. } => Some((t!("settings.providers.chatgpt_note"), false)),
            ProviderKind::ClaudeCli { path, .. } => path.clone().map(|path| (path, true)),
            ProviderKind::OpenAiCompatible { base_url, .. } if !is_local_url(base_url) => Some((t!("settings.providers.cloud_note"), false)),
            ProviderKind::Acp { registry: Some(entry), .. } => Some((t!("settings.providers.agent_from_registry", version = entry.version.clone()), false)),
            ProviderKind::Acp { registry: None, .. } => Some((t!("settings.providers.agent_custom_note"), false)),
            _ => None,
        };
        let left = div().flex_1().min_w_0().children(note.map(|(note, mono)| {
            if mono { text::mono(note, 12., c.graphite).font_weight(FontWeight::NORMAL).into_any_element() } else { plain(note, 12., 17., FontWeight::NORMAL, c.graphite).into_any_element() }
        }));
        let id = p.id.clone();
        let name = cleanup.map_or_else(|| p.name.clone(), |i| i.name.to_string());
        // A cleanup model with its files deletes them. Without them it is only removed.
        let downloaded = cleanup.filter(|i| self.model.read(cx).cleanup_model_state(i.id) == CleanupModelState::Ready);
        let (remove_label, question) = match downloaded {
            Some(info) => (t!("settings.providers.delete_download"), t!("settings.providers.delete_confirm", name = info.name)),
            None => (t!("settings.providers.remove_named", name = name), t!("settings.providers.remove_confirm", name = name)),
        };
        let mut right = div().flex().flex_none().items_center().gap(px(8.));
        if self.confirm_remove.as_deref() == Some(p.id.as_str()) {
            let model_id = downloaded.map(|i| i.id);
            let yes = if downloaded.is_some() { t!("settings.providers.delete") } else { t!("settings.providers.remove") };
            right = right
                .child(div().id("remove-confirm").role(Role::Alert).aria_label(question.clone()).a11y_live(question.clone(), true).child(plain(question, 13., 16., FontWeight::MEDIUM, c.ink)))
                .child(Button::new("remove-yes", yes.clone()).a11y_label(format!("{yes}, {name}")).danger().on_click(cx.listener(move |this, _, _, cx| {
                    this.confirm_remove = None;
                    this.open = None;
                    this.tests.remove(&id);
                    match model_id {
                        Some(model) => this.model.update(cx, |m, cx| m.delete_cleanup_model(model, cx)),
                        None => this.model.update(cx, |m, cx| m.remove_provider(&id, cx)),
                    }
                })))
                .child(Button::new("remove-no", t!("settings.providers.cancel")).ghost().on_click(cx.listener(|this, _, _, cx| {
                    this.confirm_remove = None;
                    cx.notify();
                })));
        } else {
            right = right.child(Button::new("remove-provider", remove_label.clone()).a11y_label(remove_label).ghost().danger().on_click(cx.listener(move |this, _, _, cx| {
                this.confirm_remove = Some(id.clone());
                cx.notify();
            })));
        }
        div().flex().items_center().w_full().gap(px(24.)).mt(px(28.)).pt(px(14.)).pb(px(18.)).border_t_1().border_color(c.rule).child(left).child(right)
    }

    /// The panel of a sign-in that runs: the link to the page, and the field
    /// for the code when the tool takes one.
    pub(super) fn sign_in_panel(&self, p: &Provider, open: &Open, takes_code: bool, url: Option<String>, cx: &mut Context<Self>) -> Div {
        let c = cx.paper().colors;
        let (name, _) = tool(p);
        let about = if takes_code {
            t!("settings.providers.sign_in_code", tool = name)
        } else {
            t!("settings.providers.sign_in_wait", tool = name)
        };
        let link = url.filter(|u| u.starts_with("https://") || u.starts_with("http://")).map(|url| {
            pkit::link(SharedString::from(format!("sign-in-page-{}", p.id)), t!("settings.providers.open_sign_in_page"), 13., cx)
                .font_weight(FontWeight::SEMIBOLD)
                .on_click(move |_, _, _| AppModel::open_url(&url))
        });
        let mut panel = div()
            .id(SharedString::from(format!("sign-in-panel-{}", p.id)))
            .role(Role::Group)
            .aria_label(t!("settings.providers.finish_sign_in"))
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .gap(px(12.))
            .px(px(16.))
            .py(px(14.))
            .rounded(px(10.))
            .bg(c.accent_wash)
            .border_1()
            .border_color(c.accent.opacity(0.18))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(16.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(px(3.))
                            .child(plain(t!("settings.providers.finish_sign_in"), 14., 18., FontWeight::SEMIBOLD, c.ink))
                            .child(ui(about, 13., 18., FontWeight::NORMAL, c.graphite)),
                    )
                    .children(link),
            );
        if takes_code {
            panel = panel.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(pkit::input_well_named(&open.code, Some(t!("settings.providers.code_placeholder").into()), cx).flex_1().min_w_0().font_family(sayso_ui::fonts::MONO))
                    .child(Button::new("send-code", t!("settings.providers.send_code")).primary().on_click(cx.listener(|this, _, window, cx| this.send_code(window, cx)))),
            );
        }
        div().w_full().min_w_0().pb(px(6.)).child(panel)
    }

    /// The whole text of a problem. The text wraps, and Copy takes the words
    /// of the tool.
    pub(super) fn problem(&self, id: &str, problem: Problem, cx: &mut Context<Self>) -> Div {
        let c = cx.paper().colors;
        let text_color = if c.is_dark() { c.danger } else { hsla(0.02, 0.6, 0.23, 1.0) };
        let copy = problem.details.clone().map(|details| {
            let all = format!("{}\n{details}", problem.message);
            pkit::link(SharedString::from(format!("copy-problem-{id}")), t!("settings.providers.copy_details"), 13., cx)
                .text_color(c.danger)
                .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(all.clone())))
        });
        let summary = problem.message.clone();
        div().w_full().min_w_0().pb(px(6.)).child(
            div()
                // An alert, as `kit::notice` is: a screen reader reads a new problem.
                .id(SharedString::from(format!("problem-{id}")))
                .role(Role::Alert)
                .aria_label(summary.clone())
                .a11y_live(summary, true)
                .flex()
                .items_start()
                .w_full()
                .min_w_0()
                .gap(px(12.))
                .px(px(14.))
                .py(px(12.))
                .rounded(px(10.))
                .bg(c.danger_wash)
                .border_1()
                .border_color(c.danger.opacity(0.18))
                .child(icon(Icon::Warning, 16., c.danger).mt(px(1.)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(px(6.))
                        .child(plain(problem.message, 13., 18., FontWeight::NORMAL, text_color))
                        .when_some(problem.details, |d, details| {
                            d.child(div().w_full().min_w_0().font_family(sayso_ui::fonts::MONO).text_size(px(12.)).line_height(px(17.)).text_color(text_color.opacity(0.85)).child(details))
                        }),
                )
                .children(copy),
        )
    }

    /// The model of a provider, with its thinking level: "Haiku 5.5 · Low".
    /// A click opens the provider's list.
    pub(super) fn model_picker(&self, p: &Provider, cx: &mut Context<Self>) -> AnyElement {
        let m = self.model.read(cx);
        let level = p.thinking_sent().filter(|sent| m.thinking_choices(p).iter().any(|level| level == sent)).map(thinking_label);
        let label = match p.model() {
            Some(id) => m.model_label(&p.id, id),
            None if p.needs_model() => t!("settings.providers.choose_model"),
            // macOS chooses the model. Its name comes with the list.
            None if p.kind == ProviderKind::AppleIntelligence => match m.model_lists.get(&p.id) {
                Some(crate::model::ModelList::Ready(list)) if !list.is_empty() => list[0].name.clone(),
                _ => t!("settings.providers.default_model"),
            },
            None => t!("settings.providers.default_model"),
        };
        let label = match level {
            Some(level) => format!("{label} · {level}"),
            None => label,
        };
        let pid = p.id.clone();
        model_picker::trigger(&format!("provider:{}", p.id), label, !p.needs_model(), cx)
            .on_click(cx.listener(move |this, _, _, cx| this.open_models(&pid, cx)))
            .into_any_element()
    }

    /// Open the model list of a saved provider under its trigger.
    pub(super) fn open_models(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(p) = self.provider(id, cx) else { return };
        self.model.update(cx, |m, cx| m.load_provider_models(id, false, cx));
        let page = cx.entity().downgrade();
        let model = self.model.clone();
        let (pick_id, refresh_id) = (p.id.clone(), p.id.clone());
        let request = model_picker::Request {
            key: format!("provider:{}", p.id),
            list_key: p.id.clone(),
            selected: p.model().map(str::to_string),
            extra: None,
            thinking_for: Some(p.id.clone()),
            on_pick: Box::new(move |pick, cx| {
                let Some(choice) = pick else { return };
                let id = pick_id.clone();
                let _ = page.update(cx, |this, cx| {
                    this.tests.remove(&id);
                    this.model.update(cx, |m, cx| m.set_provider_model(&id, choice, cx));
                });
            }),
            on_refresh: Box::new(move |cx| model.update(cx, |m, cx| m.load_provider_models(&refresh_id, true, cx))),
        };
        model_picker::open(&self.model, request, cx);
    }

    /// The button at the right of a cleanup model row, when the model needs
    /// one: Cancel while it downloads, Try again after an error, Download
    /// when the files are not there.
    pub(super) fn cleanup_head_action(&self, p: &Provider, info: &'static CleanupModelInfo, cx: &mut Context<Self>) -> Option<Button> {
        let id = SharedString::from(format!("cleanup-action-{}", p.id));
        let model = info.id;
        let (label, primary) = match self.model.read(cx).cleanup_model_state(model) {
            CleanupModelState::Ready | CleanupModelState::Deleting => return None,
            CleanupModelState::Downloading(_) => (t!("settings.providers.cancel"), false),
            CleanupModelState::Failed(_) | CleanupModelState::DeleteFailed(_) => (t!("settings.providers.try_again"), true),
            CleanupModelState::NotDownloaded => (t!("settings.providers.download"), true),
        };
        let state = self.model.read(cx).cleanup_model_state(model);
        let downloading = matches!(state, CleanupModelState::Downloading(_));
        let delete_failed = matches!(state, CleanupModelState::DeleteFailed(_));
        let button = Button::new(id, label.clone()).a11y_label(format!("{label}, {}", info.name)).on_click(cx.listener(move |this, _, _, cx| {
            this.model.update(cx, |m, cx| {
                if downloading {
                    m.cancel_cleanup_download(model);
                } else if delete_failed {
                    m.delete_cleanup_model(model, cx);
                } else {
                    m.download_cleanup_model(model, cx);
                }
            });
            cx.notify();
        }));
        Some(if primary { button.primary() } else { button.ghost() })
    }

    /// What a cleanup model runs, from the plan of each built-in style.
    pub(super) fn cleanup_summary(&self, p: &Provider, cx: &App) -> Div {
        let c = cx.paper().colors;
        let (runs, needs) = style_names(p);
        div().w_full().min_w_0().pb(px(6.)).child(
            div()
                .id(SharedString::from(format!("cleanup-runs-{}", p.id)))
                .role(Role::Group)
                .aria_label(t!("settings.providers.cleanup_runs", styles = list(&runs)))
                .flex()
                .flex_col()
                .w_full()
                .min_w_0()
                .gap(px(3.))
                .px(px(16.))
                .py(px(14.))
                .rounded(px(10.))
                .bg(c.accent_wash)
                .border_1()
                .border_color(c.accent.opacity(0.18))
                .child(plain(t!("settings.providers.cleanup_runs", styles = list(&runs)), 14., 18., FontWeight::SEMIBOLD, c.ink))
                .child(ui(t!("settings.providers.cleanup_needs", styles = list(&needs)), 13., 18., FontWeight::NORMAL, c.graphite)),
        )
    }

    /// The model file: its size, its name, and a button that shows it.
    pub(super) fn cleanup_file(&self, info: &'static CleanupModelInfo, cx: &mut Context<Self>) -> Div {
        let c = cx.paper().colors;
        let path = self.model.read(cx).cleanup_model_file(info.id);
        let reveal = path.map(|path| {
            let label = t!(crate::shell::os_text!("settings.providers.show_in_finder", "settings.providers.show_in_explorer", "settings.providers.show_in_folder"));
            Button::new(SharedString::from(format!("reveal-{}", info.id)), label.clone())
                .a11y_label(format!("{label}, {}", info.name))
                .small()
                .on_click(move |_, _, _| crate::shell::reveal(&path))
        });
        let control = div().flex().flex_none().items_center().gap(px(12.)).child(text::mono(info.file, 12., c.graphite).font_weight(FontWeight::NORMAL)).children(reveal);
        kit::row_s(t!("settings.providers.model_file"), format_size(info.size_bytes), control, cx)
    }
}

/// A saved key with all but its last four characters hidden.
fn mask(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    let shown: String = chars[chars.len().saturating_sub(4)..].iter().collect();
    let start: String = chars.iter().take(chars.len().min(6).saturating_sub(4)).collect();
    format!("{start}••••••{shown}")
}

#[cfg(test)]
mod tests {
    use super::mask;

    #[test]
    fn a_key_shows_only_its_end() {
        assert_eq!(mask("sk-or-v1-abcdef123456"), "sk••••••3456");
        assert_eq!(mask("abc"), "••••••abc");
    }
}
