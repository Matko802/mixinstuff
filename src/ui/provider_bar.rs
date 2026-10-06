use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;

use crate::net::provider::{self, Provider};
use crate::paths::Paths;

pub struct ProviderBar {
    bar: gtk::Box,
    group: adw::ToggleGroup,
    toggles: Vec<(Provider, adw::Toggle)>,
    paths: Paths,
    syncing: Cell<bool>,
}

impl ProviderBar {
    pub fn new(paths: Paths, on_change: impl Fn() + 'static) -> Rc<Self> {
        let group = adw::ToggleGroup::builder().css_classes(["round"]).build();
        let mut toggles = Vec::new();
        for p in Provider::all() {
            let toggle = adw::Toggle::builder().name(p.id()).label(p.name()).icon_name(p.icon()).build();
            group.add(toggle.clone());
            toggles.push((p, toggle));
        }
        let bar = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).halign(gtk::Align::Center).margin_top(12).margin_bottom(4).build();
        bar.append(&group);
        let on_change = Rc::new(on_change);
        let this = Rc::new(Self { bar, group, toggles, paths, syncing: Cell::new(false) });
        {
            let weak = Rc::downgrade(&this);
            let on_change = on_change.clone();
            this.group.connect_active_name_notify(move |_| {
                let Some(this) = weak.upgrade() else { return };
                if this.syncing.get() {
                    return;
                }
                provider::set_active(&this.paths, this.active());
                on_change();
            });
        }
        this.refresh();
        this
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.bar
    }

    pub fn active(&self) -> Provider {
        self.group.active_name().map(|n| Provider::parse(n.as_str())).unwrap_or(Provider::YouTube)
    }

    pub fn refresh(&self) {
        self.syncing.set(true);
        let enabled = provider::enabled_providers(&self.paths);
        for (p, toggle) in &self.toggles {
            self.group.remove(toggle);
            if enabled.contains(p) {
                self.group.add(toggle.clone());
            }
        }
        let active = provider::active(&self.paths);
        self.group.set_active_name(Some(active.id()));
        self.bar.set_visible(enabled.len() > 1);
        self.syncing.set(false);
    }
}
