
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::App;
use crate::net::ytmusic::AuthState;
use crate::state::PlayerState;
use crate::model::Track;
use crate::ui::context::{NavRequest, UiContext};
use crate::ui::cover::load_texture;
use crate::ui::cover_view::DesktopCoverView;
use crate::ui::download_queue::DownloadQueue;
use crate::ui::upload_queue::UploadQueue;
use crate::ui::expanded_player::ExpandedPlayer;
use crate::ui::login::LoginDialog;
use crate::ui::pages::artist::ArtistPage;
use crate::ui::pages::all_moods::AllMoodsPage;
use crate::ui::pages::category::CategoryPage;
use crate::ui::pages::discography::DiscographyPage;
use crate::ui::pages::history::HistoryPage;
use crate::ui::pages::explore::ExplorePage;
use crate::ui::pages::home::HomePage;
use crate::ui::pages::library::LibraryPage;
use crate::ui::pages::playlist::{InitialData, PlaylistPage};
use crate::ui::player_bar::{PlayerBar, PlayerBarCallbacks};
use crate::ui::queue_panel::QueuePanel;

const APP_NAME: &str = "Musishark";
const NETWORK_SETTLE: Duration = Duration::from_millis(1500);
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(600);
const SEARCH_MIN_CHARS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppearancePref {
    BlurredBackground,
    DynamicAccent,
    TintedBackground,
}

pub struct MainWindow {
    window: adw::ApplicationWindow,
    toast_overlay: adw::ToastOverlay,
    split_view: adw::OverlaySplitView,
    bottom_sheet: adw::BottomSheet,
    main_stack: gtk::Stack,
    view_stack: adw::ViewStack,
    back_btn: gtk::Button,
    search_bar: gtk::SearchBar,
    search_entry: gtk::SearchEntry,
    root_content_view: adw::ToolbarView,
    player_bar_revealer: gtk::Revealer,
    player_bar_slot: gtk::Box,
    view_switcher_bar: adw::ViewSwitcherBar,
    sheet_bottom_bar: gtk::Box,
    title_bin: adw::Bin,
    switcher: adw::ViewSwitcher,
    title_widget: adw::WindowTitle,
    player_bar: Rc<PlayerBar>,
    #[allow(dead_code)]
    queue_panel: Rc<QueuePanel>,
    expanded_player: Rc<ExpandedPlayer>,
    cover_view: Rc<DesktopCoverView>,
    home: Rc<HomePage>,
    explore: Rc<ExplorePage>,
    library: Rc<LibraryPage>,
    ui: Rc<UiContext>,
    is_compact: Cell<bool>,
    sidebar_explicitly_opened: Cell<bool>,
    prev_transition: Cell<(gtk::StackTransitionType, u32)>,
    search_timer: RefCell<Option<glib::SourceId>>,
    own_channel: RefCell<Option<String>>,
    upload_progress: Rc<ProgressButton>,
    download_progress: Rc<ProgressButton>,
    download_queue: Rc<DownloadQueue>,
    upload_queue: Rc<UploadQueue>,
    lib_refresh: LibraryRefresh,
    appearance: Rc<crate::ui::appearance::Appearance>,
}

impl MainWindow {
    pub fn new(app: &adw::Application, ctx: &Rc<App>) -> Rc<Self> {
        let player = ctx.player.clone();
        let state = player.state().clone();
        let ui = UiContext::new(player.clone(), ctx.net.clone(), ctx.paths.clone(), ctx.downloads.clone(), ctx.lyrics.clone(), ctx.local.clone());
        if let Some(events) = ctx.download_events.borrow_mut().take() {
            ui.pump_downloads(events);
        }

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title(APP_NAME)
            .default_width(1040)
            .default_height(700)
            .build();
        let toast_overlay = adw::ToastOverlay::new();
        install_actions(&window, app, ctx);

        let view_stack = adw::ViewStack::builder().hhomogeneous(false).vhomogeneous(false).build();
        let switcher = adw::ViewSwitcher::builder()
            .stack(&view_stack)
            .policy(adw::ViewSwitcherPolicy::Wide)
            .build();
        let title_bin = adw::Bin::builder().child(&switcher).build();
        let title_widget = adw::WindowTitle::new(APP_NAME, "");

        let header_bar = adw::HeaderBar::new();
        let back_btn = gtk::Button::builder()
            .icon_name("go-previous-symbolic")
            .visible(false)
            .build();
        header_bar.pack_start(&back_btn);
        let search_btn = gtk::ToggleButton::builder()
            .icon_name("system-search-symbolic")
            .tooltip_text("Search")
            .build();
        header_bar.pack_start(&search_btn);
        header_bar.set_title_widget(Some(&title_bin));

        let upload_progress = ProgressButton::new("Upload Progress");
        let download_progress = ProgressButton::new("Download Progress");
        let (avatar_btn, avatar_profile) = build_avatar_menu();
        let primary_btn = build_primary_menu(&window);
        header_bar.pack_end(&primary_btn);
        header_bar.pack_end(&avatar_btn);
        header_bar.pack_end(&upload_progress.button);
        header_bar.pack_end(&download_progress.button);

        let home = HomePage::new(ui.clone());
        let library = LibraryPage::new(ui.clone());
        let explore = ExplorePage::new(ui.clone());
        for (name, title, icon, child) in [
            (
                "home",
                "Home",
                "user-home-symbolic",
                home.widget().clone().upcast::<gtk::Widget>(),
            ),
            (
                "library",
                "Library",
                "media-optical-symbolic",
                library.widget().clone().upcast(),
            ),
            (
                "search",
                "Explore",
                "compass2-symbolic",
                explore.widget().clone().upcast(),
            ),
        ] {
            let nav = create_tab_nav(&child, title);
            view_stack.add_titled_with_icon(&nav, Some(name), title, icon);
        }
        let lib_refresh = build_library_refresh();
        header_bar.pack_end(&lib_refresh.root);

        let search_entry = gtk::SearchEntry::builder()
            .placeholder_text("Search...")
            .hexpand(true)
            .build();
        let search_clamp = adw::Clamp::builder()
            .maximum_size(600)
            .child(&search_entry)
            .build();
        let search_bar = gtk::SearchBar::builder().child(&search_clamp).build();
        search_bar.connect_entry(&search_entry);
        search_bar
            .bind_property("search-mode-enabled", &search_btn, "active")
            .bidirectional()
            .sync_create()
            .build();

        let content_bin = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .child(&view_stack)
            .build();
        let main_stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::SlideLeftRight)
            .transition_duration(300)
            .build();
        main_stack.add_named(&content_bin, Some("browser"));
        let cover_view = DesktopCoverView::new(ui.clone());
        main_stack.add_named(cover_view.widget(), Some("cover"));

        let root_content_view = adw::ToolbarView::new();
        root_content_view.add_top_bar(&header_bar);
        root_content_view.add_top_bar(&search_bar);
        root_content_view.set_content(Some(&main_stack));
        let view_switcher_bar = adw::ViewSwitcherBar::builder()
            .stack(&view_stack)
            .reveal(false)
            .visible(false)
            .build();

        let split_view = adw::OverlaySplitView::builder()
            .min_sidebar_width(250.0)
            .max_sidebar_width(450.0)
            .show_sidebar(false)
            .enable_show_gesture(false)
            .enable_hide_gesture(false)
            .sidebar_position(sidebar_position(ctx))
            .build();
        let queue_panel = QueuePanel::new(ui.clone());
        queue_panel.widget().add_css_class("sidebar");
        split_view.set_sidebar(Some(queue_panel.widget()));
        split_view.set_content(Some(&root_content_view));

