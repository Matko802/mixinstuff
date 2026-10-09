
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use gtk::{gdk, glib, graphene, prelude::*, subclass::prelude::*};

use crate::model::PlaybackStatus;
use crate::player::Player;
use crate::ui::context::UiContext;
use crate::ui::widgets::sharkvis::{RawMode, SharkvisFeed};

const GRAVITY: f64 = 0.0028;
const BARS_DEFAULT: usize = 56;

mod area {
    use super::*;

    type Draw = Box<dyn Fn(&gtk::Widget, &gtk::Snapshot, f32, f32)>;

    #[derive(Default)]
    pub struct BarsArea {
        pub draw: std::cell::RefCell<Option<Draw>>,
        pub height: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for BarsArea {
        const NAME: &'static str = "MxVisualizerBars";
        type Type = super::BarsArea;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for BarsArea {}

    impl WidgetImpl for BarsArea {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            match orientation {
                gtk::Orientation::Vertical => (self.height.get(), self.height.get(), -1, -1),
                _ => (0, 0, -1, -1),
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            if let Some(draw) = self.draw.borrow().as_ref() {
                draw(widget.upcast_ref(), snapshot, widget.width() as f32, widget.height() as f32);
            }
        }
    }
}

glib::wrapper! {
    pub struct BarsArea(ObjectSubclass<area::BarsArea>) @extends gtk::Widget, @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

pub struct Visualizer {
    area: BarsArea,
    player: Rc<Player>,
    feed: Arc<SharkvisFeed>,
    bars: Cell<usize>,
    mode: Cell<RawMode>,
    levels: RefCell<Vec<f64>>,
    velocities: RefCell<Vec<f64>>,
    active: Cell<bool>,
    tick: RefCell<Option<gtk::TickCallbackId>>,
}

impl Visualizer {
    pub fn new(ctx: &Rc<UiContext>, height: i32) -> Rc<Self> {
        let prefs = ctx.paths.read_prefs();
        let bars = prefs.get("visualizer_bars").and_then(|v| v.as_u64()).map(|n| n.clamp(8, 100) as usize).unwrap_or(BARS_DEFAULT);
        let mode = prefs.get("visualizer_mode").and_then(|v| v.as_str()).and_then(RawMode::parse).unwrap_or(RawMode::Bars);
        let enabled = prefs.get("visualizer_enabled").and_then(|v| v.as_bool()).unwrap_or(true);

        let area: BarsArea = glib::Object::builder().property("visible", enabled).build();
        area.imp().height.set(height);
        let this = Rc::new(Self {
            area,
            player: ctx.player.clone(),
            feed: SharkvisFeed::global(bars, mode),
            bars: Cell::new(bars),
            mode: Cell::new(mode),
            levels: RefCell::new(Vec::new()),
            velocities: RefCell::new(Vec::new()),
            active: Cell::new(false),
            tick: RefCell::new(None),
        });
        let weak = Rc::downgrade(&this);
        this.area.imp().draw.replace(Some(Box::new(move |area, snapshot, w, h| {
            if let Some(v) = weak.upgrade() {
                v.draw(area, snapshot, w, h);
            }
        })));
        let weak = Rc::downgrade(&this);
        this.area.connect_map(move |_| {
            if let Some(v) = weak.upgrade() {
                v.sync_tick();
            }
        });
        let weak = Rc::downgrade(&this);
        this.area.connect_unmap(move |_| {
            if let Some(v) = weak.upgrade() {
                v.stop_tick();
            }
        });
        this
    }

    pub fn widget(&self) -> &BarsArea {
        &self.area
    }

    pub fn set_bar_count(&self, n: usize) {
        let n = n.clamp(8, 100);
        if n == self.bars.get() {
            return;
        }
        self.bars.set(n);
        self.levels.borrow_mut().clear();
        self.velocities.borrow_mut().clear();
        self.feed.set_bar_count(n);
        self.area.queue_draw();
    }

    pub fn set_mode(&self, mode: RawMode) {
        if mode == self.mode.get() {
            return;
        }
        self.mode.set(mode);
        self.levels.borrow_mut().clear();
        self.velocities.borrow_mut().clear();
        self.feed.set_mode(mode);
        self.area.queue_draw();
    }

    pub fn set_active(self: &Rc<Self>, active: bool) {
        self.active.set(active);
        self.sync_tick();
    }

    fn sync_tick(self: &Rc<Self>) {
        let settling = self.levels.borrow().iter().any(|level| *level > 0.0);
        if !(self.active.get() || settling) || !self.area.is_mapped() {
            self.stop_tick();
            self.area.queue_draw();
            return;
        }
        if self.tick.borrow().is_some() {
            return;
        }
        let weak = Rc::downgrade(self);
        let id = self.area.add_tick_callback(move |_, _| match weak.upgrade() {
            Some(v) => {
                v.on_tick();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        self.tick.replace(Some(id));
    }

    fn stop_tick(&self) {
        if let Some(id) = self.tick.borrow_mut().take() {
            id.remove();
        }
    }

    fn on_tick(self: &Rc<Self>) {
        if !self.active.get() {
            if self.levels.borrow().iter().all(|level| *level <= 0.0) {
                let weak = Rc::downgrade(self);
                glib::idle_add_local_once(move || {
                    if let Some(v) = weak.upgrade() {
                        v.sync_tick();
                    }
                });
            }
        } else if self.player.state().status() == PlaybackStatus::Playing {
            let frame = match self.mode.get() {
                RawMode::Oscilloscope => self.feed.pull_raw().map(|raw| resample_pairs(&raw, self.bars.get())),
                _ => self.feed.pull(self.bars.get()),
            };
            if let Some(frame) = frame {
                let mut levels = self.levels.borrow_mut();
                let mut velocities = self.velocities.borrow_mut();
                if levels.len() != frame.len() {
                    *levels = vec![0.0; frame.len()];
                    *velocities = vec![0.0; frame.len()];
                }
                for (i, h) in frame.iter().enumerate() {
                    if *h > levels[i] {
                        levels[i] = *h;
                        velocities[i] = 0.0;
                    }
                }
            }
        }
        {
            let mut levels = self.levels.borrow_mut();
            let mut velocities = self.velocities.borrow_mut();
            for i in 0..levels.len() {
                if levels[i] > 0.0 {
                    velocities[i] += GRAVITY;
                    levels[i] -= velocities[i];
                    if levels[i] <= 0.0 {
                        levels[i] = 0.0;
                        velocities[i] = 0.0;
                    }
                }
            }
        }
        self.area.queue_draw();
    }

    fn draw(&self, area: &gtk::Widget, snapshot: &gtk::Snapshot, width: f32, height: f32) {
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        match self.mode.get() {
            RawMode::Bars => self.draw_bars(area, snapshot, width, height),
            RawMode::Wave => self.draw_wave(area, snapshot, width, height),
            RawMode::Oscilloscope => self.draw_scope(area, snapshot, width, height),
        }
    }

    fn draw_bars(&self, area: &gtk::Widget, snapshot: &gtk::Snapshot, width: f32, height: f32) {
        let n = self.bars.get();
        let levels = self.levels.borrow();
        let (r, g, b) = bar_color(area);
        let gap = 2.0f32;
        let bar_w = ((width - gap * (n as f32 - 1.0)) / n as f32).max(1.0);
        let min_h = 2.0f32;
        for i in 0..n {
            let level = levels.get(i).copied().unwrap_or(0.0);
            let h = (level as f32 * height).max(min_h);
            let x = i as f32 * (bar_w + gap);
            let rect = graphene::Rect::new(x, height - h, bar_w, h);
            snapshot.append_color(&gdk::RGBA::new(r as f32, g as f32, b as f32, 1.0), &rect);
        }
    }

    fn draw_wave(&self, area: &gtk::Widget, snapshot: &gtk::Snapshot, width: f32, height: f32) {
        let levels = self.levels.borrow();
        let n = levels.len();
        if n < 2 {
            return;
        }
        let (r, g, b) = bar_color(area);
        let bounds = graphene::Rect::new(0.0, 0.0, width, height);
        let cr = snapshot.append_cairo(&bounds);
        cr.set_source_rgb(r, g, b);
        cr.set_line_width(2.0);
        cr.set_line_join(cairo::LineJoin::Round);
        cr.set_line_cap(cairo::LineCap::Round);
        for (i, v) in levels.iter().enumerate() {
            let x = f64::from(i as f32 / (n - 1) as f32) * f64::from(width);
            let y = f64::from(height) - v.clamp(0.0, 1.0) * f64::from(height);
            if i == 0 {
                cr.move_to(x, y);
            } else {
                cr.line_to(x, y);
            }
        }
        let _ = cr.stroke();
    }

    fn draw_scope(&self, area: &gtk::Widget, snapshot: &gtk::Snapshot, width: f32, height: f32) {
        let levels = self.levels.borrow();
        let n = levels.len() / 2;
        if n < 2 {
            return;
        }
        let (r, g, b) = bar_color(area);
        let bounds = graphene::Rect::new(0.0, 0.0, width, height);
        let cr = snapshot.append_cairo(&bounds);
        cr.set_source_rgb(r, g, b);
        cr.set_line_width(2.0);
        cr.set_line_join(cairo::LineJoin::Round);
        cr.set_line_cap(cairo::LineCap::Round);
        for i in 0..n {
            let x = levels[2 * i].clamp(0.0, 1.0) * f64::from(width);
            let y = f64::from(height) - levels[2 * i + 1].clamp(0.0, 1.0) * f64::from(height);
            if i == 0 {
                cr.move_to(x, y);
            } else {
                cr.line_to(x, y);
            }
        }
        let _ = cr.stroke();
    }
}

fn resample_pairs(raw: &[f32], bars: usize) -> Vec<f64> {
    let pairs = raw.len() / 2;
    if pairs == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(bars * 2);
    for i in 0..bars {
        let p = (i * pairs) / bars;
        out.push(f64::from(raw[2 * p].clamp(0.0, 1.0)));
        out.push(f64::from(raw[2 * p + 1].clamp(0.0, 1.0)));
    }
    out
}

#[allow(deprecated)]
fn bar_color(area: &gtk::Widget) -> (f64, f64, f64) {
    let ctx = area.style_context();
    ["visualizer_bar", "accent_color"]
        .iter()
        .find_map(|name| ctx.lookup_color(name))
        .map(|c| (f64::from(c.red()), f64::from(c.green()), f64::from(c.blue())))
        .unwrap_or((0.42, 0.34, 0.85))
}
