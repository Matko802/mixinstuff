
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;

use crate::model::{ItemKind, MediaItem};
use crate::net::explore::{self, Category, ChartArtist, Charts, ExploreData, Trend};
use crate::net::provider::{self, Provider};
use crate::net::search::{SearchResults, search_all};
use crate::ui::context::{NavRequest, UiContext};
use crate::ui::cover::CoverImage;
use crate::ui::pages::{activate_item, attach_item_menu, clear_children, loading_box};
use crate::ui::provider_bar::ProviderBar;
use crate::ui::toast;
use crate::ui::widgets::media_card::{CardOptions, MediaCard};
use crate::ui::widgets::scroll_box::HorizontalScrollBox;
use crate::ui::widgets::song_list::{SONG_THUMB_SIZE, search_subtitle, song_row_with_subtitle};
use crate::ui::widgets::song_row::SongRow;

const PILL_LIMIT: usize = 20;
const CHART_STRIP_SPACING: i32 = 12;
const SECTION_SPACING: i32 = 24;
const SECTION_SPACING_COMPACT: i32 = 16;
const NEW_RELEASE_LIMIT: usize = 10;
const VIDEO_LIMIT: usize = 5;
const TRENDING_LIMIT: usize = 5;
const CHART_ARTIST_LIMIT: usize = 20;

pub struct ExplorePage {
    root: gtk::Box,
    stack: gtk::Stack,
    explore_box: gtk::Box,
    results_stack: gtk::Stack,
    toggle_container: gtk::Box,
    ctx: Rc<UiContext>,
    provider_bar: Rc<ProviderBar>,
    landing: RefCell<String>,
    cards: RefCell<Vec<Rc<MediaCard>>>,
    scrollers: RefCell<Vec<Rc<HorizontalScrollBox>>>,
    toggle_group: RefCell<Option<adw::ToggleGroup>>,
    song_rows: RefCell<Vec<Rc<SongRow>>>,
    explore_rows: RefCell<Vec<Rc<SongRow>>>,
    last_results: RefCell<Vec<MediaItem>>,
    results_provider: RefCell<String>,
    current_query: RefCell<Option<String>>,
    inflight: RefCell<Option<tokio::task::AbortHandle>>,
    explore_inflight: RefCell<Option<tokio::task::AbortHandle>>,
    charts_country: RefCell<String>,
    data: RefCell<Option<ExploreData>>,
    country_menu: RefCell<Option<(gtk::DropDown, Vec<String>)>>,
    explore_loaded: Cell<bool>,
    explore_loading: Cell<bool>,
    explore_retry: Cell<u32>,
}