        let bottom_sheet = adw::BottomSheet::builder()
            .show_drag_handle(true)
            .open(false)
            .content(&split_view)
            .build();
        bottom_sheet
            .bind_property("bottom-bar-height", &split_view, "margin-bottom")
            .sync_create()
            .build();
        let sheet_bottom_bar = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["sheet-bottom-bar"])
            .build();
        let expanded_player = ExpandedPlayer::new(ui.clone());
        expanded_player.widget().add_css_class("player-drawer");
        expanded_player.widget().set_vexpand(true);

        let is_compact = Rc::new(Cell::new(false));
        let placeholder = PlayerBarCallbacks {
            on_artist_click: {
                let ui = ui.clone();
                Rc::new(move |id, name| ui.nav.go(NavRequest::Artist { id, name }))
            },
            on_album_click: {
                let ui = ui.clone();
                let overlay = toast_overlay.downgrade();
                Rc::new(move |id: Option<String>, name: String| match id {
                    Some(id) => ui.nav.go(NavRequest::Album {
                        id,
                        title: name,
                        thumb: None,
                    }),
                    None => {
                        let video_id = ui.player.state().video_id();
                        let api = ui.net.client().api();
                        let wanted = video_id.clone();
                        let handle = ui.net.spawn(async move { crate::net::playlists::get_watch_playlist(&*api, Some(&wanted), None, 1, false).await });
                        let (ui, overlay) = (ui.clone(), overlay.clone());
                        glib::spawn_future_local(async move {
                            let album = handle.await.ok().and_then(Result::ok).and_then(|w| w.tracks.into_iter().map(|t| t.track).find(|t| t.video_id.0 == video_id)).and_then(|t| t.album).filter(|a| a.id.is_some());
                            match album {
                                Some(album) => ui.nav.go(NavRequest::Album { id: album.id.unwrap_or_default(), title: album.name, thumb: None }),
                                None => {
                                    if let Some(o) = overlay.upgrade() {
                                        o.add_toast(adw::Toast::new("No album for this track"));
                                    }
                                }
                            }
                        });
                    }
                })
            },
            on_queue_click: Rc::new(|| {}),
            on_expand: Rc::new(|| {}),
        };
        let _ = &is_compact;
        let player_bar = PlayerBar::new(player.clone(), ctx.net.clone(), placeholder);
        let player_bar_revealer = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideUp)
            .transition_duration(200)
            .overflow(gtk::Overflow::Visible)
            .child(player_bar.widget())
            .build();
        let player_bar_slot = gtk::Box::builder().orientation(gtk::Orientation::Vertical).height_request(1).build();
        player_bar_slot.append(&player_bar_revealer);
        root_content_view.add_bottom_bar(&player_bar_slot);
        root_content_view.add_bottom_bar(&view_switcher_bar);

        toast_overlay.set_child(Some(&bottom_sheet));
        window.set_content(Some(&toast_overlay));

        let appearance = crate::ui::appearance::Appearance::new(&window, &ui);
        let ui_for_queue = ui.clone();
        let upload_items = upload_progress.items_box().clone();
        let this = Rc::new(Self {
            window,
            toast_overlay,
            split_view,
            bottom_sheet,
            main_stack,
            view_stack,
            back_btn,
            search_bar,
            search_entry,
            root_content_view,
            player_bar_revealer,
            player_bar_slot,
            view_switcher_bar,
            sheet_bottom_bar,
            title_bin,
            switcher,
            title_widget,
            player_bar,
            queue_panel,
            expanded_player,
            cover_view,
            home,
            explore,
            library,
            ui,
            is_compact: Cell::new(false),
            sidebar_explicitly_opened: Cell::new(false),
            prev_transition: Cell::new((gtk::StackTransitionType::SlideLeftRight, 300)),
            search_timer: RefCell::new(None),
            own_channel: RefCell::new(None),
            upload_progress,
            download_queue: DownloadQueue::new(ui_for_queue.clone(), download_progress.items_box().clone()),
            upload_queue: UploadQueue::new(ui_for_queue, upload_items),
            download_progress,
            lib_refresh,
            appearance,
        });

        {
            let weak = Rc::downgrade(&this);
            this.ui.set_download_sink(move |tracks, title, id| {
                if let Some(window) = weak.upgrade() {
                    window.download_tracks(tracks, &title, &id);
                }
            });
        }
        {
            let weak = Rc::downgrade(&this);
            this.upload_queue.set_on_progress(move |fraction| {
                if let Some(window) = weak.upgrade() {
                    window.upload_progress.set_fraction(fraction);
                    if fraction.is_none() {
                        window.add_toast("Uploads complete");
                    }
                }
            });
            let queue = this.upload_queue.clone();
            let root = this.window.clone();
            this.ui.nav.set_upload_picker(move || queue.pick_files(&root));
        }
        this.wire_downloads();
        this.wire_refresh();
        this.wire_player_bar();
        this.wire_navigation(&header_bar, &queue_header_of(&this.queue_panel));
        this.wire_search();
        this.wire_player_views();
        this.wire_breakpoints();
        this.wire_state(&state);
        avatar_profile.bind(ctx, &this.window);
        this.install_key_controller();
        install_close_handler(&this.window, ctx);
        this.install_network_state();
        this.sync_player_bar_visibility();
        this.check_auth_on_startup(ctx);
        this
    }

    pub fn window(&self) -> &adw::ApplicationWindow {
        &self.window
    }

    pub fn ui(&self) -> &Rc<UiContext> {
        &self.ui
    }

    pub fn library_page(&self) -> Rc<LibraryPage> {
        self.library.clone()
    }

    pub fn welcome_or_release_notes(self: &Rc<Self>, ctx: &Rc<App>) {
        if crate::ui::onboarding::pending(ctx) {
            tracing::info!("fresh install, opening the setup wizard");
            crate::ui::onboarding::present(self, ctx, None);
        } else if crate::ui::release_notes::due(ctx) {
            crate::ui::release_notes::present(self, ctx);
        }
    }

    pub fn present(&self) {
        self.window.present();
    }

    pub fn add_toast(&self, message: &str) {
        self.toast_overlay.add_toast(adw::Toast::new(message));
    }

    pub fn visualizers(&self) -> [Rc<crate::ui::widgets::visualizer::Visualizer>; 2] {
        [self.cover_view.visualizer().clone(), self.expanded_player.visualizer().clone()]
    }

    pub fn lyrics_views(&self) -> Vec<Rc<crate::ui::widgets::lyrics_view::LyricsView>> {
        crate::ui::widgets::lyrics_view::live_views()
    }

    pub fn set_sidebar_on_right(&self, on_right: bool) {
        self.split_view.set_sidebar_position(if on_right { gtk::PackType::End } else { gtk::PackType::Start });
    }

    pub fn appearance_pref_changed(self: &Rc<Self>, pref: AppearancePref) {
        self.appearance.pref_changed(pref);
    }

    pub fn refresh_providers(self: &Rc<Self>) {
        self.home.sync_provider();
        self.explore.sync_provider();
    }

    pub fn reload_provider_content(self: &Rc<Self>) {
        self.home.load_home_data(true);
        self.explore.reload_content();
    }

    pub fn force_offline_changed(self: &Rc<Self>) {
        self.ui.online.invalidate();
        self.ui.online.probe_now(None);
        self.library.apply_offline_state();
        self.library.load_library(false);
        self.explore.load_explore_data(true);
        self.home.refresh();
    }

    pub fn player_bar(&self) -> &Rc<PlayerBar> {
        &self.player_bar
    }

    pub fn show_queue(&self, show: bool) {
        self.split_view.set_show_sidebar(show);
        self.sidebar_explicitly_opened.set(show);
    }

    pub fn expand_player(&self) {
        self.on_expand_requested();
    }

    pub fn show_sheet_page(&self, name: &str) {
        self.expanded_player.show_page(name);
    }

    pub fn select_tab(&self, name: &str) {
        self.view_stack.set_visible_child_name(name);
    }

    pub fn open_category_for_demo(&self) -> bool {
        self.explore.open_first_category_for_demo()
    }

    pub fn open_all_moods_for_demo(&self) -> bool {
        self.explore.open_all_moods_for_demo()
    }

    pub fn history_menu_for_demo(&self) -> Vec<String> {
        match self.visible_pushed_page() {
            Some(PushedPage::History(page)) => page.menu_extras_for_demo(),
            _ => Vec::new(),
        }
    }

    pub fn activate_first_home_row(&self) -> bool {
        self.home.activate_first_playable()
    }

    pub fn pick_chart_country_for_demo(&self, code: &str) -> bool {
        self.explore.pick_chart_country_for_demo(code)
    }

    pub fn scroll_visible_page(&self, pixels: f64) -> bool {
        let Some(page) = self.active_nav().and_then(|nav| nav.visible_page()) else { return false };
        let Some(scroller) = first_scroller(page.upcast_ref::<gtk::Widget>()) else { return false };
        let adj = scroller.vadjustment();
        adj.set_value((adj.value() + pixels).clamp(0.0, (adj.upper() - adj.page_size()).max(0.0)));
        true
    }

    pub fn search(self: &Rc<Self>, query: &str) {
        self.navigate(NavRequest::Search {
            query: query.to_owned(),
        });
    }

    pub fn activate_first_result(&self) -> bool {
        self.explore.activate_first_playable()
    }

    pub fn open_provider_login(self: &Rc<Self>, provider: crate::net::provider::Provider, on_success: impl Fn() + 'static) {
        use crate::net::provider::Provider;
        let dialog = match provider {
            Provider::YouTube => LoginDialog::new(self.ui.clone(), &self.window),
            Provider::SoundCloud => LoginDialog::new_soundcloud(self.ui.clone(), &self.window),
            Provider::WatchShark => {
                let server = self.ui.net.watchshark().try_read_server();
                LoginDialog::new_watchshark(self.ui.clone(), &self.window, &server)
            }
        };
        dialog.set_on_success(on_success);
        dialog.present();
    }

    pub fn show_login(&self) {
        let dialog = LoginDialog::new(self.ui.clone(), &self.window);
        let library = self.library.clone();
        let overlay = self.toast_overlay.downgrade();
        dialog.set_on_success(move || {
            if let Some(o) = overlay.upgrade() {
                o.add_toast(adw::Toast::new("Signed in"));
            }
            library.load_library(false);
        });
        dialog.present();
        let holder = dialog.clone();
        dialog.connect_close(move || {
            let _ = &holder;
        });
    }

    fn install_network_state(self: &Rc<Self>) {
        let online = self.ui.online.clone();
        let net_online = Rc::new(Cell::new(online.is_online()));

        let weak = Rc::downgrade(self);
        let flag = net_online.clone();
        online.add_listener(move |now| {
            if flag.replace(now) == now {
                return;
            }
            if let Some(w) = weak.upgrade() {
                w.apply_network_state(now);
            }
        });

        let pending: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
        let probe = online.clone();
        gio::NetworkMonitor::default().connect_network_changed(move |_, _| {
            if let Some(id) = pending.borrow_mut().take() {
                id.remove();
            }
            let probe = probe.clone();
            let slot = pending.clone();
            let id = glib::timeout_add_local_once(NETWORK_SETTLE, move || {
                slot.borrow_mut().take();
                probe.invalidate();
                probe.probe_now(None);
            });
            pending.replace(Some(id));
        });

        let last_poll: Cell<Option<Instant>> = Cell::new(None);
        let weak = Rc::downgrade(self);
        glib::timeout_add_seconds_local(5, move || {
            if weak.upgrade().is_none() {
                return glib::ControlFlow::Break;
            }
            if net_online.get()
                && last_poll
                    .get()
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(30))
            {
                return glib::ControlFlow::Continue;
            }
            last_poll.set(Some(Instant::now()));
            online.probe_now(None);
            glib::ControlFlow::Continue
        });
    }

    fn apply_network_state(self: &Rc<Self>, online: bool) {
        if online {
            tracing::info!("back online, refreshing library");
            self.add_toast("Back online");
            crate::ui::cover::retry_failed();
            self.library.apply_offline_state();
            self.library.load_library(false);
            self.explore.load_explore_data(true);
            self.home.refresh();
            if !matches!(
                self.ui.net.client().auth_state(),
                AuthState::Authenticated(_)
            ) {
                let client = self.ui.net.client().clone();
                self.ui.net.spawn(async move {
                    let _ = client.validate().await;
                });
            }
        } else {
            tracing::info!("went offline");
            self.add_toast("Offline - downloaded songs still available");
            self.library.apply_offline_state();
            self.explore.load_explore_data(true);
            self.home.refresh();
        }
    }

    fn check_auth_on_startup(self: &Rc<Self>, ctx: &Rc<App>) {
        if crate::ui::onboarding::pending(ctx) {
            return;
        }
        if !crate::net::provider::ytm_enabled(&ctx.paths) {
            return;
        }
        let client = self.ui.net.client().clone();
        let weak = Rc::downgrade(self);
        let mut auth = client.subscribe_auth();
        glib::spawn_future_local(async move {
            loop {
                let state = auth.borrow_and_update().clone();
                let Some(w) = weak.upgrade() else { break };
                match state {
                    AuthState::Anonymous if crate::ui::login::login_skipped(&w.ui.paths) => {
                        tracing::info!("no session, sign-in skipped earlier, not asking");
                        break;
                    }
                    AuthState::Anonymous | AuthState::Invalid(_) => {
                        if w.ui.online.is_online() {
                            tracing::info!(?state, "no valid session, showing login dialog");
                            let weak = Rc::downgrade(&w);
                            glib::timeout_add_local_once(Duration::from_millis(500), move || {
                                if let Some(w) = weak.upgrade() {
                                    w.show_login();
                                }
                            });
                        } else {
                            w.add_toast("No internet - running in offline mode");
                        }
                        break;
                    }
                    AuthState::Authenticated(_) => break,
                    AuthState::Unverified => {}
                }
                if auth.changed().await.is_err() {
                    break;
                }
            }
        });
    }

    pub fn set_upload_progress(&self, fraction: Option<f64>) {
        self.upload_progress.set_fraction(fraction);
    }

    pub fn download_tracks(&self, tracks: Vec<Track>, album_title: &str, album_id: &str) {
        let queued = self.download_queue.start(tracks, album_title, album_id);
        if queued == 0 {
            self.add_toast("Already downloaded");
            return;
        }
        self.download_progress.set_fraction(Some(0.0));
    }

    fn wire_downloads(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.ui.on_download(move |event| {
            let Some(this) = weak.upgrade() else { return false };
            match event {
                crate::downloads::Event::Advanced { done, total, .. } => {
                    this.download_progress.set_fraction(Some(*done as f64 / (*total).max(1) as f64));
                }
                crate::downloads::Event::Idle { downloaded } => {
                    if *downloaded > 0 {
                        this.add_toast("Downloads complete");
                    }
                    this.download_queue.clear_later();
                    this.download_progress.set_fraction(None);
                    this.ui.nav.refresh_library();
                }
                _ => {}
            }
            true
        });
    }

    pub fn open_upload_picker(&self) {
        self.upload_queue.pick_files(&self.window);
    }

    pub fn go_back(self: &Rc<Self>) {
        self.on_back_clicked();
    }

    pub fn show_uploads_tab(&self) {
        self.library.show_uploads_for_demo();
    }

    pub fn card_menus(&self) {
        self.library.card_menus_for_demo();
    }

    pub fn new_playlist_dialog(&self) {
        self.library.new_playlist_for_demo();
    }

    pub fn slow_swipe(&self, covers: i32) {
        self.expanded_player.slow_swipe_for_demo(covers);
    }

    pub fn show_stream_info(&self) {
        match self.is_compact.get() {
            true => self.expanded_player.show_stream_info(),
            false => self.cover_view.show_stream_info(),
        }
    }

    pub fn show_download_popover(&self) {
        self.download_progress.popup();
    }

    pub fn set_download_progress(&self, fraction: Option<f64>) {
        self.download_progress.set_fraction(fraction);
    }

    pub fn upload_items(&self) -> &gtk::Box {
        self.upload_progress.items_box()
    }

    pub fn download_items(&self) -> &gtk::Box {
        self.download_progress.items_box()
    }


    fn wire_player_bar(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.player_bar.set_on_queue_click(move || {
            if let Some(w) = weak.upgrade() {
                w.toggle_queue();
            }
        });
        let weak = Rc::downgrade(self);
        self.player_bar.set_on_expand(move || {
            if let Some(w) = weak.upgrade() {
                w.on_expand_requested();
            }
        });
    }

    fn wire_player_views(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.cover_view.set_on_dismiss(move || {
            if let Some(w) = weak.upgrade() {
                w.dismiss_player();
            }
        });
        let weak = Rc::downgrade(self);
        self.cover_view.set_on_queue_click(move || {
            if let Some(w) = weak.upgrade() {
                w.toggle_queue();
            }
        });
        let weak = Rc::downgrade(self);
        self.expanded_player.set_on_dismiss(move || {
            if let Some(w) = weak.upgrade() {
                w.dismiss_player();
            }
        });
        let weak = Rc::downgrade(self);
        self.bottom_sheet.connect_open_notify(move |sheet| {
            if let Some(w) = weak.upgrade() {
                if sheet.is_open() {
                    w.expanded_player.show_player_page();
                } else {
                    w.player_bar.set_expanded(false);
                }
            }
        });
        let weak = Rc::downgrade(self);
        self.main_stack.connect_visible_child_name_notify(move |_| {
            if let Some(w) = weak.upgrade() {
                w.sync_player_bar_visibility();
                w.update_back_button();
            }
        });
    }

    pub fn toggle_queue(&self) {
        if self.is_compact.get() {
            return;
        }
        let show = !self.split_view.shows_sidebar();
        if show && self.ui.player.state().queue_length() == 0 {
            return;
        }
        self.split_view.set_show_sidebar(show);
        self.sidebar_explicitly_opened.set(show);
    }

    fn on_expand_requested(&self) {
        if !self.is_compact.get() {
            if self.main_stack.visible_child_name().as_deref() == Some("cover") {
                self.dismiss_player();
                return;
            }
            self.prev_transition.set((
                self.main_stack.transition_type(),
                self.main_stack.transition_duration(),
            ));
            self.main_stack.set_transition_duration(200);
            self.main_stack
                .set_transition_type(gtk::StackTransitionType::Crossfade);
            self.main_stack.set_visible_child_name("cover");
            self.back_btn.set_visible(true);
            self.player_bar.set_expanded(true);
            return;
        }
        if self.bottom_sheet.sheet().is_none() {
            self.bottom_sheet
                .set_sheet(Some(self.expanded_player.widget()));
        }
        self.player_bar.set_expanded(true);
        self.bottom_sheet.set_open(true);
    }

    fn dismiss_player(&self) {
        if self.is_compact.get() {
            self.bottom_sheet.set_open(false);
        } else {
            let was_cover = self.main_stack.visible_child_name().as_deref() == Some("cover");
            if was_cover {
                self.main_stack
                    .set_transition_type(gtk::StackTransitionType::Crossfade);
            }
            self.main_stack.set_visible_child_name("browser");
            if was_cover {
                let (kind, duration) = self.prev_transition.get();
                self.main_stack.set_transition_type(kind);
                self.main_stack.set_transition_duration(duration);
            }
            self.update_back_button();
        }
        self.player_bar.set_expanded(false);
    }

    fn dismiss_cover_if_open(&self) {
        if !self.is_compact.get()
            && self.main_stack.visible_child_name().as_deref() == Some("cover")
        {
            self.dismiss_player();
        }
    }

    fn sync_player_bar_visibility(&self) {
        let has_queue = self.ui.player.state().queue_length() > 0;
        let cover_shown = self.main_stack.visible_child_name().as_deref() == Some("cover");
        if has_queue && !self.is_compact.get() && cover_shown {
            self.player_bar_revealer.set_reveal_child(false);
            return;
        }
        self.player_bar_revealer.set_reveal_child(has_queue);
        self.bottom_sheet
            .set_can_open(has_queue && self.is_compact.get());
        if !has_queue {
            if self.split_view.shows_sidebar() {
                self.split_view.set_show_sidebar(false);
                self.sidebar_explicitly_opened.set(false);
            }
            if self.is_compact.get() && self.bottom_sheet.is_open() {
                self.bottom_sheet.set_open(false);
            }
            self.dismiss_cover_if_open();
        }
    }


    fn active_nav(&self) -> Option<adw::NavigationView> {
        self.view_stack
            .visible_child()
            .and_downcast::<adw::NavigationView>()
    }

    fn update_back_button(&self) {
        self.update_refresh_button();
        if !self.is_compact.get()
            && self.main_stack.visible_child_name().as_deref() == Some("cover")
        {
            self.back_btn.set_visible(true);
            return;
        }
        let deeper = self
            .active_nav()
            .and_then(|nav| nav.visible_page().and_then(|p| nav.previous_page(&p)))
            .is_some();
        self.back_btn.set_visible(deeper);
        if !deeper {
            self.title_widget.set_title(APP_NAME);
        }
    }

    fn on_back_clicked(&self) {
        if !self.is_compact.get()
            && self.main_stack.visible_child_name().as_deref() == Some("cover")
        {
            self.dismiss_player();
            return;
        }
        if let Some(nav) = self.active_nav() {
            nav.pop();
        }
    }

    fn open_playlist(self: &Rc<Self>, playlist_id: &str, initial: Option<InitialData>) {
        let page = PlaylistPage::new(self.ui.clone());
        let nav_page = adw::NavigationPage::builder()
            .child(page.widget())
            .title(format!("Playlist_{playlist_id}"))
            .build();
        let weak = Rc::downgrade(self);
        page.set_on_header_title(move |title| {
            if let Some(w) = weak.upgrade() {
                w.title_widget
                    .set_title(if title.is_empty() { APP_NAME } else { title });
            }
        });
        let page_c = page.clone();
        let id = playlist_id.to_owned();
        let initial = RefCell::new(initial);
        nav_page.connect_shown(move |_| {
            page_c.load_playlist(&id, initial.borrow_mut().take());
        });
        unsafe { nav_page.set_data("pushed", PushedPage::Playlist(page)) };
        self.push_page(nav_page);
    }

    pub fn open_playlist_for_demo(self: &Rc<Self>, id: &str) {
        self.open_playlist(id, None);
    }

    fn open_external_collection(self: &Rc<Self>, id: &str, title: &str) {
        let page = PlaylistPage::new(self.ui.clone());
        let nav_page = adw::NavigationPage::builder().child(page.widget()).title(format!("External_{id}")).build();
        page.prepare_virtual(id);
        self.push_page(nav_page);
        let net = self.ui.net.clone();
        let id = id.to_owned();
        let known_title = title.to_owned();
        let handle = self.ui.net.spawn(async move {
            use crate::net::soundcloud::ScId;
            use crate::net::watchshark::WsId;
            if let Some(sc_id) = crate::net::soundcloud::parse_sc_id(&id) {
                let sc = net.soundcloud().clone();
                return match sc_id {
                    ScId::Playlist(n) => sc.playlist(n).await,
                    ScId::User(n) => sc.user_tracks(n).await,
                    _ => Err(crate::net::ytmusic::NetError::Message("Not a SoundCloud set or artist".into())),
                };
            }
            if let Some(WsId::User(name)) = crate::net::watchshark::parse_ws_id(&id) {
                return net.watchshark().channel_tracks(&name).await;
            }
            Err(crate::net::ytmusic::NetError::Message("Not an external set or artist".into()))
        });
        glib::spawn_future_local(async move {
            let outcome = handle.await;
            match outcome {
                Ok(Ok((fetched_title, tracks))) => {
                    let title = if known_title.is_empty() { fetched_title } else { known_title };
                    let meta = if tracks.len() == 1 { "1 track".to_owned() } else { format!("{} tracks", tracks.len()) };
                    page.show_virtual(&title, tracks, &meta);
                }
                Ok(Err(err)) => {
                    tracing::warn!(%err, "soundcloud collection failed");
                    page.show_virtual(&known_title, Vec::new(), "nothing here");
                }
                Err(_) => {}
            }
        });
    }

    pub fn set_cover_on_visible_playlist(&self, image: std::path::PathBuf) -> bool {
        match self.visible_pushed_page() {
            Some(PushedPage::Playlist(p)) => {
                p.set_cover_for_demo(image);
                true
            }
            _ => false,
        }
    }

    pub fn sift_visible_playlist(&self, filter: Option<&str>, sort: Option<u32>) -> bool {
        match self.visible_pushed_page() {
            Some(PushedPage::Playlist(p)) => {
                p.sift_for_demo(filter, sort);
                true
            }
            _ => false,
        }
    }

    pub fn press_play_on_visible_playlist(&self) -> bool {
        match self.visible_pushed_page() {
            Some(PushedPage::Playlist(p)) => {
                p.press_play();
                true
            }
            _ => false,
        }
    }

    pub fn press_radio_on_visible_artist(&self) -> bool {
        match self.visible_pushed_page() {
            Some(PushedPage::Artist(p)) => {
                p.press_radio();
                true
            }
            _ => false,
        }
    }

    pub fn open_artist_for_demo(self: &Rc<Self>, channel_id: &str) {
        self.open_artist(channel_id, None);
    }

    pub fn open_discography_for_demo(self: &Rc<Self>, browse_id: &str) {
        self.open_discography("", "Songs", Some(browse_id), None, Vec::new());
    }

    pub fn open_artist(self: &Rc<Self>, channel_id: &str, initial_name: Option<&str>) {
        if channel_id.starts_with("FEmusic_library_privately_owned") {
            self.add_toast("Uploaded artists are not available yet");
            return;
        }
        let page = ArtistPage::new(self.ui.clone());
        let title = initial_name
            .filter(|n| !n.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Artist_{channel_id}"));
        let nav_page = adw::NavigationPage::builder()
            .child(page.widget())
            .title(title)
            .build();
        let weak = Rc::downgrade(self);
        page.set_on_header_title(move |title| {
            if let Some(w) = weak.upgrade() {
                w.title_widget
                    .set_title(if title.is_empty() { APP_NAME } else { title });
            }
        });
        unsafe { nav_page.set_data("pushed", PushedPage::Artist(page.clone())) };
        self.push_page(nav_page);
        page.load_artist(channel_id, initial_name);
    }

    fn resolve_artist_from_player(self: &Rc<Self>) {
        let vid = self.ui.player.state().video_id();
        if vid.is_empty() {
            return;
        }
        let api = self.ui.net.client().api();
        let handle = self
            .ui
            .net
            .spawn(async move { crate::net::artist::channel_of_video(&api, &vid).await });
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Some(w) = weak.upgrade() else { return };
            if let Ok(Ok(Some((channel, author)))) = handle.await {
                w.open_artist(&channel, Some(&author));
            }
        });
    }

    pub fn open_downloads(self: &Rc<Self>) {
        let page = PlaylistPage::new(self.ui.clone());
        page.prepare_virtual("DOWNLOADS");
        let nav_page = adw::NavigationPage::builder()
            .child(page.widget())
            .title("Downloaded Songs")
            .build();
        let weak = Rc::downgrade(self);
        page.set_on_header_title(move |title| {
            if let Some(w) = weak.upgrade() {
                w.title_widget
                    .set_title(if title.is_empty() { APP_NAME } else { title });
            }
        });
        let page_c = page.clone();
        let downloads = self.ui.downloads.clone();
        nav_page.connect_shown(move |_| {
            let page = page_c.clone();
            let downloads = downloads.clone();
            glib::idle_add_local_once(move || {
                let tracks: Vec<Track> = downloads.all().iter().map(|entry| entry.track()).collect();
                let meta = format!("{} {} available offline", tracks.len(), if tracks.len() == 1 { "song" } else { "songs" });
                page.show_virtual("Downloaded Songs", tracks, &meta);
            });
        });
        unsafe { nav_page.set_data("pushed", PushedPage::Playlist(page)) };
        self.push_page(nav_page);
    }

    pub fn open_uploads(self: &Rc<Self>) {
        let page = PlaylistPage::new(self.ui.clone());
        page.prepare_virtual("UPLOADS");
        let nav_page = adw::NavigationPage::builder().child(page.widget()).title("Uploaded Songs").build();
        let weak = Rc::downgrade(self);
        page.set_on_header_title(move |title| {
            if let Some(w) = weak.upgrade() {
                w.title_widget.set_title(if title.is_empty() { APP_NAME } else { title });
            }
        });
        let page_c = page.clone();
        let net = self.ui.net.clone();
        nav_page.connect_shown(move |_| {
            let page = page_c.clone();
            let api = net.client().api();
            let handle = net.spawn(async move { crate::net::playlists::get_upload_songs(&api).await });
            glib::spawn_future_local(async move {
                let tracks = match handle.await {
                    Ok(Ok(tracks)) => tracks,
                    Ok(Err(err)) => {
                        tracing::warn!(%err, "uploaded songs fetch failed");
                        Vec::new()
                    }
                    Err(_) => return,
                };
                let meta = format!("{} uploaded {}", tracks.len(), if tracks.len() == 1 { "song" } else { "songs" });
                page.show_virtual("Uploaded Songs", tracks, &meta);
            });
        });
        unsafe { nav_page.set_data("pushed", PushedPage::Playlist(page)) };
        self.push_page(nav_page);
    }

    pub fn open_upload_artist(self: &Rc<Self>, browse_id: &str, name: &str) {
        let page = PlaylistPage::new(self.ui.clone());
        page.prepare_virtual("UPLOADS");
        let nav_page = adw::NavigationPage::builder().child(page.widget()).title(name).build();
        let weak = Rc::downgrade(self);
        page.set_on_header_title(move |title| {
            if let Some(w) = weak.upgrade() {
                w.title_widget.set_title(if title.is_empty() { APP_NAME } else { title });
            }
        });
        let page_c = page.clone();
        let net = self.ui.net.clone();
        let (browse_id, name) = (browse_id.to_owned(), name.to_owned());
        nav_page.connect_shown(move |_| {
            let (page, name) = (page_c.clone(), name.clone());
            let api = net.client().api();
            let browse_id = browse_id.clone();
            let handle = net.spawn(async move { crate::net::uploads::artist_songs(&api, &browse_id, 200).await });
            glib::spawn_future_local(async move {
                let tracks = match handle.await {
                    Ok(Ok(tracks)) => tracks,
                    Ok(Err(err)) => {
                        tracing::warn!(%err, "uploaded artist fetch failed");
                        Vec::new()
                    }
                    Err(_) => return,
                };
                let meta = format!("{} uploaded {}", tracks.len(), if tracks.len() == 1 { "song" } else { "songs" });
                page.show_virtual(&name, tracks, &meta);
            });
        });
        unsafe { nav_page.set_data("pushed", PushedPage::Playlist(page)) };
        self.push_page(nav_page);
    }

    fn open_discography(
        self: &Rc<Self>,
        channel_id: &str,
        title: &str,
        browse_id: Option<&str>,
        params: Option<&str>,
        initial: Vec<crate::model::MediaItem>,
    ) {
        let page = DiscographyPage::new(self.ui.clone());
        let weak = Rc::downgrade(self);
        page.set_on_header_title(move |title| {
            if let Some(w) = weak.upgrade() {
                w.title_widget
                    .set_title(if title.is_empty() { APP_NAME } else { title });
            }
        });
        let nav_page = adw::NavigationPage::builder()
            .child(page.widget())
            .title(title)
            .build();
        unsafe { nav_page.set_data("pushed", PushedPage::Discography(page.clone())) };
        self.push_page(nav_page);
        page.load_discography(channel_id, title, browse_id, params, initial);
    }

    pub fn open_own_channel(self: &Rc<Self>) {
        if let Some(channel) = self.own_channel.borrow().clone() {
            self.open_artist(&channel, None);
            return;
        }
        let AuthState::Authenticated(account) = self.ui.net.client().auth_state() else { return };
        let Some(handle) = account.handle.clone().filter(|h| !h.is_empty()) else { return };
        let name = account.name.clone();
        let api = self.ui.net.client().api();
        let handle_c = handle.clone();
        let task = self.ui.net.spawn(async move { crate::net::artist::resolve_handle(&api, &handle_c).await });
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Some(w) = weak.upgrade() else { return };
            match task.await {
                Ok(Ok(Some(channel))) => {
                    w.own_channel.replace(Some(channel.clone()));
                    w.open_artist(&channel, Some(&name));
                }
                _ => w.add_toast("Couldn't open your channel"),
            }
        });
    }

    pub fn open_link(self: &Rc<Self>, text: &str) -> bool {
        if text.contains("soundcloud.com") {
            return self.open_soundcloud_link(text);
        }
        if text.contains("watchshark") {
            return self.open_watchshark_link(text);
        }
        if !crate::net::links::is_youtube_url(text) {
            return false;
        }
        let api = self.ui.net.client().api();
        let text_c = text.trim().to_owned();
        let handle = self.ui.net.spawn(async move { crate::net::links::resolve(&api, &text_c).await });
        let weak = Rc::downgrade(self);
        let text = text.trim().to_owned();
        glib::spawn_future_local(async move {
            let outcome = handle.await;
            let Some(w) = weak.upgrade() else { return };
            use crate::net::links::Link;
            match outcome {
                Ok(Ok(Some(Link::Song { video_id, playlist_id }))) => w.ui.player.play_link(video_id, playlist_id),
                Ok(Ok(Some(Link::Playlist(id)))) => w.navigate(NavRequest::Playlist { id, title: String::new(), thumb: None }),
                Ok(Ok(Some(Link::Album(id)))) => w.navigate(NavRequest::Album { id, title: String::new(), thumb: None }),
                Ok(Ok(Some(Link::Artist(id)))) => w.navigate(NavRequest::Artist { id: Some(id), name: String::new() }),
                Ok(Ok(None)) => w.add_toast("Musishark cannot open this link"),
                Ok(Err(err)) => {
                    tracing::warn!(%err, text, "link did not resolve");
                    w.add_toast("Could not open the link");
                }
                Err(_) => {}
            }
        });
        true
    }

    fn open_soundcloud_link(self: &Rc<Self>, text: &str) -> bool {
        let sc = self.ui.net.soundcloud().clone();
        let url = text.trim().to_owned();
        let handle = self.ui.net.spawn(async move { sc.resolve(&url).await });
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let outcome = handle.await;
            let Some(w) = weak.upgrade() else { return };
            use crate::net::soundcloud::Resolved;
            match outcome {
                Ok(Ok(Resolved::Track(track))) => w.ui.player.play_tracks(vec![track], 0, false, None, false),
                Ok(Ok(Resolved::Playlist(id, title))) => w.navigate(NavRequest::Playlist { id, title, thumb: None }),
                Ok(Ok(Resolved::Artist(id, name))) => w.navigate(NavRequest::Artist { id: Some(id), name }),
                Ok(Err(err)) => {
                    tracing::warn!(%err, "soundcloud link did not resolve");
                    w.add_toast("Could not open the link");
                }
                Err(_) => {}
            }
        });
        true
    }

    fn open_watchshark_link(self: &Rc<Self>, text: &str) -> bool {
        let text = text.trim().to_owned();
        let video_id = ["?id=", "&id=", "/watch/"].iter().filter_map(|m| text.split_once(m).map(|(_, after)| after)).next().map(|s| s.split(['&', '?', '/']).next().unwrap_or_default().to_owned()).filter(|s| !s.is_empty());
        if let Some(id) = video_id.and_then(|s| s.parse::<i64>().ok()) {
            let ws = self.ui.net.watchshark().clone();
            let handle = self.ui.net.spawn(async move { ws.video(id).await });
            let weak = Rc::downgrade(self);
            glib::spawn_future_local(async move {
                let Some(w) = weak.upgrade() else { return };
                match handle.await {
                    Ok(Ok(track)) => w.ui.player.play_tracks(vec![track], 0, false, None, false),
                    Ok(Err(err)) => {
                        tracing::warn!(%err, "watchshark link did not resolve");
                        w.add_toast("Could not open the link");
                    }
                    Err(_) => {}
                }
            });
            return true;
        }
        let user = ["?user=", "&user="].iter().filter_map(|m| text.split_once(m).map(|(_, after)| after)).next().map(|s| s.split(['&', '?', '/']).next().unwrap_or_default().to_owned()).filter(|s| !s.is_empty());
        if let Some(user) = user {
            self.navigate(NavRequest::Artist { id: Some(crate::net::watchshark::user_item_id(&user)), name: user });
            return true;
        }
        false
    }

    pub fn open_history(self: &Rc<Self>) {
        let signed_in = matches!(self.ui.net.client().auth_state(), AuthState::Authenticated(_));
        if signed_in && !self.ui.online.is_online() {
            self.add_toast("History requires an internet connection");
            return;
        }
        let page = HistoryPage::new(self.ui.clone());
        let weak = Rc::downgrade(self);
        page.set_on_header_title(move |title| {
            if let Some(w) = weak.upgrade() {
                w.title_widget
                    .set_title(if title.is_empty() { APP_NAME } else { title });
            }
        });
        let nav_page = adw::NavigationPage::builder()
            .child(page.widget())
            .title("Listening History")
            .build();
        let page_c = page.clone();
        nav_page.connect_shown(move |_| page_c.load());
        unsafe { nav_page.set_data("pushed", PushedPage::History(page)) };
        self.push_page(nav_page);
    }

    fn open_category(self: &Rc<Self>, params: &str, title: &str) {
        let page = CategoryPage::new(self.ui.clone());
        let weak = Rc::downgrade(self);
        page.set_on_header_title(move |title| {
            if let Some(w) = weak.upgrade() {
                w.title_widget
                    .set_title(if title.is_empty() { APP_NAME } else { title });
            }
        });
        let nav_page = adw::NavigationPage::builder()
            .child(page.widget())
            .title(title)
            .build();
        unsafe { nav_page.set_data("pushed", PushedPage::Category(page.clone())) };
        self.push_page(nav_page);
        page.load_category(params, title);
    }

    fn open_all_moods(self: &Rc<Self>, title: &str, items: Vec<crate::net::explore::Category>) {
        let page = AllMoodsPage::new(self.ui.clone(), title, items);
        let display = crate::ui::pages::all_moods::display_title(title);
        let nav_page = adw::NavigationPage::builder()
            .child(page.widget())
            .title(&display)
            .build();
        unsafe { nav_page.set_data("pushed", PushedPage::AllMoods(page)) };
        self.push_page(nav_page);
        self.title_widget.set_title(&display);
    }

    fn visible_pushed_page(&self) -> Option<PushedPage> {
        let nav_page = self.active_nav()?.visible_page()?;
        unsafe { nav_page.data::<PushedPage>("pushed") }.map(|p| unsafe { p.as_ref() }.clone())
    }

    fn active_filterable(&self) -> Option<PushedPage> {
        self.visible_pushed_page().filter(PushedPage::is_filterable)
    }

    fn active_filter(&self) -> Option<SearchFilter> {
        if self.view_stack.visible_child_name().as_deref() == Some("library") {
            let at_root = self
                .active_nav()
                .and_then(|nav| nav.visible_page().map(|page| nav.previous_page(&page).is_none()))
                .unwrap_or(false);
            if at_root {
                let library = self.library.clone();
                return Some(Rc::new(move |text: &str| library.filter_content(text)));
            }
        }
        let page = self.active_filterable()?;
        Some(Rc::new(move |text: &str| page.filter_content(text)))
    }

    fn refresh_target(&self) -> Option<RefreshTarget> {
        let on_library = self.view_stack.visible_child_name().as_deref() == Some("library");
        let nav = self.active_nav()?;
        let page = nav.visible_page()?;
        if on_library && nav.previous_page(&page).is_none() {
            return Some(RefreshTarget::Library);
        }
        match self.visible_pushed_page()? {
            PushedPage::Playlist(p) if p.is_refreshable() => Some(RefreshTarget::Playlist(p)),
            PushedPage::History(p) => Some(RefreshTarget::History(p)),
            _ => None,
        }
    }

    fn update_refresh_button(&self) {
        self.lib_refresh
            .root
            .set_visible(self.refresh_target().is_some());
    }

    fn wire_refresh(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.lib_refresh.button.connect_clicked(move |_| {
            let Some(w) = weak.upgrade() else { return };
            let Some(target) = w.refresh_target() else {
                return;
            };
            w.lib_refresh.button.set_visible(false);
            w.lib_refresh.spinner.set_visible(true);
            let done = {
                let w = w.clone();
                move || {
                    w.lib_refresh.spinner.set_visible(false);
                    w.lib_refresh.button.set_visible(true);
                }
            };
            match target {
                RefreshTarget::Library => w.library.refresh(done),
                RefreshTarget::Playlist(page) => {
                    page.refresh_in_place();
                    glib::timeout_add_local(Duration::from_millis(250), move || {
                        if page.content_spinner_visible() {
                            return glib::ControlFlow::Continue;
                        }
                        done();
                        glib::ControlFlow::Break
                    });
                }
                RefreshTarget::History(page) => {
                    page.refresh();
                    glib::timeout_add_local(Duration::from_millis(250), move || {
                        if page.is_loading() {
                            return glib::ControlFlow::Continue;
                        }
                        done();
                        glib::ControlFlow::Break
                    });
                }
            }
        });
        let weak = Rc::downgrade(self);
        self.view_stack.connect_visible_child_name_notify(move |_| {
            if let Some(w) = weak.upgrade() {
                w.update_refresh_button();
            }
        });
        self.update_refresh_button();
    }

    fn push_page(&self, page: adw::NavigationPage) {
        self.dismiss_cover_if_open();
        if self.search_bar.is_search_mode() {
            self.search_bar.set_search_mode(false);
        }
        let Some(nav) = self.active_nav() else { return };
        if nav
            .visible_page()
            .is_some_and(|p| p.title() == page.title())
        {
            return;
        }
        if let Some(child) = page.child() {
            crate::ui::style_only_while_shown(&page, &child);
        }
        nav.push(&page);
    }

    fn navigate(self: &Rc<Self>, request: NavRequest) {
        match request {
            NavRequest::Playlist { id, title, thumb } | NavRequest::Album { id, title, thumb } => {
                if crate::net::soundcloud::is_soundcloud_id(&id) || crate::net::watchshark::is_watchshark_id(&id) {
                    self.open_external_collection(&id, &title);
                    return;
                }
                self.open_playlist(
                    &id,
                    Some(InitialData {
                        title,
                        thumb,
                        author: None,
                    }),
                );
            }
            NavRequest::Discography {
                channel_id,
                title,
                browse_id,
                params,
                initial,
            } => self.open_discography(
                &channel_id,
                &title,
                browse_id.as_deref(),
                params.as_deref(),
                initial,
            ),
            NavRequest::Artist { id, name } => match id {
                Some(id) if crate::net::soundcloud::is_soundcloud_id(&id) || crate::net::watchshark::is_watchshark_id(&id) => self.open_external_collection(&id, &name),
                Some(id) => self.open_artist(&id, Some(&name)),
                None => self.resolve_artist_from_player(),
            },
            NavRequest::Category { title, params } => self.open_category(&params, &title),
            NavRequest::AllMoods { title, items } => self.open_all_moods(&title, items),
            NavRequest::Search { query } => {
                self.search_bar.set_search_mode(true);
                self.search_entry.set_text(&query);
                self.search_entry.set_position(-1);
            }
        }
    }

    fn wire_navigation(
        self: &Rc<Self>,
        header_bar: &adw::HeaderBar,
        queue_header: &adw::HeaderBar,
    ) {
        let weak = Rc::downgrade(self);
        self.ui.nav.set_sink(move |request| {
            if let Some(w) = weak.upgrade() {
                w.navigate(request);
            }
        });
        let library = self.library.clone();
        self.ui
            .nav
            .set_library_refresh(move || {
                library.refresh_local_items();
                library.load_library(false);
            });
        {
            let library = self.library.clone();
            self.ui.nav.set_library_card_refresh(move |playlist_id| library.invalidate_card(playlist_id));
        }

        let weak = Rc::downgrade(self);
        self.back_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.on_back_clicked();
            }
        });

        let mut child = self.view_stack.first_child();
        while let Some(widget) = child {
            if let Some(nav) = widget.downcast_ref::<adw::NavigationView>() {
                let weak = Rc::downgrade(self);
                nav.connect_visible_page_notify(move |_| {
                    if let Some(w) = weak.upgrade() {
                        w.update_back_button();
                    }
                });
            }
            child = widget.next_sibling();
        }

        let weak = Rc::downgrade(self);
        self.view_stack
            .connect_visible_child_name_notify(move |stack| {
                let Some(w) = weak.upgrade() else { return };
                w.dismiss_cover_if_open();
                w.update_back_button();
                if w.search_bar.is_search_mode()
                    && stack.visible_child_name().as_deref() != Some("search")
                {
                    w.search_bar.set_search_mode(false);
                }
            });

        for widget in [
            self.switcher.clone().upcast::<gtk::Widget>(),
            self.view_switcher_bar.clone().upcast(),
        ] {
            let click = gtk::GestureClick::new();
            let weak = Rc::downgrade(self);
            click.connect_pressed(move |_, _, _, _| {
                let Some(w) = weak.upgrade() else { return };
                let before = w.view_stack.visible_child_name();
                let weak = weak.clone();
                glib::timeout_add_local_once(Duration::from_millis(100), move || {
                    if let Some(w) = weak.upgrade() {
                        if w.view_stack.visible_child_name() == before {
                            if let Some(nav) = w.active_nav() {
                                nav.pop_to_tag("root");
                            }
                        }
                    }
                });
            });
            widget.add_controller(click);
        }

        {
            let player_bar = self.player_bar.clone();
            let cover_view = self.cover_view.clone();
            let header_bar = header_bar.clone();
            let queue_header = queue_header.clone();
            let sync = move |split_view: &adw::OverlaySplitView| {
                player_bar.set_queue_active(split_view.shows_sidebar());
                cover_view.set_queue_active(split_view.shows_sidebar());
                apply_window_controls_position(split_view, &header_bar, &queue_header);
            };
            sync(&self.split_view);
            let sync2 = sync.clone();
            let sync3 = sync.clone();
            self.split_view.connect_sidebar_position_notify(move |sv| sync3(sv));
            self.split_view
                .connect_show_sidebar_notify(move |sv| sync(sv));
            let weak = Rc::downgrade(self);
            self.split_view.connect_collapsed_notify(move |sv| {
                sync2(sv);
                if !sv.is_collapsed() {
                    if let Some(w) = weak.upgrade() {
                        let weak = Rc::downgrade(&w);
                        glib::idle_add_local_once(move || {
                            if let Some(w) = weak.upgrade() {
                                w.restore_sidebar_state();
                            }
                        });
                    }
                }
            });
        }
    }

    fn restore_sidebar_state(&self) {
        let has_queue = self.ui.player.state().queue_length() > 0;
        let show = self.sidebar_explicitly_opened.get() && has_queue && !self.is_compact.get();
        self.split_view.set_show_sidebar(show);
    }


    fn wire_search(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.search_bar
            .connect_search_mode_enabled_notify(move |bar| {
                let Some(w) = weak.upgrade() else { return };
                if bar.is_search_mode() {
                    w.search_entry.grab_focus();
                    if w.active_filter().is_some() {
                        return;
                    }
                    if w.view_stack.visible_child_name().as_deref() != Some("search") {
                        let stack = w.view_stack.clone();
                        glib::idle_add_local_once(move || stack.set_visible_child_name("search"));
                    }
                    if let Some(nav) = w
                        .view_stack
                        .child_by_name("search")
                        .and_downcast::<adw::NavigationView>()
                    {
                        nav.pop_to_tag("root");
                    }
                } else {
                    w.explore.show_explore();
                }
            });
        let weak = Rc::downgrade(self);
        self.search_entry.connect_search_changed(move |entry| {
            let Some(w) = weak.upgrade() else { return };
            if let Some(id) = w.search_timer.borrow_mut().take() {
                id.remove();
            }
            let text = entry.text().to_string();
            if crate::net::links::is_youtube_url(&text) && w.open_link(&text) {
                let entry = entry.clone();
                glib::idle_add_local_once(move || entry.set_text(""));
                w.search_bar.set_search_mode(false);
                return;
            }
            if let Some(filter) = w.active_filter() {
                filter(&text);
                return;
            }
            let weak = Rc::downgrade(&w);
            let id = glib::timeout_add_local_once(SEARCH_DEBOUNCE, move || {
                if let Some(w) = weak.upgrade() {
                    w.search_timer.borrow_mut().take();
                    w.run_search(&text);
                }
            });
            w.search_timer.replace(Some(id));
        });
        let weak = Rc::downgrade(self);
        self.search_entry.connect_stop_search(move |_| {
            if let Some(w) = weak.upgrade() {
                w.search_bar.set_search_mode(false);
                if let Some(filter) = w.active_filter() {
                    filter("");
                }
            }
        });
    }

    fn run_search(&self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            self.explore.show_explore();
            return;
        }
        if text.chars().count() < SEARCH_MIN_CHARS {
            return;
        }
        if self.view_stack.visible_child_name().as_deref() != Some("search") {
            self.view_stack.set_visible_child_name("search");
        }
        self.explore.show_results(text);
    }


    fn wire_breakpoints(self: &Rc<Self>) {
        let collapse = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            750.0,
            adw::LengthUnit::Px,
        ));
        collapse.add_setter(&self.split_view, "collapsed", Some(&true.to_value()));
        self.window.add_breakpoint(collapse);

        let mobile = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            500.0,
            adw::LengthUnit::Px,
        ));
        mobile.add_setter(&self.view_switcher_bar, "reveal", Some(&true.to_value()));
        mobile.add_setter(&self.view_switcher_bar, "visible", Some(&true.to_value()));
        let weak = Rc::downgrade(self);
        mobile.connect_apply(move |_| {
            if let Some(w) = weak.upgrade() {
                w.apply_compact(true);
            }
        });
        let weak = Rc::downgrade(self);
        mobile.connect_unapply(move |_| {
            if let Some(w) = weak.upgrade() {
                w.apply_compact(false);
            }
        });
        self.window.add_breakpoint(mobile);
    }

    fn apply_compact(self: &Rc<Self>, compact: bool) {
        if self.is_compact.get() == compact {
            return;
        }
        self.is_compact.set(compact);
        self.ui.set_compact(compact);
        if compact {
            if self.main_stack.visible_child_name().as_deref() == Some("cover") {
                let prev = self.main_stack.transition_type();
                self.main_stack
                    .set_transition_type(gtk::StackTransitionType::None);
                self.main_stack.set_visible_child_name("browser");
                self.main_stack.set_transition_type(prev);
                self.player_bar.set_expanded(false);
            }
            self.title_bin.set_child(Some(&self.title_widget));
            self.player_bar.set_compact(true);
            self.attach_bottom_bar_to_sheet();
            self.split_view.set_show_sidebar(false);
            if self.bottom_sheet.sheet().is_none() {
                self.bottom_sheet
                    .set_sheet(Some(self.expanded_player.widget()));
            }
            self.expanded_player.set_compact_mode(true);
        } else {
            self.title_bin.set_child(Some(&self.switcher));
            self.player_bar.set_compact(false);
            self.bottom_sheet.set_open(false);
            self.detach_bottom_bar_from_sheet();
            self.expanded_player.set_compact_mode(false);
            let weak = Rc::downgrade(self);
            glib::idle_add_local_once(move || {
                if let Some(w) = weak.upgrade() {
                    w.restore_sidebar_state();
                }
            });
        }
        self.home.set_compact(compact);
        self.explore.set_compact(compact);
        self.library.set_compact(compact);
        self.sync_player_bar_visibility();
    }

    fn attach_bottom_bar_to_sheet(&self) {
        if self.bottom_sheet.bottom_bar().is_some() {
            return;
        }
        self.player_bar_slot.remove(&self.player_bar_revealer);
        self.root_content_view.remove(&self.view_switcher_bar);
        self.sheet_bottom_bar.append(&self.player_bar_revealer);
        self.sheet_bottom_bar.append(&self.view_switcher_bar);
        self.bottom_sheet
            .set_bottom_bar(Some(&self.sheet_bottom_bar));
        self.bottom_sheet.set_reveal_bottom_bar(true);
        self.bottom_sheet
            .set_can_open(self.ui.player.state().queue_length() > 0);
        self.player_bar.set_sheet_bar(true);
    }

    fn detach_bottom_bar_from_sheet(&self) {
        if self.bottom_sheet.bottom_bar().is_none() {
            return;
        }
        self.bottom_sheet.set_bottom_bar(gtk::Widget::NONE);
        self.bottom_sheet.set_can_open(false);
        self.player_bar.set_sheet_bar(false);
        self.sheet_bottom_bar.remove(&self.player_bar_revealer);
        self.sheet_bottom_bar.remove(&self.view_switcher_bar);
        self.player_bar_slot.append(&self.player_bar_revealer);
        self.root_content_view.add_bottom_bar(&self.view_switcher_bar);
    }


    fn wire_state(self: &Rc<Self>, state: &PlayerState) {
        let weak = Rc::downgrade(self);
        state.connect_notify_local(Some("queue-length"), move |_, _| {
            if let Some(w) = weak.upgrade() {
                w.sync_player_bar_visibility();
            }
        });
        let overlay = self.toast_overlay.downgrade();
        state.connect_local("track-error", false, move |values| {
            let title = values
                .get(2)
                .and_then(|v| v.get::<String>().ok())
                .unwrap_or_default();
            let reason = values
                .get(3)
                .and_then(|v| v.get::<String>().ok())
                .unwrap_or_default();
            if let Some(overlay) = overlay.upgrade() {
                let text = if title.is_empty() {
                    format!("Couldn't play track: {reason}")
                } else {
                    format!("Couldn't play '{title}': {reason}")
                };
                overlay.add_toast(adw::Toast::new(&text));
            }
            None
        });
        let overlay = self.toast_overlay.downgrade();
        state.connect_local("notice", false, move |values| {
            let text = values
                .get(1)
                .and_then(|v| v.get::<String>().ok())
                .unwrap_or_default();
            if let Some(overlay) = overlay.upgrade() {
                overlay.add_toast(adw::Toast::new(&text));
            }
            None
        });
    }

    fn install_key_controller(self: &Rc<Self>) {
        let ctrl = gtk::EventControllerKey::new();
        ctrl.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        ctrl.connect_key_pressed(move |_, key, _, modifier| {
            let Some(w) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if key == gdk::Key::Escape {
                if w.search_bar.is_search_mode() {
                    w.search_bar.set_search_mode(false);
                    w.window.grab_focus();
                    return glib::Propagation::Stop;
                }
                if w.back_btn.is_visible() {
                    w.on_back_clicked();
                    return glib::Propagation::Stop;
                }
                return glib::Propagation::Proceed;
            }
            let in_text = GtkWindowExt::focus(&w.window).is_some_and(|f| {
                f.is::<gtk::Text>() || f.is::<gtk::TextView>() || f.is::<gtk::Entry>()
            });
            if in_text {
                return glib::Propagation::Proceed;
            }
            let player = &w.ui.player;
            match key {
                gdk::Key::space | gdk::Key::AudioPlay => {
                    player.toggle_play();
                    return glib::Propagation::Stop;
                }
                gdk::Key::AudioNext => {
                    player.next();
                    return glib::Propagation::Stop;
                }
                gdk::Key::AudioPrev => {
                    player.previous();
                    return glib::Propagation::Stop;
                }
                _ => {}
            }
            let Some(ch) = key
                .to_unicode()
                .filter(|c| !c.is_control() && !c.is_whitespace())
            else {
                return glib::Propagation::Proceed;
            };
            if modifier.intersects(
                gdk::ModifierType::CONTROL_MASK
                    | gdk::ModifierType::ALT_MASK
                    | gdk::ModifierType::META_MASK,
            ) {
                return glib::Propagation::Proceed;
            }
            w.search_bar.set_search_mode(true);
            w.search_entry.grab_focus();
            w.search_entry.set_text(&ch.to_string());
            w.search_entry.set_position(-1);
            glib::Propagation::Stop
        });
        self.window.add_controller(ctrl);
    }
}

