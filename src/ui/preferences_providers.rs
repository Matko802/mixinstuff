use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use crate::App;
use crate::net::provider::{self, Provider};
use crate::net::ytmusic::AuthState;
use crate::ui::window::MainWindow;

pub fn build_page(win: &Rc<MainWindow>, ctx: &Rc<App>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder().name("providers").title("Music Providers").icon_name("folder-music-symbolic").build();
    let group = adw::PreferencesGroup::builder()
        .title("Providers")
        .description("Pick which services supply Home and Explore. Keep several on to switch between them in the tabs.")
        .build();

    let mut switches = Vec::new();
    for provider in Provider::all() {
        let row = adw::SwitchRow::builder().title(provider.name()).build();
        row.add_prefix(&gtk::Image::builder().icon_name(provider.icon()).build());
        group.add(&row);
        switches.push((provider, row));
    }

    let sc_account_row = adw::ActionRow::builder().title("SoundCloud Account").build();
    let sc_account_button = gtk::Button::builder().valign(gtk::Align::Center).build();
    sc_account_row.add_suffix(&sc_account_button);
    group.add(&sc_account_row);

    let ws_server_row = adw::ActionRow::builder().title("WatchShark Server").activatable(true).build();
    group.add(&ws_server_row);
    let ws_account_row = adw::ActionRow::builder().title("WatchShark Account").build();
    let ws_account_button = gtk::Button::builder().valign(gtk::Align::Center).build();
    ws_account_row.add_suffix(&ws_account_button);
    group.add(&ws_account_row);
    page.add(&group);

    let state = Rc::new(ProviderRows {
        ctx: ctx.clone(),
        win: Rc::downgrade(win),
        switches,
        sc_account_row,
        sc_account_button,
        ws_server_row,
        ws_account_row,
        ws_account_button,
    });
    for (provider, row) in &state.switches {
        let state = state.clone();
        let row = row.clone();
        let provider = *provider;
        row.connect_active_notify(move |row| state.toggled(provider, row.is_active()));
    }
    {
        let state = state.clone();
        let button = state.sc_account_button.clone();
        button.connect_clicked(move |_| state.sc_account_clicked());
    }
    {
        let state = state.clone();
        let row = state.ws_server_row.clone();
        row.connect_activated(move |_| state.ws_server_clicked());
    }
    {
        let state = state.clone();
        let button = state.ws_account_button.clone();
        button.connect_clicked(move |_| state.ws_account_clicked());
    }
    state.refresh();
    page
}

struct ProviderRows {
    ctx: Rc<App>,
    win: std::rc::Weak<MainWindow>,
    switches: Vec<(Provider, adw::SwitchRow)>,
    sc_account_row: adw::ActionRow,
    sc_account_button: gtk::Button,
    ws_server_row: adw::ActionRow,
    ws_account_row: adw::ActionRow,
    ws_account_button: gtk::Button,
}

impl ProviderRows {
    fn toast(&self, message: &str) {
        if let Some(win) = self.win.upgrade() {
            win.add_toast(message);
        }
    }

    fn enabled(&self, provider: Provider) -> bool {
        match provider {
            Provider::YouTube => provider::ytm_enabled(&self.ctx.paths),
            Provider::SoundCloud => provider::sc_enabled(&self.ctx.paths),
            Provider::WatchShark => provider::ws_enabled(&self.ctx.paths),
        }
    }

    fn toggled(self: &Rc<Self>, provider: Provider, active: bool) {
        if !active && !Provider::all().iter().any(|p| *p != provider && self.enabled(*p)) {
            self.toast("Keep at least one provider enabled");
            self.refresh();
            return;
        }
        match provider {
            Provider::YouTube => provider::set_ytm_enabled(&self.ctx.paths, active),
            Provider::SoundCloud => provider::set_sc_enabled(&self.ctx.paths, active),
            Provider::WatchShark => provider::set_ws_enabled(&self.ctx.paths, active),
        }
        if !active && provider::active(&self.ctx.paths) == provider {
            let fallback = Provider::all().into_iter().find(|p| *p != provider && self.enabled(*p)).unwrap_or(Provider::YouTube);
            provider::set_active(&self.ctx.paths, fallback);
        }
        self.refresh();
        if let Some(win) = self.win.upgrade() {
            win.refresh_providers();
        }
    }

