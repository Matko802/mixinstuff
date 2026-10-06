
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gtk::{glib, prelude::*, subclass::prelude::*};

use crate::ui::widgets::media_card::{CARD_SIZE_COMPACT, CARD_SIZE_DEFAULT, GRID_LINE_SPACING, GRID_SPACING, MediaCard};

const CARD_PADDING: i32 = 20;
const CARD_SIZE_MIN: i32 = 110;
const CARD_SIZE_MAX: i32 = 190;

pub type CardList = Rc<RefCell<Vec<Rc<MediaCard>>>>;

pub fn column_size(width: i32, base: i32) -> Option<i32> {
    if width <= 0 {
        return None;
    }
    let mut best: Option<i32> = None;
    let mut columns = 1;
    loop {
        let size = (width - GRID_SPACING * (columns - 1)) / columns - CARD_PADDING;
        if size < CARD_SIZE_MIN && columns > 1 {
            break;
        }
        if best.is_none_or(|b| (size - base).abs() < (b - base).abs()) {
            best = Some(size);
        }
        columns += 1;
        if columns > 64 {
            break;
        }
    }
    best.map(|b| b.clamp(CARD_SIZE_MIN, CARD_SIZE_MAX))
}

fn children(widget: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut out = Vec::new();
    let mut child = widget.first_child();
    while let Some(c) = child {
        child = c.next_sibling();
        if c.should_layout() {
            out.push(c);
        }
    }
    out
}

mod layout_imp {
    use super::*;

    type Placement = (gtk::Widget, i32, i32, i32, i32);

    #[derive(Default)]
    pub struct CardGridLayout {
        pub cards: RefCell<Weak<RefCell<Vec<Rc<MediaCard>>>>>,
        pub compact: RefCell<Option<Rc<Cell<bool>>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CardGridLayout {
        const NAME: &'static str = "MusisharkCardGridLayout";
        type Type = super::CardGridLayout;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for CardGridLayout {}

    impl LayoutManagerImpl for CardGridLayout {
        fn request_mode(&self, _widget: &gtk::Widget) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, widget: &gtk::Widget, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            match orientation {
                gtk::Orientation::Horizontal => {
                    let kids = children(widget);
                    let nat: i32 = kids.iter().map(|c| c.measure(gtk::Orientation::Horizontal, -1).1).sum::<i32>() + GRID_SPACING * (kids.len() as i32 - 1).max(0);
                    let min = (CARD_SIZE_MIN + CARD_PADDING).min(nat.max(0));
                    (min, nat.max(min), -1, -1)
                }
                _ => {
                    self.sync(for_size);
                    let (height, _) = self.layout(widget, for_size);
                    (height, height, -1, -1)
                }
            }
        }

        fn allocate(&self, widget: &gtk::Widget, width: i32, _height: i32, _baseline: i32) {
            self.sync(width);
            for (child, x, y, w, h) in self.layout(widget, width).1 {
                child.size_allocate(&gtk::Allocation::new(x, y, w, h), -1);
            }
        }
    }

    impl CardGridLayout {
        fn sync(&self, width: i32) {
            let Some(cards) = self.cards.borrow().upgrade() else { return };
            let compact = self.compact.borrow().as_ref().is_some_and(|c| c.get());
            let base = if compact { CARD_SIZE_COMPACT } else { CARD_SIZE_DEFAULT };
            let Some(size) = column_size(width, base) else { return };
            for card in cards.borrow().iter() {
                card.set_card_size(size);
            }
        }

