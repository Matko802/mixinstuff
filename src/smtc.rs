//! System media controls outside Linux. Windows gets the System Media
//! Transport Controls here, the way `mpris.rs` serves MPRIS on Linux. Until
//! then this keeps the two calls main.rs makes.

use std::rc::Rc;

use crate::App;

pub struct Mpris;

impl Mpris {
    pub fn start(_ctx: &Rc<App>) -> Rc<Self> {
        Rc::new(Self)
    }

    pub fn shutdown(&self) {}
}
