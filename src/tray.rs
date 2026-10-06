//! Linux has the shell's media controls (MPRIS), so there is no
//! notification-area icon. Kept as two no-ops so callers stay unchanged.

pub fn start(_ctx: &std::rc::Rc<crate::App>) {}

pub fn stop() {}
