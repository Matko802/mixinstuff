
use std::cell::RefCell;

use gio::prelude::*;
use glib::subclass::prelude::*;

use crate::model::MediaItem;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct MediaObject {
        pub item: RefCell<MediaItem>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MediaObject {
        const NAME: &'static str = "MxMediaObject";
        type Type = super::MediaObject;
    }

    impl ObjectImpl for MediaObject {}
}

glib::wrapper! {
    pub struct MediaObject(ObjectSubclass<imp::MediaObject>);
}

impl MediaObject {
    pub fn new(item: MediaItem) -> Self {
        let object: Self = glib::Object::new();
        object.imp().item.replace(item);
        object
    }

    pub fn item(&self) -> MediaItem {
        self.imp().item.borrow().clone()
    }

    pub fn id(&self) -> String {
        self.imp().item.borrow().id.clone()
    }
}

pub fn sync_store(store: &gio::ListStore, items: Vec<MediaItem>) {
    let current: Vec<MediaItem> = (0..store.n_items()).filter_map(|index| store.item(index).and_downcast::<MediaObject>()).map(|object| object.item()).collect();
    let head = current.iter().zip(items.iter()).take_while(|(a, b)| same_row(a, b)).count();
    let rest = current.len().min(items.len()) - head;
    let tail = current.iter().rev().zip(items.iter().rev()).take(rest).take_while(|(a, b)| same_row(a, b)).count();
    if head == current.len() && head == items.len() {
        return;
    }
    let replacements: Vec<MediaObject> = items[head..items.len() - tail].iter().cloned().map(MediaObject::new).collect();
    store.splice(head as u32, (current.len() - head - tail) as u32, &replacements);
}

fn same_row(a: &MediaItem, b: &MediaItem) -> bool {
    if a.thumb == b.thumb {
        return a == b;
    }
    let address = |thumb: &Option<String>| thumb.as_deref().map(|url| url.split('?').next().unwrap_or(url).to_owned());
    address(&a.thumb) == address(&b.thumb) && MediaItem { thumb: None, ..a.clone() } == MediaItem { thumb: None, ..b.clone() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ItemKind;

    fn item(id: &str, title: &str) -> MediaItem {
        MediaItem { kind: ItemKind::Playlist, id: id.to_owned(), title: title.to_owned(), ..MediaItem::default() }
    }

    fn ids(store: &gio::ListStore) -> Vec<String> {
        (0..store.n_items()).filter_map(|i| store.item(i).and_downcast::<MediaObject>()).map(|o| o.id()).collect()
    }

    #[test]
    fn rows_that_did_not_change_keep_their_object() {
        let store = gio::ListStore::new::<MediaObject>();
        sync_store(&store, vec![item("a", "One"), item("b", "Two")]);
        let first = store.item(0).and_downcast::<MediaObject>().unwrap();

        sync_store(&store, vec![item("a", "One"), item("b", "Two changed")]);
        let after = store.item(0).and_downcast::<MediaObject>().unwrap();
        assert_eq!(first, after, "the untouched row is the same object, so its card is not rebound");
        assert_eq!(store.item(1).and_downcast::<MediaObject>().unwrap().item().title, "Two changed");
    }

    #[test]
    fn a_freshly_signed_address_is_not_a_change() {
        let mut signed = item("a", "One");
        signed.thumb = Some("https://i.ytimg.com/pl_c/A/studio_square_thumbnail.jpg?sqp=one&rs=first".to_owned());
        let mut resigned = signed.clone();
        resigned.thumb = Some("https://i.ytimg.com/pl_c/A/studio_square_thumbnail.jpg?sqp=two&rs=second".to_owned());
        assert!(same_row(&signed, &resigned));

        let mut elsewhere = signed.clone();
        elsewhere.thumb = Some("https://i.ytimg.com/pl_c/B/studio_square_thumbnail.jpg?sqp=one".to_owned());
        assert!(!same_row(&signed, &elsewhere));

        let mut renamed = resigned.clone();
        renamed.title = "One renamed".to_owned();
        assert!(!same_row(&signed, &renamed), "the rest of the row still counts");
    }

    #[test]
    fn a_resigned_row_keeps_the_address_it_already_had() {
        let store = gio::ListStore::new::<MediaObject>();
        let mut first = item("a", "One");
        first.thumb = Some("https://i.ytimg.com/pl_c/A/studio_square_thumbnail.jpg?sqp=one".to_owned());
        sync_store(&store, vec![first.clone()]);
        let mut resigned = first.clone();
        resigned.thumb = Some("https://i.ytimg.com/pl_c/A/studio_square_thumbnail.jpg?sqp=two".to_owned());
        sync_store(&store, vec![resigned]);
        assert_eq!(store.item(0).and_downcast::<MediaObject>().unwrap().item().thumb, first.thumb);
    }

    #[test]
    fn a_single_change_moves_a_single_row() {
        let store = gio::ListStore::new::<MediaObject>();
        sync_store(&store, vec![item("a", "One"), item("b", "Two"), item("c", "Three")]);
        let (first, last) = (store.item(0).and_downcast::<MediaObject>().unwrap(), store.item(2).and_downcast::<MediaObject>().unwrap());
        let changes = std::rc::Rc::new(std::cell::Cell::new(0));
        let counter = changes.clone();
        store.connect_items_changed(move |_, _, removed, added| counter.set(counter.get() + removed.max(added)));

        sync_store(&store, vec![item("a", "One"), item("b", "Two but different"), item("c", "Three")]);
        assert_eq!(changes.get(), 1, "one row changed, so one row moved");
        assert_eq!(first, store.item(0).and_downcast::<MediaObject>().unwrap());
        assert_eq!(last, store.item(2).and_downcast::<MediaObject>().unwrap());
    }

    #[test]
    fn a_reload_that_changed_nothing_says_nothing() {
        let store = gio::ListStore::new::<MediaObject>();
        sync_store(&store, vec![item("a", "One"), item("b", "Two")]);
        let changes = std::rc::Rc::new(std::cell::Cell::new(0));
        let counter = changes.clone();
        store.connect_items_changed(move |_, _, _, _| counter.set(counter.get() + 1));
        sync_store(&store, vec![item("a", "One"), item("b", "Two")]);
        assert_eq!(changes.get(), 0);
    }

    #[test]
    fn the_store_follows_additions_and_removals() {
        let store = gio::ListStore::new::<MediaObject>();
        sync_store(&store, vec![item("a", "One"), item("b", "Two")]);
        sync_store(&store, vec![item("a", "One"), item("b", "Two"), item("c", "Three")]);
        assert_eq!(ids(&store), ["a", "b", "c"]);
        sync_store(&store, vec![item("c", "Three")]);
        assert_eq!(ids(&store), ["c"]);
        sync_store(&store, Vec::new());
        assert_eq!(store.n_items(), 0);
    }
}
