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
        .description("Pick which services supply Home and Explore. Keep both on to switch between them in the tabs.")
        .build();

    let ytm_row = adw::SwitchRow::builder().title(Provider::YouTube.name()).active(provider::ytm_enabled(&ctx.paths)).build();
    ytm_row.add_prefix(&gtk::Image::builder().icon_name(Provider::YouTube.icon()).build());
    let sc_row = adw::SwitchRow::builder().title(Provider::SoundCloud.name()).active(provider::sc_enabled(&ctx.paths)).build();
    sc_row.add_prefix(&gtk::Image::builder().icon_name(Provider::SoundCloud.icon()).build());

    let account_row = adw::ActionRow::builder().title("SoundCloud Account").build();
    let account_button = gtk::Button::builder().valign(gtk::Align::Center).build();
    account_row.add_suffix(&account_button);

    group.add(&ytm_row);
    group.add(&sc_row);
    group.add(&account_row);
    page.add(&group);

    let state = Rc::new(ProviderRows {
        ctx: ctx.clone(),
        win: Rc::downgrade(win),
        ytm_row,
        sc_row,
        account_row,
        account_button,
    });
    {
        let state = state.clone();
        let row = state.ytm_row.clone();
        row.connect_active_notify(move |row| state.toggled(Provider::YouTube, row.is_active()));
    }
    {
        let state = state.clone();
        let row = state.sc_row.clone();
        row.connect_active_notify(move |row| state.toggled(Provider::SoundCloud, row.is_active()));
    }
    {
        let state = state.clone();
        let button = state.account_button.clone();
        button.connect_clicked(move |_| state.account_clicked());
    }
    state.refresh();
    page
}

struct ProviderRows {
    ctx: Rc<App>,
    win: std::rc::Weak<MainWindow>,
    ytm_row: adw::SwitchRow,
    sc_row: adw::SwitchRow,
    account_row: adw::ActionRow,
    account_button: gtk::Button,
}

impl ProviderRows {
    fn toast(&self, message: &str) {
        if let Some(win) = self.win.upgrade() {
            win.add_toast(message);
        }
    }

    fn toggled(self: &Rc<Self>, provider: Provider, active: bool) {
        if !active {
            let other = match provider {
                Provider::YouTube => provider::sc_enabled(&self.ctx.paths),
                Provider::SoundCloud => provider::ytm_enabled(&self.ctx.paths),
            };
            if !other {
                self.toast("Keep at least one provider enabled");
                self.refresh();
                return;
            }
        }
        match provider {
            Provider::YouTube => provider::set_ytm_enabled(&self.ctx.paths, active),
            Provider::SoundCloud => provider::set_sc_enabled(&self.ctx.paths, active),
        }
        if !active && provider::active(&self.ctx.paths) == provider {
            let fallback = if provider == Provider::YouTube { Provider::SoundCloud } else { Provider::YouTube };
            provider::set_active(&self.ctx.paths, fallback);
        }
        self.refresh();
        if let Some(win) = self.win.upgrade() {
            win.refresh_providers();
        }
    }

    fn refresh(&self) {
        let ytm_on = provider::ytm_enabled(&self.ctx.paths);
        let sc_on = provider::sc_enabled(&self.ctx.paths);
        self.ytm_row.set_active(ytm_on);
        self.sc_row.set_active(sc_on);
        let subtitle = match self.ctx.net.client().auth_state() {
            AuthState::Authenticated(info) if !info.name.is_empty() => format!("Signed in as {}", info.name),
            AuthState::Authenticated(_) => "Signed in".to_owned(),
            _ => "Not signed in".to_owned(),
        };
        self.ytm_row.set_subtitle(&glib::markup_escape_text(&subtitle));
        self.sc_row.set_subtitle("Public catalog: search, charts, tracks and sets");
        self.account_row.set_sensitive(sc_on);
        let sc = self.ctx.net.soundcloud();
        if sc.has_token() {
            self.account_row.set_subtitle("Connected: likes sync with your account");
            self.account_button.set_label("Disconnect");
            self.account_button.remove_css_class("suggested-action");
            self.account_button.add_css_class("destructive-action");
        } else {
            self.account_row.set_subtitle("Optional: connect for likes");
            self.account_button.set_label("Connect");
            self.account_button.remove_css_class("destructive-action");
            self.account_button.add_css_class("suggested-action");
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

    fn account_clicked(self: &Rc<Self>) {
        let sc = self.ctx.net.soundcloud().clone();
        if sc.has_token() {
            let sc = sc.clone();
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
}