fn queue_header_of(panel: &Rc<QueuePanel>) -> adw::HeaderBar {
    panel.header_bar.clone()
}

fn create_tab_nav(content: &gtk::Widget, title: &str) -> adw::NavigationView {
    let page = adw::NavigationPage::builder()
        .child(content)
        .title(title)
        .tag("root")
        .build();
    crate::ui::style_only_while_shown(&page, content);
    let nav = adw::NavigationView::new();
    nav.add(&page);
    nav
}


pub struct ProgressButton {
    button: gtk::Button,
    area: gtk::DrawingArea,
    fraction: Rc<Cell<f64>>,
    items_box: gtk::Box,
}

impl ProgressButton {
    fn new(tooltip: &str) -> Rc<Self> {
        let fraction: Rc<Cell<f64>> = Rc::new(Cell::new(0.0));
        let area = gtk::DrawingArea::builder()
            .width_request(16)
            .height_request(16)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .can_target(false)
            .build();
        {
            let fraction = fraction.clone();
            area.set_draw_func(move |area, cr, w, h| {
                let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
                let r = (w.min(h) as f64) / 2.0 - 1.0;
                let color = area.color();
                cr.set_source_rgba(
                    color.red() as f64,
                    color.green() as f64,
                    color.blue() as f64,
                    0.25,
                );
                cr.arc(cx, cy, r, 0.0, std::f64::consts::TAU);
                let _ = cr.fill();
                cr.set_source_rgba(
                    color.red() as f64,
                    color.green() as f64,
                    color.blue() as f64,
                    1.0,
                );
                cr.move_to(cx, cy);
                let start = -std::f64::consts::FRAC_PI_2;
                cr.arc(
                    cx,
                    cy,
                    r,
                    start,
                    start + std::f64::consts::TAU * fraction.get().clamp(0.0, 1.0),
                );
                cr.close_path();
                let _ = cr.fill();
            });
        }
        let button = gtk::Button::builder()
            .css_classes(["flat"])
            .tooltip_text(tooltip)
            .visible(false)
            .child(&area)
            .build();
        let items_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .build();
        let popover_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(8)
            .margin_end(8)
            .build();
        popover_box.append(&items_box);
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .max_content_height(400)
            .propagate_natural_height(true)
            .child(&popover_box)
            .build();
        let popover = gtk::Popover::builder()
            .width_request(300)
            .child(&scroll)
            .build();
        popover.set_parent(&button);
        button.connect_clicked(move |_| popover.popup());
        Rc::new(Self {
            button,
            area,
            fraction,
            items_box,
        })
    }