impl ExplorePage {
    pub fn new(ctx: Rc<UiContext>) -> Rc<Self> {
        let stack = gtk::Stack::builder().vexpand(true).build();

        let results_page = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
        let toggle_container = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).halign(gtk::Align::Center).margin_start(12).margin_end(12).build();
        let toggle_viewport = gtk::Viewport::builder().hscroll_policy(gtk::ScrollablePolicy::Natural).child(&toggle_container).build();
        let toggle_scroller = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::External).vscrollbar_policy(gtk::PolicyType::Never).child(&toggle_viewport).margin_top(16).margin_bottom(8).build();
        results_page.append(&toggle_scroller);
        let results_stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).vhomogeneous(false).hhomogeneous(false).valign(gtk::Align::Start).build();
        let results_clamp = adw::Clamp::builder().maximum_size(1024).tightening_threshold(600).child(&results_stack).build();
        let results_scrolled = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).child(&results_clamp).build();
        crate::ui::suppress_hover_while_scrolling(&results_scrolled);
        results_page.append(&results_scrolled);
        stack.add_named(&results_page, Some("results"));

        stack.add_named(&loading_box("Searching..."), Some("loading"));

        let explore_box = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(SECTION_SPACING).margin_top(24).margin_bottom(24).margin_start(12).margin_end(12).build();
        let explore_clamp = adw::Clamp::builder().maximum_size(1024).tightening_threshold(600).child(&explore_box).build();
        let explore_scrolled = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&explore_clamp).build();
        crate::ui::suppress_hover_while_scrolling(&explore_scrolled);
        stack.add_named(&explore_scrolled, Some("explore"));
        stack.set_visible_child_name("explore");

        let country = ctx.paths.read_prefs().get("charts_country").and_then(|v| v.as_str()).unwrap_or("ZZ").to_owned();
        let root = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
        let slot: Rc<RefCell<Option<std::rc::Weak<ExplorePage>>>> = Rc::new(RefCell::new(None));
        let provider_bar = ProviderBar::new(ctx.paths.clone(), {
            let slot = slot.clone();
            move || {
                let Some(page) = slot.borrow().as_ref().and_then(|w| w.upgrade()) else { return };
                let query = page.current_query.borrow().clone();
                match query {
                    Some(q) => {
                        page.current_query.replace(None);
                        page.show_results(&q);
                    }
                    None => page.load_explore_data(true),
                }
            }
        });
        root.append(provider_bar.widget());
        root.append(&stack);
        let page = Rc::new(Self {
            root,
            stack,
            explore_box,
            results_stack,
            toggle_container,
            provider_bar,
            landing: RefCell::new(provider::YOUTUBE.to_owned()),
            ctx,
            cards: RefCell::new(Vec::new()),
            scrollers: RefCell::new(Vec::new()),
            toggle_group: RefCell::new(None),
            song_rows: RefCell::new(Vec::new()),
            explore_rows: RefCell::new(Vec::new()),
            last_results: RefCell::new(Vec::new()),
            results_provider: RefCell::new(provider::YOUTUBE.to_owned()),
            current_query: RefCell::new(None),
            inflight: RefCell::new(None),
            explore_inflight: RefCell::new(None),
            charts_country: RefCell::new(country),
            data: RefCell::new(None),
            country_menu: RefCell::new(None),
            explore_loaded: Cell::new(false),
            explore_loading: Cell::new(false),
            explore_retry: Cell::new(0),
        });
        let weak = Rc::downgrade(&page);
        slot.replace(Some(weak.clone()));
        glib::idle_add_local_once(move || {
            if let Some(p) = weak.upgrade() {
                p.load_explore_data(false);
            }
        });
        let weak = Rc::downgrade(&page);
        page.stack.connect_map(move |_| {
            let Some(p) = weak.upgrade() else { return };
            if p.current_query.borrow().is_some() {
                return;
            }
            p.stack.set_visible_child_name("explore");
            p.load_explore_data(false);
        });
        page
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    pub fn sync_provider(self: &Rc<Self>) {
        self.provider_bar.refresh();
        let want = self.provider_bar.active().id().to_owned();
        let query = self.current_query.borrow().clone();
        if let Some(q) = query {
            if *self.results_provider.borrow() != want {
                self.current_query.replace(None);
                self.show_results(&q);
            }
            return;
        }
        if self.explore_loaded.get() && *self.landing.borrow() == want {
            return;
        }
        self.load_explore_data(true);
    }

    pub fn reload_content(self: &Rc<Self>) {
        let query = self.current_query.borrow().clone();
        match query {
            Some(q) => {
                self.current_query.replace(None);
                self.show_results(&q);
            }
            None => self.load_explore_data(true),
        }
    }

    pub fn set_compact(&self, compact: bool) {
        if compact {
            self.stack.add_css_class("compact");
        } else {
            self.stack.remove_css_class("compact");
        }
        let spacing = if compact { SECTION_SPACING_COMPACT } else { SECTION_SPACING };
        self.explore_box.set_spacing(spacing);
        let mut child = self.results_stack.first_child();
        while let Some(page) = child {
            child = page.next_sibling();
            if let Some(page) = page.downcast_ref::<gtk::Box>() {
                page.set_spacing(spacing);
            }
        }
        for card in self.cards.borrow().iter() {
            card.set_compact(compact);
        }
    }

    pub fn show_explore(&self) {
        self.current_query.replace(None);
        if let Some(handle) = self.inflight.borrow_mut().take() {
            handle.abort();
        }
        self.stack.set_visible_child_name("explore");
    }


    pub fn load_explore_data(self: &Rc<Self>, force: bool) {
        if (self.explore_loading.get() || self.explore_loaded.get()) && !force {
            return;
        }
        if let Some(handle) = self.explore_inflight.borrow_mut().take() {
            handle.abort();
        }
        if force {
            self.explore_loaded.set(false);
        }
        self.explore_loading.set(true);

        if !self.ctx.online.is_online() {
            self.update_explore_ui(None);
            return;
        }
        let want = self.provider_bar.active();
        if want == Provider::SoundCloud {
            self.clear_explore();
            self.explore_box.append(&loading_box("Loading…"));
            let sc = self.ctx.net.soundcloud().clone();
            let handle = self.ctx.net.spawn(async move { sc.charts().await });
            self.explore_inflight.replace(Some(handle.abort_handle()));
            let weak = Rc::downgrade(self);
            glib::spawn_future_local(async move {
                let outcome = handle.await;
                let Some(page) = weak.upgrade() else { return };
                if page.provider_bar.active() != want {
                    return;
                }
                let Ok(result) = outcome else { return };
                page.explore_inflight.borrow_mut().take();
                match result {
                    Ok(sections) if !sections.is_empty() => {
                        page.landing.replace(want.id().to_owned());
                        page.render_chart_sections(sections);
                    }
                    Ok(_) => page.update_explore_ui(None),
                    Err(err) => {
                        tracing::warn!(%err, "soundcloud charts failed");
                        page.update_explore_ui(None);
                    }
                }
            });
            return;
        }
        if want == Provider::WatchShark {
            self.clear_explore();
            self.explore_box.append(&loading_box("Loading…"));
            let ws = self.ctx.net.watchshark().clone();
            let handle = self.ctx.net.spawn(async move { ws.home_sections().await });
            self.explore_inflight.replace(Some(handle.abort_handle()));
            let weak = Rc::downgrade(self);
            glib::spawn_future_local(async move {
                let outcome = handle.await;
                let Some(page) = weak.upgrade() else { return };
                if page.provider_bar.active() != want {
                    return;
                }
                let Ok(result) = outcome else { return };
                page.explore_inflight.borrow_mut().take();
                match result {
                    Ok(sections) if !sections.is_empty() => {
                        page.landing.replace(want.id().to_owned());
                        page.render_chart_sections(sections);
                    }
                    Ok(_) => page.update_explore_ui(None),
                    Err(err) => {
                        tracing::warn!(%err, "watchshark home failed");
                        page.update_explore_ui(None);
                    }
                }
            });
            return;
        }
        if self.explore_box.first_child().is_none() {
            self.explore_box.append(&loading_box("Loading…"));
        }

        let api = self.ctx.net.client().api();
        let country = self.charts_country.borrow().clone();
        let handle = self.ctx.net.spawn(explore::load_explore(api, country));
        self.explore_inflight.replace(Some(handle.abort_handle()));
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let outcome = handle.await;
            let Some(page) = weak.upgrade() else { return };
            let Ok(result) = outcome else { return };
            page.explore_inflight.borrow_mut().take();
            match result {
                Ok(data) => {
                    page.landing.replace(want.id().to_owned());
                    page.update_explore_ui(Some(data));
                }
                Err(err) => {
                    tracing::warn!(%err, "explore fetch failed");
                    page.update_explore_ui(None);
                }
            }
        });
    }

    fn update_explore_ui(self: &Rc<Self>, data: Option<ExploreData>) {
        self.explore_loading.set(false);
        let Some(data) = data else {
            if !self.ctx.online.is_online() {
                self.clear_explore();
                self.explore_box.append(&status_box("network-offline-symbolic", "You're offline", Some("Explore requires an internet connection.\nYour downloaded songs are still available.")));
                return;
            }
            let attempt = self.explore_retry.get();
            if attempt < 3 {
                self.explore_retry.set(attempt + 1);
                let weak = Rc::downgrade(self);
                glib::timeout_add_local_once(Duration::from_millis(1500 * (attempt as u64 + 1)), move || {
                    if let Some(p) = weak.upgrade() {
                        if !p.explore_loaded.get() {
                            p.load_explore_data(true);
                        }
                    }
                });
            } else {
                self.show_explore_retry_placeholder();
            }
            return;
        };
        self.clear_explore();
        self.explore_loaded.set(true);
        self.explore_retry.set(0);
        self.populate_explore(&data);
        self.data.replace(Some(data));
    }

    fn show_explore_retry_placeholder(self: &Rc<Self>) {
        self.clear_explore();
        let status = status_box("dialog-warning-symbolic", "Couldn't load Explore", None);
        let retry = gtk::Button::builder().label("Retry").css_classes(["pill", "suggested-action"]).halign(gtk::Align::Center).build();
        let weak = Rc::downgrade(self);
        retry.connect_clicked(move |_| {
            if let Some(p) = weak.upgrade() {
                p.explore_retry.set(0);
                p.load_explore_data(true);
            }
        });
        status.append(&retry);
        self.explore_box.append(&status);
    }

    fn clear_explore(&self) {
        clear_children(&self.explore_box);
        self.cards.borrow_mut().clear();
        self.scrollers.borrow_mut().clear();
        self.explore_rows.borrow_mut().clear();
        self.country_menu.replace(None);
    }

    fn populate_explore(self: &Rc<Self>, data: &ExploreData) {
        let with_podcasts = |pills: &[Category]| {
            let podcasts = Category { title: "Podcasts".to_owned(), params: crate::net::explore::PODCASTS_KEY.to_owned() };
            std::iter::once(podcasts).chain(pills.iter().cloned()).collect::<Vec<_>>()
        };
        if !data.for_you.is_empty() || !data.moods.is_empty() || !data.genres.is_empty() {
            self.add_pill_section("For You", &data.for_you);
            self.add_pill_section("Moods & Moments", &data.moods);
            self.add_pill_section("Genres", &with_podcasts(&data.genres));
        } else {
            self.add_pill_section("Moods & Genres", &with_podcasts(&data.feed.moods_and_genres));
        }

        self.add_row_section("New Albums & Singles", capped(&data.feed.new_releases, NEW_RELEASE_LIMIT));
        self.add_row_section("New Music Videos", capped(&data.feed.new_videos, VIDEO_LIMIT));
        self.add_row_section("Trending", capped(&data.feed.trending, TRENDING_LIMIT));
        if let Some(charts) = &data.charts {
            self.add_charts(charts);
        }
        self.set_compact(self.ctx.compact.get());
    }

    fn add_pill_section(self: &Rc<Self>, title: &str, categories: &[Category]) {
        if categories.is_empty() {
            return;
        }
        let section = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).build();
        section.append(&heading(title));
        let scroll_box = HorizontalScrollBox::new();
        scroll_box.widget().set_margin_bottom(12);
        let strip = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(12).build();
        for category in categories.iter().take(PILL_LIMIT) {
            let button = gtk::Button::builder().label(&category.title).css_classes(["pill"]).build();
            let ctx = self.ctx.clone();
            let category = category.clone();
            button.connect_clicked(move |_| ctx.nav.go(NavRequest::Category { title: category.title.clone(), params: category.params.clone() }));
            strip.append(&button);
        }
        if categories.len() > PILL_LIMIT {
            let view_all = gtk::Button::builder().label("View All").css_classes(["pill", "flat"]).build();
            let ctx = self.ctx.clone();
            let (title, items) = (title.to_owned(), categories.to_vec());
            view_all.connect_clicked(move |_| ctx.nav.go(NavRequest::AllMoods { title: title.clone(), items: items.clone() }));
            strip.append(&view_all);
        }
        scroll_box.set_content(&strip);
        section.append(scroll_box.widget());
        self.explore_box.append(&section);
        self.scrollers.borrow_mut().push(scroll_box);
    }

    fn add_row_section(self: &Rc<Self>, title: &str, items: &[MediaItem]) {
        if items.is_empty() {
            return;
        }
        self.add_song_list(&self.explore_box, title, items, &self.explore_rows);
    }

    pub fn pick_chart_country_for_demo(&self, code: &str) -> bool {
        let menu = self.country_menu.borrow();
        let Some((dropdown, codes)) = menu.as_ref() else { return false };
        let Some(index) = codes.iter().position(|c| c == code) else { return false };
        dropdown.set_selected(index as u32);
        true
    }

    pub fn open_first_category_for_demo(&self) -> bool {
        let data = self.data.borrow();
        let Some(category) = data.as_ref().and_then(|d| d.genres.first().or_else(|| d.moods.first())) else { return false };
        self.ctx.nav.go(NavRequest::Category { title: category.title.clone(), params: category.params.clone() });
        true
    }

    pub fn open_all_moods_for_demo(&self) -> bool {
        let data = self.data.borrow();
        let Some(items) = data.as_ref().map(|d| if d.genres.is_empty() { d.moods.clone() } else { d.genres.clone() }) else { return false };
        if items.is_empty() {
            return false;
        }
        self.ctx.nav.go(NavRequest::AllMoods { title: "Genres".into(), items });
        true
    }


    fn add_charts(self: &Rc<Self>, charts: &Charts) {
        let header = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(8).build();
        header.append(&gtk::Label::builder().label("Charts").css_classes(["title-3"]).halign(gtk::Align::Start).hexpand(true).build());
        if let Some(dropdown) = self.country_dropdown(&charts.countries) {
            header.append(&dropdown);
        }
        self.explore_box.append(&header);

        self.add_chart_playlists("Trending", &charts.videos);
        self.add_chart_playlists("Daily", &charts.daily);
        self.add_chart_playlists("Weekly", &charts.weekly);
        self.add_chart_playlists("Genre Charts", &charts.genres);
        self.add_chart_artists("Top Artists", &charts.artists);
    }

    fn country_dropdown(self: &Rc<Self>, codes: &[String]) -> Option<gtk::DropDown> {
        if codes.is_empty() {
            return None;
        }
        let options = explore::country_options(codes);
        let names: Vec<&str> = options.iter().map(|(_, name)| name.as_str()).collect();
        let dropdown = gtk::DropDown::from_strings(&names);
        dropdown.add_css_class("flat");
        if let Some(index) = options.iter().position(|(code, _)| *code == *self.charts_country.borrow()) {
            dropdown.set_selected(index as u32);
        }
        let codes: Vec<String> = options.into_iter().map(|(code, _)| code).collect();
        self.country_menu.replace(Some((dropdown.clone(), codes.clone())));
        let weak = Rc::downgrade(self);
        dropdown.connect_selected_notify(move |dd| {
            let Some(page) = weak.upgrade() else { return };
            let Some(code) = codes.get(dd.selected() as usize) else { return };
            if *code == *page.charts_country.borrow() {
                return;
            }
            page.charts_country.replace(code.clone());
            let code = code.clone();
            page.ctx.paths.update_prefs(|prefs| {
                prefs.insert("charts_country".into(), serde_json::Value::String(code));
            });
            page.load_explore_data(true);
        });
        Some(dropdown)
    }

    fn add_chart_playlists(self: &Rc<Self>, title: &str, items: &[MediaItem]) {
        if items.is_empty() {
            return;
        }
        let section = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).build();
        section.append(&heading(title));
        let scroll_box = HorizontalScrollBox::new();
        scroll_box.widget().set_margin_bottom(8);
        let strip = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(CHART_STRIP_SPACING).build();
        for item in items {
            let card = MediaCard::new(&self.ctx, item.clone(), CardOptions { title_lines: 2, ..CardOptions::default() });
            let ctx = self.ctx.clone();
            card.connect_clicked(move |item| activate_item(&ctx, item, &[]));
            attach_item_menu(&self.ctx, card.widget(), item.clone());
            strip.append(card.widget());
            self.cards.borrow_mut().push(card);
        }
        scroll_box.set_content(&strip);
        section.append(scroll_box.widget());
        self.explore_box.append(&section);
        self.scrollers.borrow_mut().push(scroll_box);
    }

    fn add_chart_artists(self: &Rc<Self>, title: &str, artists: &[ChartArtist]) {
        if artists.is_empty() {
            return;
        }
        let section = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).build();
        section.append(&heading(title));
        let list = gtk::ListBox::builder().css_classes(["boxed-list", "songs-list"]).selection_mode(gtk::SelectionMode::None).build();
        for artist in artists.iter().take(CHART_ARTIST_LIMIT) {
            let row = gtk::ListBoxRow::builder().activatable(true).build();
            let inner = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(12).css_classes(["song-row"]).build();
            row.set_child(Some(&inner));

            inner.append(&gtk::Label::builder().label(artist.rank.clone().unwrap_or_default()).width_chars(3).css_classes(["heading"]).valign(gtk::Align::Center).build());
            let (icon, css) = match artist.trend {
                Trend::Up => ("go-up-symbolic", "success"),
                Trend::Down => ("go-down-symbolic", "error"),
                Trend::Neutral => ("go-next-symbolic", "dim-label"),
            };
            inner.append(&gtk::Image::builder().icon_name(icon).pixel_size(12).valign(gtk::Align::Center).css_classes([css]).build());

            let cover = CoverImage::in_context(&self.ctx, SONG_THUMB_SIZE);
            cover.widget().add_css_class("song-img");
            match &artist.item.thumb {
                Some(url) => cover.load(url),
                None => cover.set_placeholder("avatar-default-symbolic"),
            }
            inner.append(cover.widget());
            unsafe { row.set_data("cover", cover) };

            let text = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).valign(gtk::Align::Center).hexpand(true).build();
            text.append(&gtk::Label::builder().label(&artist.item.title).halign(gtk::Align::Start).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).lines(1).width_chars(1).build());
            if let Some(subscribers) = artist.item.subscribers.as_deref().filter(|s| !s.is_empty()) {
                text.append(&gtk::Label::builder().label(subscribers).halign(gtk::Align::Start).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).lines(1).width_chars(1).css_classes(["dim-label", "caption"]).build());
            }
            inner.append(&text);

            attach_item_menu(&self.ctx, &row, artist.item.clone());
            list.append(&row);
        }
        let ctx = self.ctx.clone();
        let items: Vec<MediaItem> = artists.iter().take(CHART_ARTIST_LIMIT).map(|a| a.item.clone()).collect();
        list.connect_row_activated(move |_, row| {
            if let Some(item) = items.get(row.index().max(0) as usize) {
                ctx.nav.go(NavRequest::Artist { id: Some(item.id.clone()), name: item.title.clone() });
            }
        });
        section.append(&list);
        self.explore_box.append(&section);
    }


    pub fn show_results(self: &Rc<Self>, query: &str) {
        let query = query.trim().to_owned();
        if query.is_empty() {
            self.show_explore();
            return;
        }
        if self.current_query.borrow().as_deref() == Some(query.as_str()) {
            return;
        }
        self.current_query.replace(Some(query.clone()));
        if let Some(handle) = self.inflight.borrow_mut().take() {
            handle.abort();
        }
        if !self.ctx.online.is_online() {
            let items = local_results(&self.ctx.downloads.all(), &query);
            self.render_results(&query, SearchResults { top_result: None, items });
            return;
        }
        self.stack.set_visible_child_name("loading");

        let want = self.provider_bar.active();
        if want == Provider::SoundCloud {
            let sc = self.ctx.net.soundcloud().clone();
            let lookup = query.clone();
            let handle = self.ctx.net.spawn(async move { sc.search(&lookup).await });
            self.inflight.replace(Some(handle.abort_handle()));
            let weak = Rc::downgrade(self);
            glib::spawn_future_local(async move {
                let outcome = handle.await;
                let Some(page) = weak.upgrade() else { return };
                if page.current_query.borrow().as_deref() != Some(query.as_str()) || page.provider_bar.active() != want {
                    return;
                }
                page.inflight.borrow_mut().take();
                match outcome {
                    Ok(Ok(results)) => page.render_results(&query, results),
                    Ok(Err(err)) => {
                        tracing::warn!(%err, %query, "soundcloud search failed");
                        toast(&page.stack, &format!("Search failed: {err}"));
                        page.render_results(&query, SearchResults::default());
                    }
                    Err(_) => {}
                }
            });
            return;
        }
        if want == Provider::WatchShark {
            let ws = self.ctx.net.watchshark().clone();
            let lookup = query.clone();
            let handle = self.ctx.net.spawn(async move { ws.search(&lookup).await });
            self.inflight.replace(Some(handle.abort_handle()));
            let weak = Rc::downgrade(self);
            glib::spawn_future_local(async move {
                let outcome = handle.await;
                let Some(page) = weak.upgrade() else { return };
                if page.current_query.borrow().as_deref() != Some(query.as_str()) || page.provider_bar.active() != want {
                    return;
                }
                page.inflight.borrow_mut().take();
                match outcome {
                    Ok(Ok(results)) => page.render_results(&query, results),
                    Ok(Err(err)) => {
                        tracing::warn!(%err, %query, "watchshark search failed");
                        toast(&page.stack, &format!("Search failed: {err}"));
                        page.render_results(&query, SearchResults::default());
                    }
                    Err(_) => {}
                }
            });
            return;
        }
        let client = self.ctx.net.client().api();
        let handle = self.ctx.net.spawn(search_all(client, query.clone()));
        self.inflight.replace(Some(handle.abort_handle()));
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let outcome = handle.await;
            let Some(page) = weak.upgrade() else { return };
            if page.current_query.borrow().as_deref() != Some(query.as_str()) || page.provider_bar.active() != want {
                return;
            }
            page.inflight.borrow_mut().take();
            match outcome {
                Ok(Ok(results)) => page.render_results(&query, results),
                Ok(Err(err)) => {
                    tracing::warn!(%err, %query, "search failed");
                    toast(&page.stack, &format!("Search failed: {err}"));
                    page.render_results(&query, SearchResults::default());
                }
                Err(_) => {}
            }
        });
    }

    fn render_results(self: &Rc<Self>, query: &str, results: SearchResults) {
        self.stack.set_visible_child_name("results");
        self.results_provider.replace(self.provider_bar.active().id().to_owned());
        clear_children(&self.results_stack);
        clear_children(&self.toggle_container);
        self.toggle_group.replace(None);
        self.song_rows.borrow_mut().clear();

        let mut all: Vec<MediaItem> = Vec::new();
        let has_top = results.top_result.is_some();
        if let Some(top) = &results.top_result {
            all.push(top.clone());
        }
        all.extend(results.items);
        self.last_results.replace(all.clone());
        if all.is_empty() {
            self.results_stack.add_named(&adw::StatusPage::builder().icon_name("system-search-symbolic").title("No results").description(format!("Nothing found for \"{query}\"")).build(), Some("empty"));
            self.results_stack.set_visible_child_name("empty");
            return;
        }

        let group = adw::ToggleGroup::builder().css_classes(["round"]).build();
        self.toggle_container.append(&group);
        let compact = self.ctx.compact.get();
        let mut first_id: Option<&str> = None;
        let mut make_tab = |name: &str, id: &'static str, compact_name: &str| -> gtk::Box {
            let page_box = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(if compact { SECTION_SPACING_COMPACT } else { SECTION_SPACING }).margin_top(16).margin_bottom(24).margin_start(12).margin_end(12).build();
            self.results_stack.add_named(&page_box, Some(id));
            group.add(adw::Toggle::builder().name(id).label(if compact { compact_name } else { name }).build());
            if first_id.is_none() {
                first_id = Some(id);
            }
            page_box
        };

        let songs: Vec<MediaItem> = all.iter().filter(|r| r.kind == ItemKind::Song).cloned().collect();
        let artists: Vec<MediaItem> = all.iter().filter(|r| r.kind == ItemKind::Artist).cloned().collect();
        let playlists: Vec<MediaItem> = all.iter().filter(|r| r.kind == ItemKind::Playlist).cloned().collect();
        let albums: Vec<MediaItem> = all.iter().filter(|r| r.kind == ItemKind::Album).cloned().collect();
        let videos: Vec<MediaItem> = all.iter().filter(|r| r.kind == ItemKind::Video).cloned().collect();

        let main = make_tab("Main", "main", "Main");
        let rest = if has_top {
            self.add_result_section(&main, "Top Result", &all[..1]);
            &all[1..]
        } else {
            &all[..]
        };
        if !rest.is_empty() {
            self.add_result_section(&main, "Relevant Results", rest);
        }
        if !songs.is_empty() {
            let tab = make_tab("Songs", "songs", "Songs");
            self.add_result_section(&tab, "Songs", &songs);
        }
        if !artists.is_empty() {
            let tab = make_tab("Artists", "artists", "Artists");
            self.add_result_section(&tab, "Artists", &artists);
        }
        if !playlists.is_empty() {
            let tab = make_tab("Community Playlists", "playlists", "Playlists");
            self.add_result_section(&tab, "Playlists", &playlists);
        }
        if !albums.is_empty() || !videos.is_empty() {
            let tab = make_tab("Other results", "others", "Other");
            if !albums.is_empty() {
                self.add_result_section(&tab, "Albums", &albums);
            }
            let (episodes, videos): (Vec<MediaItem>, Vec<MediaItem>) = videos.into_iter().partition(|v| v.item_type.as_deref() == Some("Episode"));
            if !videos.is_empty() {
                self.add_result_section(&tab, "Videos", &videos);
            }
            if !episodes.is_empty() {
                self.add_result_section(&tab, "More results", &episodes);
            }
        }

        let results_stack = self.results_stack.clone();
        group.connect_active_name_notify(move |g| {
            if let Some(name) = g.active_name() {
                results_stack.set_visible_child_name(&name);
            }
        });
        if let Some(id) = first_id {
            group.set_active_name(Some(id));
            self.results_stack.set_visible_child_name(id);
        }
        self.toggle_group.replace(Some(group));
    }

    pub fn activate_first_playable(&self) -> bool {
        let results = self.last_results.borrow();
        let pool: Vec<MediaItem> = results.iter().filter(|i| i.kind.is_playable()).cloned().collect();
        match pool.first() {
            Some(item) => {
                tracing::info!(title = %item.title, id = %item.id, "activating first search result");
                activate_item(&self.ctx, item, &pool);
                true
            }
            None => false,
        }
    }

    fn render_chart_sections(self: &Rc<Self>, sections: Vec<crate::net::home::HomeSection>) {
        self.explore_loading.set(false);
        self.explore_loaded.set(true);
        self.explore_retry.set(0);
        self.clear_explore();
        for section in &sections {
            self.add_song_list(&self.explore_box, &section.title, &section.items, &self.explore_rows);
        }
    }

    fn add_result_section(self: &Rc<Self>, parent: &gtk::Box, title: &str, items: &[MediaItem]) {
        self.add_song_list(parent, title, items, &self.song_rows);
    }

    fn add_song_list(self: &Rc<Self>, parent: &gtk::Box, title: &str, items: &[MediaItem], rows: &RefCell<Vec<Rc<SongRow>>>) {
        let section = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).build();
        section.append(&heading(title));
        let list = gtk::ListBox::builder().css_classes(["boxed-list", "songs-list"]).selection_mode(gtk::SelectionMode::None).build();
        let pool: Vec<MediaItem> = items.iter().filter(|i| i.kind.is_playable()).cloned().collect();
        for item in items {
            if item.kind.is_playable() {
                let row = SongRow::new(self.ctx.clone());
                row.set_search_style(true);
                row.bind(item, None);
                list.append(row.widget());
                rows.borrow_mut().push(row);
            } else {
                let subtitle = search_subtitle(item);
                let (row, _) = song_row_with_subtitle(&self.ctx, item, Some(&subtitle));
                attach_item_menu(&self.ctx, &row, item.clone());
                list.append(&row);
            }
        }
        let ctx = self.ctx.clone();
        let items_c = items.to_vec();
        list.connect_row_activated(move |_, row| {
            if let Some(item) = items_c.get(row.index().max(0) as usize) {
                activate_item(&ctx, item, &pool);
            }
        });
        section.append(&list);
        parent.append(&section);
    }
}

