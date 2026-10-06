
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use webkit6::prelude::*;

use crate::ui::context::UiContext;

const LOGIN_URL: &str = "https://accounts.google.com/ServiceLogin?ltmpl=music&service=youtube&uilel=3&passive=true&continue=https%3A%2F%2Fmusic.youtube.com%2Flibrary";
const BROWSER_UA: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:147.0) Gecko/20100101 Firefox/147.0";

#[derive(Clone)]
enum LoginMode {
    YouTube,
    WatchShark { server: String },
}

impl LoginMode {
    fn title(&self) -> &'static str {
        match self {
            LoginMode::YouTube => "Sign in to YouTube Music",
            LoginMode::WatchShark { .. } => "Sign in to WatchShark",
        }
    }

    fn start_url(&self) -> String {
        match self {
            LoginMode::YouTube => LOGIN_URL.to_owned(),
            LoginMode::WatchShark { server } => format!("{server}/"),
        }
    }
}

pub struct LoginDialog {
    window: adw::Window,
    webview: webkit6::WebView,
    web_status: gtk::Label,
    captured: RefCell<BTreeMap<String, String>>,
    finished: Cell<bool>,
    tried_token: RefCell<Option<String>>,
    mode: LoginMode,
    ctx: Rc<UiContext>,
    on_success: RefCell<Option<Rc<dyn Fn()>>>,
}

impl LoginDialog {
    pub fn new(ctx: Rc<UiContext>, parent: &impl IsA<gtk::Window>) -> Rc<Self> {
        Self::open(ctx, parent, LoginMode::YouTube)
    }

    pub fn new_watchshark(ctx: Rc<UiContext>, parent: &impl IsA<gtk::Window>, server: &str) -> Rc<Self> {
        Self::open(ctx, parent, LoginMode::WatchShark { server: server.to_owned() })
    }

    fn open(ctx: Rc<UiContext>, parent: &impl IsA<gtk::Window>, mode: LoginMode) -> Rc<Self> {
        let window = adw::Window::builder()
            .modal(true)
            .transient_for(parent)
            .default_width(520)
            .default_height(640)
            .title(mode.title())
            .build();
        let toolbar = adw::ToolbarView::new();
        window.set_content(Some(&toolbar));
        let header = adw::HeaderBar::builder().show_title(true).build();
        let dismiss = gtk::Button::builder().label(match mode {
            LoginMode::YouTube => "Skip",
            _ => "Close",
        }).build();
        header.pack_end(&dismiss);
        toolbar.add_top_bar(&header);

        let webview = webkit6::WebView::new();
        if let Some(settings) = WebViewExt::settings(&webview) {
            settings.set_user_agent(Some(BROWSER_UA));
            settings.set_enable_javascript(true);
            settings.set_enable_webgl(true);
            settings.set_enable_html5_local_storage(true);
            settings.set_enable_html5_database(true);
        }
        webview.set_vexpand(true);
        let web_status = gtk::Label::builder().visible(false).margin_top(6).margin_bottom(6).build();
        let page = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
        page.append(&webview);
        page.append(&web_status);
        toolbar.set_content(Some(&page));

        let dialog = Rc::new(Self {
            window,
            webview,
            web_status,
            captured: RefCell::new(BTreeMap::new()),
            finished: Cell::new(false),
            tried_token: RefCell::new(None),
            mode,
            ctx,
            on_success: RefCell::new(None),
        });
        {
            let win = dialog.window.clone();
            dismiss.connect_clicked(move |_| win.close());
        }
        {
            let weak = Rc::downgrade(&dialog);
            dialog.window.connect_close_request(move |_| {
                if let Some(d) = weak.upgrade() {
                    if matches!(d.mode, LoginMode::YouTube) && !d.ctx.net.client().is_authenticated() {
                        set_login_skipped(&d.ctx.paths, true);
                    }
                }
                glib::Propagation::Proceed
            });
        }
        {
            let weak = Rc::downgrade(&dialog);
            dialog.webview.connect_resource_load_started(move |_, _, request| {
                if let Some(d) = weak.upgrade() {
                    d.on_resource_load_started(request);
                }
            });
        }
        let url = dialog.mode.start_url();
        dialog.webview.load_uri(&url);
        dialog
    }

    pub fn set_on_success(&self, f: impl Fn() + 'static) {
        self.on_success.replace(Some(Rc::new(f)));
    }

    pub fn present(&self) {
        self.window.present();
    }

    pub fn connect_close(&self, f: impl Fn() + 'static) {
        self.window.connect_close_request(move |_| {
            f();
            glib::Propagation::Proceed
        });
    }

    fn set_status(&self, color: &str, text: &str) {
        self.web_status.set_markup(&format!("<span color='{color}'>{}</span>", glib::markup_escape_text(text)));
        self.web_status.set_visible(true);
    }

    fn on_resource_load_started(self: &Rc<Self>, request: &webkit6::URIRequest) {
        if self.finished.get() {
            return;
        }
        match self.mode.clone() {
            LoginMode::YouTube => self.on_youtube_request(request),
            LoginMode::WatchShark { server } => self.on_watchshark_request(&server),
        }
    }