    pub fn popup(&self) {
        self.button.emit_clicked();
    }

    pub fn items_box(&self) -> &gtk::Box {
        &self.items_box
    }

    pub fn set_fraction(&self, fraction: Option<f64>) {
        match fraction {
            Some(f) => {
                self.fraction.set(f);
                self.button.set_visible(true);
                self.area.queue_draw();
            }
            None => self.button.set_visible(false),
        }
    }
}

pub struct LibraryRefresh {
    root: gtk::Box,
    button: gtk::Button,
    spinner: adw::Spinner,
}

fn build_library_refresh() -> LibraryRefresh {
    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .visible(false)
        .build();
    let button = gtk::Button::builder()
        .icon_name("view-refresh-symbolic")
        .css_classes(["flat"])
        .valign(gtk::Align::Center)
        .tooltip_text("Refresh")
        .build();
    let spinner = adw::Spinner::builder()
        .valign(gtk::Align::Center)
        .margin_start(4)
        .margin_end(4)
        .visible(false)
        .build();
    root.append(&button);
    root.append(&spinner);
    LibraryRefresh {
        root,
        button,
        spinner,
    }
}

fn first_scroller(widget: &gtk::Widget) -> Option<gtk::ScrolledWindow> {
    if let Some(scroller) = widget.downcast_ref::<gtk::ScrolledWindow>() {
        let adj = scroller.vadjustment();
        if adj.upper() > adj.page_size() {
            return Some(scroller.clone());
        }
    }
    let mut child = widget.first_child();
    while let Some(node) = child {
        child = node.next_sibling();
        if let Some(found) = first_scroller(&node) {
            return Some(found);
        }
    }
    None
}