        fn layout(&self, widget: &gtk::Widget, width: i32) -> (i32, Vec<Placement>) {
            let kids = children(widget);
            if kids.is_empty() {
                return (0, Vec::new());
            }
            let child_w = kids.iter().map(|c| c.measure(gtk::Orientation::Horizontal, -1).1).max().unwrap_or(0).max(1);
            let columns = if width <= 0 { kids.len() } else { (((width + GRID_SPACING) / (child_w + GRID_SPACING)).max(1)) as usize };
            let mut placements = Vec::with_capacity(kids.len());
            let mut y = 0;
            for row in kids.chunks(columns) {
                let row_h = row.iter().map(|c| c.measure(gtk::Orientation::Vertical, child_w).1).max().unwrap_or(0);
                for (i, child) in row.iter().enumerate() {
                    placements.push((child.clone(), i as i32 * (child_w + GRID_SPACING), y, child_w, row_h));
                }
                y += row_h + GRID_LINE_SPACING;
            }
            (y - GRID_LINE_SPACING, placements)
        }
    }
}

glib::wrapper! {
    pub struct CardGridLayout(ObjectSubclass<layout_imp::CardGridLayout>) @extends gtk::LayoutManager;
}

mod grid_imp {
    use super::*;

    #[derive(Default)]
    pub struct CardGrid;

    #[glib::object_subclass]
    impl ObjectSubclass for CardGrid {
        const NAME: &'static str = "MusisharkCardGrid";
        type Type = super::CardGrid;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_layout_manager_type::<super::CardGridLayout>();
        }
    }

    impl ObjectImpl for CardGrid {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for CardGrid {}
}

glib::wrapper! {
    pub struct CardGrid(ObjectSubclass<grid_imp::CardGrid>) @extends gtk::Widget, @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for CardGrid {
    fn default() -> Self {
        Self::new()
    }
}

impl CardGrid {
    pub fn new() -> Self {
        glib::Object::new()
    }

    pub fn set_cards(&self, cards: &CardList, compact: Rc<Cell<bool>>) {
        let layout = self.layout_manager().and_downcast::<CardGridLayout>().expect("card grid layout");
        let imp = layout_imp::CardGridLayout::from_obj(&layout);
        imp.cards.replace(Rc::downgrade(cards));
        imp.compact.replace(Some(compact));
    }

    pub fn append(&self, child: &impl IsA<gtk::Widget>) {
        child.set_parent(self);
    }

    pub fn remove_all(&self) {
        while let Some(child) = self.first_child() {
            child.unparent();
        }
    }
}

mod card_layout_imp {
    use super::*;

    #[derive(Default)]
    pub struct CardLayout;

    #[glib::object_subclass]
    impl ObjectSubclass for CardLayout {
        const NAME: &'static str = "MusisharkCardLayout";
        type Type = super::CardLayout;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for CardLayout {}

    impl LayoutManagerImpl for CardLayout {
        fn request_mode(&self, _widget: &gtk::Widget) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, widget: &gtk::Widget, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let kids = children(widget);
            let width = kids.iter().map(|c| c.measure(gtk::Orientation::Horizontal, -1).0).max().unwrap_or(0);
            if orientation == gtk::Orientation::Horizontal {
                return (width, width, -1, -1);
            }
            let (mut min, mut nat) = (0, 0);
            for child in &kids {
                let (m, n, _, _) = child.measure(gtk::Orientation::Vertical, width);
                min = min.max(m);
                nat = nat.max(n);
            }
            let height = min.max(nat);
            (height, height, -1, -1)
        }

        fn allocate(&self, widget: &gtk::Widget, width: i32, height: i32, baseline: i32) {
            for child in children(widget) {
                child.allocate(width, height, baseline, None);
            }
        }
    }
}

glib::wrapper! {
    pub struct CardLayout(ObjectSubclass<card_layout_imp::CardLayout>) @extends gtk::LayoutManager;
}

impl Default for CardLayout {
    fn default() -> Self {
        Self::new()
    }
}

impl CardLayout {
    pub fn new() -> Self {
        glib::Object::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_five_columns_at_desktop_width() {
        let size = column_size(904, CARD_SIZE_DEFAULT).unwrap();
        let columns = (904 + GRID_SPACING) / (size + CARD_PADDING + GRID_SPACING);
        assert_eq!(columns, 5);
    }

    #[test]
    fn narrow_width_keeps_one_column_above_minimum() {
        assert_eq!(column_size(120, CARD_SIZE_DEFAULT), Some(CARD_SIZE_MIN));
    }
}
