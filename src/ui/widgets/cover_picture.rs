//! Large cover art as a gtk::Picture with cover fit, loaded through the
//! shared texture loader. Port of the parts of AsyncPicture the player views use.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::{gdk, glib};

use crate::net::NetHandle;
use crate::ui::cover::{self, load_texture};

pub struct CoverPicture {
    picture: gtk::Picture,
    net: NetHandle,
    current: RefCell<Option<String>>,
    /// Track whose cover was asked for, empty for a plain address.
    track: RefCell<String>,
}

impl CoverPicture {
    pub fn new(net: NetHandle) -> Rc<Self> {
        let picture = gtk::Picture::builder().content_fit(gtk::ContentFit::Cover).can_shrink(true).build();
        Rc::new(Self { picture, net, current: RefCell::new(None), track: RefCell::new(String::new()) })
    }

    pub fn widget(&self) -> &gtk::Picture {
        &self.picture
    }

    #[allow(dead_code)]
    pub fn url(&self) -> Option<String> {
        self.current.borrow().clone()
    }

    /// Load a track's cover: the downloaded file's own art when there is
    /// one, so it shows offline, else `url`.
    pub fn load_track(self: &Rc<Self>, video_id: &str, url: &str) {
        self.track.replace(video_id.to_owned());
        if let Some(path) = cover::cached_track_art(video_id) {
            self.load_address(&path.to_string_lossy());
            return;
        }
        self.load_address(url);
        if !cover::is_downloaded(video_id) {
            return;
        }
        // Downloaded before the cover was kept: pull it out of the file, then swap.
        let (net, id) = (self.net.clone(), video_id.to_owned());
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Some(path) = cover::track_art(&net, &id).await else { return };
            let Some(this) = weak.upgrade() else { return };
            if *this.track.borrow() == id {
                this.load_address(&path.to_string_lossy());
            }
        });
    }

    pub fn load(self: &Rc<Self>, url: &str) {
        self.track.replace(String::new());
        self.load_address(url);
    }

    fn load_address(self: &Rc<Self>, url: &str) {
        if url.is_empty() {
            self.current.replace(None);
            self.picture.set_paintable(gdk::Paintable::NONE);
            return;
        }
        let url = url.to_owned();
        if self.current.borrow().as_deref() == Some(url.as_str()) {
            return;
        }
        self.current.replace(Some(url.clone()));
        let net = self.net.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            // None: this one is drawn as large as the window allows.
            let texture = load_texture(&net, &url, None).await;
            let Some(this) = weak.upgrade() else { return };
            if this.current.borrow().as_deref() != Some(url.as_str()) {
                return;
            }
            if texture.is_none() {
                // Forgotten, so the next metadata update asks again instead of seeing the same address.
                this.current.replace(None);
            }
            this.picture.set_paintable(texture.as_ref());
        });
    }
}