    fn refresh(&self) {
        for (provider, row) in &self.switches {
            row.set_active(self.enabled(*provider));
        }
        for (provider, row) in &self.switches {
            match provider {
                Provider::YouTube => {
                    let subtitle = match self.ctx.net.client().auth_state() {
                        AuthState::Authenticated(info) if !info.name.is_empty() => format!("Signed in as {}", info.name),
                        AuthState::Authenticated(_) => "Signed in".to_owned(),
                        _ => "Not signed in".to_owned(),
                    };
                    row.set_subtitle(&glib::markup_escape_text(&subtitle));
                }
                Provider::SoundCloud => row.set_subtitle("Public catalog: search, charts, tracks and sets"),
                Provider::WatchShark => row.set_subtitle("Music from your own server"),
            }
        }
        let sc = self.ctx.net.soundcloud();
        let sc_on = self.enabled(Provider::SoundCloud);
        self.sc_account_row.set_sensitive(sc_on);
        if sc.has_token() {
            self.sc_account_row.set_subtitle("Connected: likes sync with your account");
            self.sc_account_button.set_label("Disconnect");
            self.sc_account_button.remove_css_class("suggested-action");
            self.sc_account_button.add_css_class("destructive-action");
        } else {
            self.sc_account_row.set_subtitle("Optional: connect for likes");
            self.sc_account_button.set_label("Connect");
            self.sc_account_button.remove_css_class("destructive-action");
            self.sc_account_button.add_css_class("suggested-action");
        }
        let ws = self.ctx.net.watchshark();
        let ws_on = self.enabled(Provider::WatchShark);
        let server = ws.try_read_server();
        self.ws_server_row.set_subtitle(&glib::markup_escape_text(&server));
        self.ws_server_row.set_sensitive(ws_on);
        self.ws_account_row.set_sensitive(ws_on);
        if ws.has_account() {
            let subtitle = match ws.try_username() {
                Some(name) => format!("Connected as {name}"),
                None => "Connected".to_owned(),
            };
            self.ws_account_row.set_subtitle(&glib::markup_escape_text(&subtitle));
            self.ws_account_button.set_label("Disconnect");
            self.ws_account_button.remove_css_class("suggested-action");
            self.ws_account_button.add_css_class("destructive-action");
        } else {
            self.ws_account_row.set_subtitle("Sign in for likes");
            self.ws_account_button.set_label("Connect");
            self.ws_account_button.remove_css_class("destructive-action");
            self.ws_account_button.add_css_class("suggested-action");
        }
    }

    fn open_uri(&self, uri: &str) {
        let Some(win) = self.win.upgrade() else { return };
        let uri = uri.to_owned();
        gtk::UriLauncher::new(&uri).launch(Some(win.window()), gtk::gio::Cancellable::NONE, move |result| {
            if let Err(err) = result {
                tracing::warn!(%uri, %err, "could not open the browser");
            }
        });
    }

    fn sc_account_clicked(self: &Rc<Self>) {
        let sc = self.ctx.net.soundcloud().clone();
        if sc.has_token() {
            let this = self.clone();
            glib::spawn_future_local(async move {
                sc.set_token(None).await;
                this.refresh();
                this.toast("Disconnected from SoundCloud");
            });
            return;
        }
        let Some(win) = self.win.upgrade() else { return };
        let dialog = adw::AlertDialog::builder()
            .heading("Connect SoundCloud")
            .body("Sign in on soundcloud.com, open the browser devtools (F12) on the Network tab, reload, open any api-v2 request and copy the token from its Authorization header (the part after OAuth). Likes sync only; everything else works without it.")
            .default_response("connect")
            .close_response("cancel")
            .build();
        let entry = adw::PasswordEntryRow::builder().title("OAuth Token").build();
        let listbox = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list", "songs-list"]).build();
        listbox.append(&entry);
        let link = gtk::Button::builder().label("Open SoundCloud").halign(gtk::Align::Center).build();
        link.add_css_class("flat");
        {
            let this = Rc::downgrade(self);
            link.connect_clicked(move |_| {
                if let Some(this) = this.upgrade() {
                    this.open_uri("https://soundcloud.com/");
                }
            });
        }
        let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).margin_top(6).build();
        content.append(&listbox);
        content.append(&link);
        dialog.set_extra_child(Some(&content));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("connect", "Connect");
        dialog.set_response_appearance("connect", adw::ResponseAppearance::Suggested);

