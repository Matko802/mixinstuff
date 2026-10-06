
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use crate::downloads::{Downloads, Event as DownloadEvent};
use crate::model::{MediaItem, Track};
use crate::net::explore::Category;
use crate::net::NetHandle;
use crate::net::online::Online;
use crate::paths::Paths;
use crate::player::Player;

#[derive(Clone, Debug)]
pub enum NavRequest {
    Playlist { id: String, title: String, thumb: Option<String> },
    Album { id: String, title: String, thumb: Option<String> },
    Artist { id: Option<String>, name: String },
    Category { title: String, params: String },
    AllMoods { title: String, items: Vec<Category> },
    Search { query: String },
    Discography { channel_id: String, title: String, browse_id: Option<String>, params: Option<String>, initial: Vec<MediaItem> },
}

type NavSink = Box<dyn Fn(NavRequest)>;

type CardSink = Box<dyn Fn(&str)>;

type UploadSink = Box<dyn Fn()>;

#[derive(Default)]
pub struct Navigator {
    sink: RefCell<Option<NavSink>>,
    library_refresh: RefCell<Option<Box<dyn Fn()>>>,
    library_card_refresh: RefCell<Option<CardSink>>,
    upload_picker: RefCell<Option<UploadSink>>,
}

impl Navigator {
    pub fn set_sink(&self, sink: impl Fn(NavRequest) + 'static) {
        self.sink.replace(Some(Box::new(sink)));
    }

    pub fn set_library_refresh(&self, f: impl Fn() + 'static) {
        self.library_refresh.replace(Some(Box::new(f)));
    }

    pub fn set_library_card_refresh(&self, f: impl Fn(&str) + 'static) {
        self.library_card_refresh.replace(Some(Box::new(f)));
    }

    pub fn set_upload_picker(&self, f: impl Fn() + 'static) {
        self.upload_picker.replace(Some(Box::new(f)));
    }

    pub fn pick_uploads(&self) {
        if let Some(f) = self.upload_picker.borrow().as_ref() {
            f();
        }
    }

    pub fn refresh_library_card(&self, playlist_id: &str) {
        if let Some(f) = self.library_card_refresh.borrow().as_ref() {
            f(playlist_id);
        }
    }

    pub fn refresh_library(&self) {
        if let Some(f) = self.library_refresh.borrow().as_ref() {
            f();
        }
    }

    pub fn go(&self, request: NavRequest) {
        match self.sink.borrow().as_ref() {
            Some(sink) => sink(request),
            None => tracing::warn!(?request, "navigation before the window exists"),
        }
    }
}

type CompactListener = Box<dyn Fn(bool) -> bool>;

type DownloadListener = Box<dyn Fn(&DownloadEvent) -> bool>;

type DownloadSink = Box<dyn Fn(Vec<Track>, String, String)>;

pub struct UiContext {
    pub player: Rc<Player>,
    pub net: NetHandle,
    pub paths: Paths,
    pub nav: Rc<Navigator>,
    pub online: Rc<Online>,
    pub downloads: Arc<Downloads>,
    pub lyrics: crate::lyrics::Lyrics,
    pub local: Arc<crate::local_library::LocalLibrary>,
    pub compact: Cell<bool>,
    compact_listeners: RefCell<Vec<CompactListener>>,
    download_listeners: RefCell<Vec<DownloadListener>>,
    download_sink: RefCell<Option<DownloadSink>>,
}

impl UiContext {
    pub fn new(player: Rc<Player>, net: NetHandle, paths: Paths, downloads: Arc<Downloads>, lyrics: crate::lyrics::Lyrics, local: Arc<crate::local_library::LocalLibrary>) -> Rc<Self> {
        let online = Online::new(net.clone(), paths.clone());
        Rc::new(Self {
            player,
            net,
            paths,
            nav: Rc::new(Navigator::default()),
            online,
            downloads,
            lyrics,
            local,
            compact: Cell::new(false),
            compact_listeners: RefCell::new(Vec::new()),
            download_listeners: RefCell::new(Vec::new()),
            download_sink: RefCell::new(None),
        })
    }

    pub fn pump_downloads(self: &Rc<Self>, events: async_channel::Receiver<DownloadEvent>) {
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok(event) = events.recv().await {
                let Some(ctx) = weak.upgrade() else { return };
                let listeners = std::mem::take(&mut *ctx.download_listeners.borrow_mut());
                let kept: Vec<DownloadListener> = listeners.into_iter().filter(|f| f(&event)).collect();
                ctx.download_listeners.borrow_mut().extend(kept);
            }
        });
    }

    pub fn set_download_sink(&self, sink: impl Fn(Vec<Track>, String, String) + 'static) {
        self.download_sink.replace(Some(Box::new(sink)));
    }

    pub fn download(&self, tracks: Vec<Track>, album_title: &str, album_id: &str) {
        match self.download_sink.borrow().as_ref() {
            Some(sink) => sink(tracks, album_title.to_owned(), album_id.to_owned()),
            None => self.downloads.queue_tracks(tracks, album_title, album_id),
        }
    }

    pub fn on_download(&self, listener: impl Fn(&DownloadEvent) -> bool + 'static) {
        self.download_listeners.borrow_mut().push(Box::new(listener));
    }

    pub fn set_compact(&self, compact: bool) {
        self.compact.set(compact);
        let listeners = std::mem::take(&mut *self.compact_listeners.borrow_mut());
        let kept: Vec<CompactListener> = listeners.into_iter().filter(|f| f(compact)).collect();
        self.compact_listeners.borrow_mut().extend(kept);
    }

    pub fn on_compact(&self, listener: impl Fn(bool) -> bool + 'static) {
        if listener(self.compact.get()) {
            self.compact_listeners.borrow_mut().push(Box::new(listener));
        }
    }
}
