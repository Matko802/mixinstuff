
mod audio;
mod bootstrap;
mod demo;
mod discord;
mod downloads;
mod local_library;
mod lyrics;
mod model;
mod mpris;
mod net;
mod paths;
mod player;
mod presence;
mod queue;
#[cfg(all(target_os = "linux", any(target_arch = "aarch64", target_arch = "x86_64")))]
mod sampler;
mod scrobbler;
mod state;
mod tray;
mod ui;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::mpris::Mpris;
use crate::paths::Paths;
use crate::player::Player;
use crate::ui::window::MainWindow;

const APP_ID: &str = "io.github.matko802.Musishark";
const APP_NAME: &str = "Musishark";

pub struct App {
    pub player: Rc<Player>,
    pub net: net::NetHandle,
    pub downloads: Arc<downloads::Downloads>,
    pub download_events: RefCell<Option<async_channel::Receiver<downloads::Event>>>,
    pub paths: Paths,
    pub demo: Option<demo::Demo>,
    pub window: RefCell<Option<Rc<MainWindow>>>,
    pub mpris: RefCell<Option<Rc<Mpris>>>,
    pub scrobbler: Arc<scrobbler::Scrobbler>,
    pub lyrics: lyrics::Lyrics,
    pub discord: discord::Discord,
    pub local: Arc<local_library::LocalLibrary>,
}

fn main() -> glib::ExitCode {
    bootstrap::cap_malloc_arenas();
    bootstrap::raise_fd_limit();

    let paths = Paths::discover();
    bootstrap::init_logging(&paths);
    bootstrap::apply_gsk_renderer_pref(&paths);

    if let Err(err) = gio::resources_register_include!("musishark.gresource") {
        eprintln!("resource bundle failed to load: {err}");
        return glib::ExitCode::FAILURE;
    }
    glib::set_application_name(APP_NAME);
    if let Err(err) = gst::init() {
        eprintln!("GStreamer init failed: {err}");
        return glib::ExitCode::FAILURE;
    }

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .thread_name("musishark-net")
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("tokio runtime failed: {err}");
            return glib::ExitCode::FAILURE;
        }
    };

    let mut net = match net::NetHandle::new(runtime.handle().clone(), &paths) {
        Ok(net) => net,
        Err(err) => {
            eprintln!("network client failed: {err:#}");
            return glib::ExitCode::FAILURE;
        }
    };
    let demo = demo::from_env();
    if let Some(demo) = &demo {
        let inner = net.resolver().clone();
        net = net.with_resolver(Arc::new(net::stream::DemoResolver::new(inner, demo.uris.clone())));
    }

    let (audio, audio_events) = match audio::spawn() {
        Ok(pair) => pair,
        Err(err) => {
            eprintln!("audio engine failed: {err:#}");
            return glib::ExitCode::FAILURE;
        }
    };

    let (downloads, download_events) = downloads::Downloads::new(paths.clone(), net.clone());
    let publish = demo.is_none() || std::env::var_os("MUSISHARK_DEMO_PRESENCE").is_some();
    let scrobbler = scrobbler::Scrobbler::start(&paths, runtime.handle());
    let lyrics = lyrics::Lyrics::new(&paths, net.client().http().clone(), net.client().clone());
    scrobbler.set_muted(!publish);
    let local = local_library::LocalLibrary::open(&paths);
    let app_ctx = Rc::new(App {
        player: Player::new(net.clone(), downloads.clone(), local.clone(), audio, audio_events, &paths),
        local,
        net,
        downloads,
        download_events: RefCell::new(Some(download_events)),
        scrobbler,
        lyrics,
        discord: discord::Discord::new(publish && discord::enabled_pref(&paths.read_prefs())),
        paths,
        demo,
        window: RefCell::new(None),
        mpris: RefCell::new(None),
    });

    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(if app_ctx.demo.is_some() { gio::ApplicationFlags::NON_UNIQUE } else { gio::ApplicationFlags::FLAGS_NONE } | gio::ApplicationFlags::HANDLES_OPEN)
        .build();

    app.connect_startup(glib::clone!(
        #[strong]
        app_ctx,
        move |_| on_startup(&app_ctx)
    ));
    app.connect_activate(glib::clone!(
        #[strong]
        app_ctx,
        move |app| on_activate(app, &app_ctx)
    ));
    app.connect_open(glib::clone!(
        #[strong]
        app_ctx,
        move |app, files, _| {
            on_activate(app, &app_ctx);
            let Some(window) = app_ctx.window.borrow().clone() else { return };
            for file in files {
                let uri = file.uri();
                if !window.open_link(&uri) {
                    tracing::warn!(%uri, "not a YouTube link");
                }
            }
        }
    ));
    app.connect_shutdown(glib::clone!(
        #[strong]
        app_ctx,
        move |_| {
            tracing::info!("shutting down");
            if let Some(mpris) = app_ctx.mpris.borrow_mut().take() {
                mpris.shutdown();
            }
            tray::stop();
            app_ctx.scrobbler.stop();
            app_ctx.discord.stop();
            app_ctx.player.shutdown();
        }
    ));

    let code = app.run();
    runtime.shutdown_timeout(Duration::from_secs(2));
    code
}

fn on_startup(ctx: &Rc<App>) {
    if let Some(display) = gdk::Display::default() {
        let theme = gtk::IconTheme::for_display(&display);
        theme.add_resource_path("/io/github/matko802/musishark/icons/hicolor");
    }
    gtk::Window::set_default_icon_name(APP_ID);
    ui::load_css();
    ui::cover::init_disk_cache(&ctx.paths.cache_dir);
    let (downloaded, cached, extract) = (ctx.downloads.clone(), ctx.downloads.clone(), ctx.downloads.clone());
    ui::cover::set_track_art_lookup(move |id| downloaded.is_downloaded(id), move |id| cached.cached_cover(id), move |id| extract.extract_cover(id));

    ctx.player.start();
    ctx.mpris.replace(Some(Mpris::start(ctx)));
    tray::start(ctx);
    presence::wire(ctx);
    tracing::info!(auth = ?ctx.net.client().auth_state(), "core started");
}

fn on_activate(app: &adw::Application, ctx: &Rc<App>) {
    if let Some(window) = app.active_window() {
        window.present();
        return;
    }
    let window = MainWindow::new(app, ctx);
    ctx.window.replace(Some(window.clone()));
    if let Some(demo) = &ctx.demo {
        demo::install(demo, ctx, &window);
    }
    window.present();
    tracing::info!("main window presented");
    if ctx.demo.is_none() {
        let (window, ctx) = (Rc::downgrade(&window), ctx.clone());
        glib::timeout_add_local_once(Duration::from_millis(700), move || {
            if let Some(window) = window.upgrade() {
                window.welcome_or_release_notes(&ctx);
            }
        });
    }
}