fn heading(title: &str) -> gtk::Label {
    gtk::Label::builder().label(title).css_classes(["heading"]).halign(gtk::Align::Start).build()
}

fn capped(items: &[MediaItem], limit: usize) -> &[MediaItem] {
    &items[..items.len().min(limit)]
}

fn status_box(icon: &str, title: &str, subtitle: Option<&str>) -> gtk::Box {
    let status = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).valign(gtk::Align::Center).halign(gtk::Align::Center).vexpand(true).build();
    status.append(&gtk::Image::builder().icon_name(icon).pixel_size(48).css_classes(["dim-label"]).build());
    status.append(&gtk::Label::builder().label(title).css_classes(["title-3"]).build());
    if let Some(subtitle) = subtitle {
        status.append(&gtk::Label::builder().label(subtitle).css_classes(["dim-label"]).justify(gtk::Justification::Center).build());
    }
    status
}

fn local_results(downloads: &[crate::downloads::store::Entry], query: &str) -> Vec<MediaItem> {
    let needle = query.to_lowercase();
    downloads
        .iter()
        .filter(|d| [&d.title, &d.artist, &d.album].iter().any(|field| field.to_lowercase().contains(&needle)))
        .map(|d| MediaItem {
            kind: ItemKind::Song,
            id: d.video_id.clone(),
            title: d.title.clone(),
            artists: vec![crate::model::Person { name: d.artist.clone(), id: (!d.artist_id.is_empty()).then(|| d.artist_id.clone()) }],
            album: (!d.album.is_empty()).then(|| crate::model::Named { name: d.album.clone(), id: (!d.album_id.is_empty()).then(|| d.album_id.clone()) }),
            thumb: (!d.thumbnail_url.is_empty()).then(|| d.thumbnail_url.clone()),
            duration_seconds: d.duration_seconds,
            like_status: Some(d.like_status),
            ..MediaItem::default()
        })
        .collect()
}