    fn on_youtube_request(self: &Rc<Self>, request: &webkit6::URIRequest) {
        if self.finished.get() {
            return;
        }
        let uri = request.uri().map(|u| u.to_string()).unwrap_or_default();
        if !uri.contains("music.youtube.com/youtubei/v1/browse") {
            return;
        }
        let Some(headers) = request.http_headers() else {
            return;
        };
        let cookie = headers.one("Cookie").map(|c| c.to_string()).unwrap_or_default();
        let auth = headers.one("Authorization").map(|a| a.to_string()).unwrap_or_default();
        let has_sapisid = cookie.contains("SAPISID");
        if !has_sapisid && !auth.contains("SAPISIDHASH") {
            return;
        }
        let mut captured = BTreeMap::new();
        headers.foreach(|name, value| {
            captured.insert(name.to_string(), value.to_string());
        });
        self.captured.replace(captured);
        if has_sapisid {
            self.finished.set(true);
            let weak = Rc::downgrade(self);
            glib::idle_add_local_once(move || {
                if let Some(d) = weak.upgrade() {
                    d.finish_login();
                }
            });
        } else {
            self.fetch_cookies_from_jar();
        }
    }

    fn fetch_cookies_from_jar(self: &Rc<Self>) {
        let Some(manager) = self.webview.network_session().and_then(|s| s.cookie_manager()) else {
            return;
        };
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let cookies = manager.cookies_future("https://music.youtube.com").await;
            let Some(d) = weak.upgrade() else { return };
            let mut cookies = match cookies {
                Ok(cookies) => cookies,
                Err(err) => {
                    d.set_status("red", &format!("Could not read cookies: {err}"));
                    return;
                }
            };
            let cookie = cookies.iter_mut().filter_map(|c| Some(format!("{}={}", c.name()?, c.value()?))).collect::<Vec<_>>().join("; ");
            if !cookie.contains("SAPISID") {
                d.set_status("orange", "Not signed in yet, the session has no SAPISID cookie.");
                return;
            }
            let mut captured = d.captured.borrow().clone();
            captured.insert("Cookie".into(), cookie);
            captured.entry("User-Agent".into()).or_insert_with(|| BROWSER_UA.to_owned());
            d.captured.replace(captured);
            d.finished.set(true);
            d.finish_login();
        });
    }

    fn on_watchshark_request(self: &Rc<Self>, server: &str) {
        let Some(manager) = self.webview.network_session().and_then(|s| s.cookie_manager()) else {
            return;
        };
        let server = server.to_owned();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let cookies = manager.cookies_future(&server).await;
            let Some(d) = weak.upgrade() else { return };
            let mut cookies = match cookies {
                Ok(cookies) => cookies,
                Err(err) => {
                    d.set_status("red", &format!("Could not read cookies: {err}"));
                    return;
                }
            };
            let token = cookies.iter_mut().find_map(|c| {
                let (name, value) = (c.name()?, c.value()?);
                (name.as_str() == "ws_token").then(|| value.to_string())
            }).filter(|t| !t.is_empty());
            let Some(token) = token else { return };
            if d.tried_token.borrow().as_deref() == Some(token.as_str()) {
                return;
            }
            d.tried_token.replace(Some(token.clone()));
            d.finished.set(true);
            d.set_status("blue", "Signing in...");
            let ws = d.ctx.net.watchshark().clone();
            match ws.adopt_session(token).await {
                Ok(name) => {
                    tracing::info!(name = %name, "watchshark login successful");
                    d.set_status("green", "Signed in.");
                    if let Some(f) = d.on_success.borrow().as_ref() {
                        f();
                    }
                    d.window.close();
                }
                Err(err) => {
                    tracing::warn!(%err, "watchshark login failed");
                    d.finished.set(false);
                    d.tried_token.replace(None);
                    d.set_status("red", "WatchShark refused the session. Try again.");
                }
            }
        });
    }

    fn finish_login(self: &Rc<Self>) {
        self.set_status("blue", "Signing in...");
        let json = serde_json::to_string(&*self.captured.borrow()).unwrap_or_default();
        self.login_with(json, move |d, ok| {
            if ok {
                d.set_status("green", "Signed in.");
                d.clear_webkit_cookies();
                d.window.close();
            } else {
                d.finished.set(false);
                d.set_status("red", "The server did not accept the session. Try again.");
            }
        });
    }

    fn clear_webkit_cookies(&self) {
        if let Some(manager) = self.webview.network_session().and_then(|s| s.website_data_manager()) {
            manager.clear(webkit6::WebsiteDataTypes::COOKIES, glib::TimeSpan::from_seconds(0), None::<&gtk::gio::Cancellable>, |_| {});
        }
    }

    fn login_with(self: &Rc<Self>, input: String, done: impl Fn(&Rc<Self>, bool) + 'static) {
        let client = self.ctx.net.client().clone();
        let handle = self.ctx.net.spawn(async move { client.login(&input).await });
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let outcome = handle.await;
            let Some(d) = weak.upgrade() else { return };
            match outcome {
                Ok(Ok(info)) => {
                    tracing::info!(name = %info.name, "login successful");
                    set_login_skipped(&d.ctx.paths, false);
                    if let Some(f) = d.on_success.borrow().as_ref() {
                        f();
                    }
                    done(&d, true);
                }
                Ok(Err(err)) => {
                    tracing::warn!(%err, "login failed");
                    done(&d, false);
                }
                Err(_) => done(&d, false),
            }
        });
    }
}

pub const LOGIN_SKIPPED_PREF: &str = "login_skipped";

pub fn login_skipped(paths: &crate::paths::Paths) -> bool {
    paths.read_prefs().get(LOGIN_SKIPPED_PREF).and_then(|v| v.as_bool()).unwrap_or(false)
}

pub fn set_login_skipped(paths: &crate::paths::Paths, skipped: bool) {
    paths.update_prefs(|p| {
        p.insert(LOGIN_SKIPPED_PREF.to_owned(), serde_json::Value::Bool(skipped));
    });
}