const AVATAR_LARGE: u32 = 48;

type SearchFilter = Rc<dyn Fn(&str)>;

#[derive(Clone)]
pub enum PushedPage {
    Playlist(Rc<PlaylistPage>),
    Discography(Rc<DiscographyPage>),
    Artist(Rc<ArtistPage>),
    #[allow(dead_code)]
    Category(Rc<CategoryPage>),
    #[allow(dead_code)]
    History(Rc<HistoryPage>),
    AllMoods(Rc<AllMoodsPage>),
}

impl PushedPage {
    fn filter_content(&self, text: &str) {
        match self {
            PushedPage::Playlist(p) => p.filter_content(text),
            PushedPage::Discography(p) => p.filter_content(text),
            PushedPage::AllMoods(p) => p.filter_content(text),
            PushedPage::Artist(_) | PushedPage::Category(_) | PushedPage::History(_) => {}
        }
    }

    fn is_filterable(&self) -> bool {
        !matches!(self, PushedPage::Artist(_) | PushedPage::Category(_) | PushedPage::History(_))
    }
}

enum RefreshTarget {
    Library,
    Playlist(Rc<PlaylistPage>),
    History(Rc<HistoryPage>),
}

fn build_primary_menu(window: &adw::ApplicationWindow) -> gtk::MenuButton {
    let menu = gio::Menu::new();
    let theme_section = gio::Menu::new();
    let theme_item = gio::MenuItem::new(None, None);
    theme_item.set_attribute_value("custom", Some(&"theme-swatches".to_variant()));
    theme_section.append_item(&theme_item);
    menu.append_section(None, &theme_section);

    let app_section = gio::Menu::new();
    app_section.append(Some("Downloaded Songs"), Some("win.open-downloads"));
    app_section.append(Some("Keyboard Shortcuts"), Some("win.shortcuts"));
    app_section.append(Some("Preferences"), Some("win.preferences"));
    app_section.append(Some("What's New"), Some("win.whats-new"));
    app_section.append(Some("About Musishark"), Some("win.about"));
    app_section.append(Some("Quit"), Some("win.quit"));
    menu.append_section(None, &app_section);

    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.add_css_class("menu");
    popover.add_child(&build_theme_swatches(window), "theme-swatches");
    let btn = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .css_classes(["flat"])
        .tooltip_text("Main Menu")
        .build();
    btn.set_popover(Some(&popover));
    btn
}