        let this = self.clone();
        dialog.connect_response(None, move |_, response| {
            if response != "connect" {
                return;
            }
            let token = entry.text().trim().to_owned();
            if token.is_empty() {
                return;
            }
            let this = this.clone();
            glib::spawn_future_local(async move {
                let sc = this.ctx.net.soundcloud().clone();
                sc.set_token(Some(token)).await;
                match this.ctx.net.spawn(async move { sc.me().await }).await {
                    Ok(Ok(name)) => {
                        this.refresh();
                        this.toast(&format!("Connected to SoundCloud as {name}"));
                    }
                    Ok(Err(err)) => {
                        let sc = this.ctx.net.soundcloud().clone();
                        sc.set_token(None).await;
                        this.refresh();
                        this.toast(&format!("SoundCloud: {err}"));
                    }
                    Err(err) => {
                        let sc = this.ctx.net.soundcloud().clone();
                        sc.set_token(None).await;
                        this.refresh();
                        this.toast(&format!("SoundCloud: {err}"));
                    }
                }
            });
        });
        dialog.present(Some(win.window()));
    }

    fn ws_server_clicked(self: &Rc<Self>) {
        let Some(win) = self.win.upgrade() else { return };
        let dialog = adw::AlertDialog::builder()
            .heading("WatchShark Server")
            .body("The address of your WatchShark instance. Self-hosters point this at their own server.")
            .default_response("save")
            .close_response("cancel")
            .build();
        let entry = adw::EntryRow::builder().title("Server URL").build();
        let ws = self.ctx.net.watchshark().clone();
        entry.set_text(&ws.try_read_server());
        let listbox = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list", "songs-list"]).build();
        listbox.append(&entry);
        let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).margin_top(6).build();
        content.append(&listbox);
        dialog.set_extra_child(Some(&content));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("save", "Save");
        dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);

        let this = self.clone();
        dialog.connect_response(None, move |_, response| {
            if response != "save" {
                return;
            }
            let server = entry.text().trim().to_owned();
            let this = this.clone();
            glib::spawn_future_local(async move {
                let ws = this.ctx.net.watchshark().clone();
                match ws.set_server(&server).await {
                    Ok(clean) => {
                        this.refresh();
                        this.toast(&format!("WatchShark server: {clean}"));
                        if let Some(win) = this.win.upgrade() {
                            win.reload_provider_content();
                        }
                    }
                    Err(err) => this.toast(&err),
                }
            });
        });
        dialog.present(Some(win.window()));
    }

    fn ws_account_clicked(self: &Rc<Self>) {
        let ws = self.ctx.net.watchshark().clone();
        if ws.has_account() {
            let this = self.clone();
            glib::spawn_future_local(async move {
                this.ctx.net.watchshark().logout().await;
                this.refresh();
                this.toast("Disconnected from WatchShark");
            });
            return;
        }
        let Some(win) = self.win.upgrade() else { return };
        let dialog = adw::AlertDialog::builder()
            .heading("Connect WatchShark")
            .body("Sign in with your WatchShark account. Needed for likes; browsing works without it.")
            .default_response("connect")
            .close_response("cancel")
            .build();
        let login_entry = adw::EntryRow::builder().title("Username or Email").build();
        let password_entry = adw::PasswordEntryRow::builder().title("Password").build();
        let listbox = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list", "songs-list"]).build();
        listbox.append(&login_entry);
        listbox.append(&password_entry);
        let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).margin_top(6).build();
        content.append(&listbox);
        dialog.set_extra_child(Some(&content));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("connect", "Connect");
        dialog.set_response_appearance("connect", adw::ResponseAppearance::Suggested);

        let this = self.clone();
        dialog.connect_response(None, move |_, response| {
            if response != "connect" {
                return;
            }
            let login = login_entry.text().trim().to_owned();
            let password = password_entry.text().to_owned();
            if login.is_empty() || password.is_empty() {
                return;
            }
            let this = this.clone();
            glib::spawn_future_local(async move {
                let ws = this.ctx.net.watchshark().clone();
                let server = ws.server().await;
                match this.ctx.net.spawn(async move { ws.login(&server, &login, &password).await }).await {
                    Ok(Ok(name)) => {
                        this.refresh();
                        this.toast(&format!("Connected to WatchShark as {name}"));
                    }
                    Ok(Err(err)) => this.toast(&format!("WatchShark: {err}")),
                    Err(err) => this.toast(&format!("WatchShark: {err}")),
                }
            });
        });
        dialog.present(Some(win.window()));
    }
}