fn build_theme_swatches(window: &adw::ApplicationWindow) -> gtk::Box {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .hexpand(true)
        .css_classes(["themeselector"])
        .build();
    let action = window
        .lookup_action("color-scheme")
        .and_downcast::<gio::SimpleAction>();
    let syncing = Rc::new(Cell::new(false));
    let mut buttons: Vec<(&'static str, gtk::CheckButton)> = Vec::new();
    let mut head: Option<gtk::CheckButton> = None;
    for (value, variant, tooltip) in [
        ("default", "follow", "Follow System Style"),
        ("light", "light", "Light Style"),
        ("dark", "dark", "Dark Style"),
    ] {
        let cb = gtk::CheckButton::builder()
            .css_classes(["theme-selector", variant])
            .tooltip_text(tooltip)
            .hexpand(true)
            .halign(gtk::Align::Center)
            .focus_on_click(false)
            .build();
        match &head {
            Some(h) => cb.set_group(Some(h)),
            None => head = Some(cb.clone()),
        }
        let action = action.clone();
        let syncing = syncing.clone();
        cb.connect_toggled(move |button| {
            if syncing.get() || !button.is_active() {
                return;
            }
            if let Some(action) = &action {
                if action
                    .state()
                    .and_then(|s| s.str().map(str::to_owned))
                    .as_deref()
                    != Some(value)
                {
                    action.change_state(&value.to_variant());
                }
            }
        });
        row.append(&cb);
        buttons.push((value, cb));
    }
    if let Some(action) = action {
        let buttons = Rc::new(buttons);
        let sync = {
            let buttons = buttons.clone();
            let syncing = syncing.clone();
            move |state: Option<glib::Variant>| {
                let current = state
                    .and_then(|s| s.str().map(str::to_owned))
                    .unwrap_or_else(|| "default".to_owned());
                syncing.set(true);
                if let Some((_, cb)) = buttons.iter().find(|(v, _)| *v == current) {
                    if !cb.is_active() {
                        cb.set_active(true);
                    }
                }
                syncing.set(false);
            }
        };
        sync(action.state());
        action.connect_state_notify(move |a| sync(a.state()));
    }
    row
}

struct AvatarProfile {
    small: adw::Avatar,
    large: adw::Avatar,
    name_label: gtk::Label,
    handle_label: gtk::Label,
    photo_url: RefCell<String>,
}

impl AvatarProfile {
    fn bind(self: Rc<Self>, ctx: &Rc<App>, window: &adw::ApplicationWindow) {
        let state = ctx.player.state().clone();
        let net = ctx.net.clone();
        let window = window.downgrade();
        let apply = move |state: &PlayerState, profile: &Rc<AvatarProfile>| {
            let authed = state.authenticated();
            let name = state.account_name();
            let handle = state.account_handle();
            let display = if authed && !name.is_empty() {
                name.clone()
            } else {
                "Not signed in".to_owned()
            };
            profile.name_label.set_label(&display);
            profile.small.set_text(Some(&name));
            profile.large.set_text(Some(&name));
            profile.small.set_show_initials(authed);
            profile.large.set_show_initials(authed);
            profile.handle_label.set_label(&handle);
            profile.handle_label.set_visible(!handle.is_empty());
            if let Some(win) = window.upgrade() {
                set_account_actions_authed(&win, authed, !handle.is_empty());
            }
            let photo = if authed {
                state.account_photo_url()
            } else {
                String::new()
            };
            if *profile.photo_url.borrow() == photo {
                return;
            }
            profile.photo_url.replace(photo.clone());
            if photo.is_empty() {
                profile.small.set_custom_image(gdk::Paintable::NONE);
                profile.large.set_custom_image(gdk::Paintable::NONE);
                return;
            }
            let url = photo.clone();
            let net = net.clone();
            let weak = Rc::downgrade(profile);
            glib::spawn_future_local(async move {
                let texture = load_texture(&net, &url, Some(AVATAR_LARGE)).await;
                let Some(profile) = weak.upgrade() else {
                    return;
                };
                if *profile.photo_url.borrow() != photo {
                    return;
                }
                profile.small.set_custom_image(texture.as_ref());
                profile.large.set_custom_image(texture.as_ref());
            });
        };
        apply(&state, &self);
        for prop in [
            "authenticated",
            "account-name",
            "account-handle",
            "account-photo-url",
        ] {
            let apply = apply.clone();
            let profile = self.clone();
            state.connect_notify_local(Some(prop), move |state, _| apply(state, &profile));
        }
    }
}

fn build_avatar_menu() -> (gtk::MenuButton, Rc<AvatarProfile>) {
    let small = adw::Avatar::new(28, None, false);
    let menu_btn = gtk::MenuButton::builder()
        .css_classes(["flat", "circular"])
        .tooltip_text("Account")
        .child(&small)
        .build();

    let menu = gio::Menu::new();
    let header_section = gio::Menu::new();
    let header_item = gio::MenuItem::new(None, None);
    header_item.set_attribute_value("custom", Some(&"profile-header".to_variant()));
    header_section.append_item(&header_item);
    menu.append_section(None, &header_section);

    let authed_section = gio::Menu::new();
    for (label, action) in [
        ("Your Channel", "win.open-channel"),
        ("Upload Songs", "win.open-upload"),
        ("Listening History", "win.open-history"),
    ] {
        let item = gio::MenuItem::new(Some(label), Some(action));
        item.set_attribute_value("hidden-when", Some(&"action-disabled".to_variant()));
        authed_section.append_item(&item);
    }
    menu.append_section(None, &authed_section);

    let auth_section = gio::Menu::new();
    for (label, action) in [("Sign In", "win.sign-in"), ("Log Out", "win.logout")] {
        let item = gio::MenuItem::new(Some(label), Some(action));
        item.set_attribute_value("hidden-when", Some(&"action-disabled".to_variant()));
        auth_section.append_item(&item);
    }
    menu.append_section(None, &auth_section);

    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.add_css_class("menu");
    menu_btn.set_popover(Some(&popover));

    let header = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .css_classes(["avatar-menu-header"])
        .margin_top(2)
        .margin_bottom(6)
        .margin_start(6)
        .margin_end(6)
        .build();
    let large = adw::Avatar::new(AVATAR_LARGE as i32, None, false);
    header.append(&large);
    let name_col = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .valign(gtk::Align::Center)
        .hexpand(true)
        .build();
    let name_label = gtk::Label::builder()
        .label("Not signed in")
        .css_classes(["heading"])
        .halign(gtk::Align::Start)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    let handle_label = gtk::Label::builder()
        .css_classes(["caption", "dim-label"])
        .halign(gtk::Align::Start)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .visible(false)
        .build();
    name_col.append(&name_label);
    name_col.append(&handle_label);
    header.append(&name_col);
    popover.add_child(&header, "profile-header");

    (
        menu_btn,
        Rc::new(AvatarProfile {
            small,
            large,
            name_label,
            handle_label,
            photo_url: RefCell::new(String::new()),
        }),
    )
}

fn set_account_actions_authed(window: &adw::ApplicationWindow, authed: bool, has_handle: bool) {
    for (name, enabled) in [
        ("open-channel", authed && has_handle),
        ("open-upload", authed),
        ("open-history", true),
        ("logout", authed),
        ("sign-in", !authed),
    ] {
        if let Some(action) = window
            .lookup_action(name)
            .and_downcast::<gio::SimpleAction>()
        {
            action.set_enabled(enabled);
        }
    }
}

fn sidebar_position(ctx: &Rc<App>) -> gtk::PackType {
    match ctx
        .paths
        .read_prefs()
        .get("sidebar_position")
        .and_then(|v| v.as_str())
    {
        Some("right") => gtk::PackType::End,
        _ => gtk::PackType::Start,
    }
}

fn apply_window_controls_position(
    split_view: &adw::OverlaySplitView,
    content_hdr: &adw::HeaderBar,
    sidebar_hdr: &adw::HeaderBar,
) {
    let is_right = split_view.sidebar_position() == gtk::PackType::End;
    let visible = split_view.shows_sidebar() && !split_view.is_collapsed();
    let sidebar_owns_trailing = is_right && visible;
    content_hdr.set_show_start_title_buttons(sidebar_owns_trailing);
    content_hdr.set_show_end_title_buttons(!sidebar_owns_trailing);
    sidebar_hdr.set_show_start_title_buttons(!is_right && visible);
    sidebar_hdr.set_show_end_title_buttons(sidebar_owns_trailing);
}


fn install_actions(
    window: &adw::ApplicationWindow,
    app: &adw::Application,
    ctx: &Rc<App>,
) {
    let add = |name: &str, enabled: bool, f: Box<dyn Fn()>| {
        let action = gio::SimpleAction::new(name, None);
        action.set_enabled(enabled);
        action.connect_activate(move |_, _| f());
        window.add_action(&action);
    };
    {
        let app = app.downgrade();
        let player = ctx.player.clone();
        add(
            "quit",
            true,
            Box::new(move || {
                player.stop();
                if let Some(app) = app.upgrade() {
                    app.quit();
                }
            }),
        );
    }
    {
        let win = window.downgrade();
        add(
            "about",
            true,
            Box::new(move || {
                let Some(win) = win.upgrade() else { return };
                let dialog = adw::AboutDialog::builder()
                    .application_icon(crate::ui::appearance::app_icon_name())
                    .application_name(APP_NAME)
                    .developer_name("Matko802")
                    .developers(["Matko802", "m-obeid (original creator)"])
                    .version(crate::ui::release_notes::current_version())
                    .issue_url("https://github.com/Matko802/musishark/issues")
                    .copyright("© 2026 Matko802")
                    .license_type(gtk::License::Gpl30)
                    .build();
                dialog.add_link(
                    "GitHub repo",
                    "https://github.com/Matko802/musishark",
                );
                dialog.add_link(
                    &format!(
                        "{} · original creator m-obeid",
                        crate::ui::release_notes::current_version()
                    ),
                    "https://github.com/m-obeid?tab=repositories",
                );
                dialog.present(Some(&win));
            }),
        );
    }
    {
        let ctx = ctx.clone();
        add(
            "whats-new",
            true,
            Box::new(move || {
                if let Some(win) = ctx.window.borrow().as_ref() {
                    crate::ui::release_notes::present(win, &ctx);
                }
            }),
        );
    }
    {
        let win = window.downgrade();
        add(
            "shortcuts",
            true,
            Box::new(move || {
                if let Some(win) = win.upgrade() {
                    build_shortcuts_dialog().present(Some(&win));
                }
            }),
        );
    }
    {
        let ctx = ctx.clone();
        add(
            "preferences",
            true,
            Box::new(move || {
                if let Some(win) = ctx.window.borrow().as_ref() {
                    let _ = crate::ui::preferences::present(win, &ctx);
                }
            }),
        );
    }
    {
        let ctx = ctx.clone();
        add(
            "open-channel",
            false,
            Box::new(move || {
                if let Some(win) = ctx.window.borrow().as_ref() {
                    win.open_own_channel();
                }
            }),
        );
    }
    {
        let ctx = ctx.clone();
        add(
            "open-upload",
            false,
            Box::new(move || {
                if let Some(win) = ctx.window.borrow().as_ref() {
                    win.ui.nav.pick_uploads();
                }
            }),
        );
    }
    {
        let ctx = ctx.clone();
        add(
            "open-history",
            false,
            Box::new(move || {
                if let Some(win) = ctx.window.borrow().as_ref() {
                    win.open_history();
                }
            }),
        );
    }
    {
        let ctx = ctx.clone();
        add(
            "open-downloads",
            true,
            Box::new(move || {
                if let Some(win) = ctx.window.borrow().as_ref() {
                    win.open_downloads();
                }
            }),
        );
    }
    {
        let ctx = ctx.clone();
        add(
            "open-uploads",
            true,
            Box::new(move || {
                if let Some(win) = ctx.window.borrow().as_ref() {
                    win.open_uploads();
                }
            }),
        );
    }
    {
        let ctx = ctx.clone();
        let action = gio::SimpleAction::new("open-upload-artist", Some(&<(String, String)>::static_variant_type()));
        action.connect_activate(move |_, parameter| {
            let Some((browse_id, name)) = parameter.and_then(|p| p.get::<(String, String)>()) else { return };
            if let Some(win) = ctx.window.borrow().as_ref() {
                win.open_upload_artist(&browse_id, &name);
            }
        });
        window.add_action(&action);
    }
    {
        let ctx = ctx.clone();
        add(
            "sign-in",
            true,
            Box::new(move || {
                if let Some(win) = ctx.window.borrow().as_ref() {
                    win.show_login();
                }
            }),
        );
    }
    {
        let net = ctx.net.clone();
        add(
            "logout",
            false,
            Box::new(move || {
                let client = net.client().clone();
                net.spawn(async move { client.logout().await });
            }),
        );
    }

    let current = ctx
        .paths
        .read_prefs()
        .get("color_scheme")
        .and_then(|v| v.as_str())
        .filter(|v| ["default", "light", "dark"].contains(v))
        .unwrap_or("default")
        .to_owned();
    apply_color_scheme(&current);
    let scheme = gio::SimpleAction::new_stateful(
        "color-scheme",
        Some(glib::VariantTy::STRING),
        &current.to_variant(),
    );
    {
        let paths = ctx.paths.clone();
        scheme.connect_change_state(move |action, value| {
            let chosen = value
                .and_then(|v| v.str())
                .filter(|v| ["default", "light", "dark"].contains(v))
                .unwrap_or("default")
                .to_owned();
            action.set_state(&chosen.to_variant());
            apply_color_scheme(&chosen);
            paths.update_prefs(|prefs| {
                prefs.insert(
                    "color_scheme".into(),
                    serde_json::Value::String(chosen.clone()),
                );
            });
        });
    }
    window.add_action(&scheme);

    app.set_accels_for_action("win.preferences", &["<Primary>comma"]);
    app.set_accels_for_action("win.shortcuts", &["<Primary>question", "<Primary>slash"]);
    app.set_accels_for_action("win.quit", &["<Primary>q"]);
}

fn apply_color_scheme(value: &str) {
    let scheme = match value {
        "light" => adw::ColorScheme::ForceLight,
        "dark" => adw::ColorScheme::ForceDark,
        _ => adw::ColorScheme::Default,
    };
    adw::StyleManager::default().set_color_scheme(scheme);
}

fn build_shortcuts_dialog() -> adw::Dialog {
    let page = adw::PreferencesPage::new();
    for (title, entries) in [
        (
            "General",
            vec![
                ("Preferences", "Ctrl + ,"),
                ("Keyboard Shortcuts", "Ctrl + ?"),
                ("Quit", "Ctrl + Q"),
                ("Go Back / Close Search", "Esc"),
            ],
        ),
        ("Playback", vec![("Play / Pause", "Space")]),
        ("Search", vec![("Start Typing to Search", "a")]),
    ] {
        let group = adw::PreferencesGroup::builder().title(title).build();
        for (name, accel) in entries {
            let row = adw::ActionRow::builder().title(name).build();
            row.add_suffix(
                &gtk::Label::builder()
                    .label(accel)
                    .css_classes(["dim-label", "numeric"])
                    .build(),
            );
            group.add(&row);
        }
        page.add(&group);
    }
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&page));
    adw::Dialog::builder()
        .title("Keyboard Shortcuts")
        .content_width(420)
        .content_height(480)
        .child(&view)
        .build()
}

fn install_close_handler(window: &adw::ApplicationWindow, ctx: &Rc<App>) {
    let state = ctx.player.state().clone();
    let paths = ctx.paths.clone();
    window.connect_close_request(move |win| {
        let background = paths
            .read_prefs()
            .get("background_play")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if background && state.queue_length() > 0 && state.current_index() >= 0 {
            win.set_visible(false);
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
}
