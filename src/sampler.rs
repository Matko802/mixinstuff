//! A sampling profiler for the GTK thread, for devices with no perf or
//! sysprof, and for the Flatpak sandbox, which blocks perf_event_open. A
//! wall-clock timer signals the thread every millisecond and the handler
//! walks the frame pointers into a fixed buffer. The GNOME runtime and this
//! binary both keep frame pointers. `dump` writes the raw addresses and the
//! memory map, which `tools/symbolize_samples.py` turns into a profile.
//!
//! Demo only: MIXINSTUFF_DEMO_SAMPLE=path.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const WORDS: usize = 4 << 20;
const MAX_DEPTH: usize = 128;

static mut BUF: [usize; WORDS] = [0; WORDS];
static NEXT: AtomicUsize = AtomicUsize::new(0);
static STACK_LO: AtomicUsize = AtomicUsize::new(0);
static STACK_HI: AtomicUsize = AtomicUsize::new(0);
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Start sampling the calling thread at `hz`.
pub fn start(hz: i64) {
    unsafe {
        let mut attr: libc::pthread_attr_t = std::mem::zeroed();
        if libc::pthread_getattr_np(libc::pthread_self(), &mut attr) != 0 {
            return;
        }
        let mut addr: *mut libc::c_void = std::ptr::null_mut();
        let mut size: libc::size_t = 0;
        libc::pthread_attr_getstack(&attr, &mut addr, &mut size);
        libc::pthread_attr_destroy(&mut attr);
        STACK_LO.store(addr as usize, Ordering::Relaxed);
        STACK_HI.store(addr as usize + size, Ordering::Relaxed);

        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = handler as *const () as usize;
        action.sa_flags = libc::SA_SIGINFO | libc::SA_RESTART;
        libc::sigaction(libc::SIGPROF, &action, std::ptr::null_mut());

        let mut event: libc::sigevent = std::mem::zeroed();
        event.sigev_notify = libc::SIGEV_THREAD_ID;
        event.sigev_signo = libc::SIGPROF;
        event.sigev_notify_thread_id = libc::gettid();
        let mut timer: libc::timer_t = std::mem::zeroed();
        if libc::timer_create(libc::CLOCK_MONOTONIC, &mut event, &mut timer) != 0 {
            return;
        }
        let period = libc::timespec { tv_sec: 0, tv_nsec: 1_000_000_000 / hz };
        let spec = libc::itimerspec { it_interval: period, it_value: period };
        RUNNING.store(true, Ordering::Relaxed);
        libc::timer_settime(timer, 0, &spec, std::ptr::null_mut());
    }
}

/// Write `samples` (one line of hex addresses per sample, leaf first) and
/// `maps` (a copy of /proc/self/maps) next to `path`.
pub fn dump(path: &str) {
    RUNNING.store(false, Ordering::Relaxed);
    let end = NEXT.load(Ordering::Relaxed).min(WORDS);
    let mut out = String::new();
    let mut at = 0;
    while at < end {
        let len = unsafe { BUF[at] };
        if len == 0 || at + 1 + len > end {
            break;
        }
        let frames: Vec<String> = (0..len).map(|i| format!("{:x}", unsafe { BUF[at + 1 + i] })).collect();
        out.push_str(&frames.join(" "));
        out.push('\n');
        at += 1 + len;
    }
    let _ = std::fs::write(format!("{path}.samples"), out);
    // Read, not copied: procfs reports a size of zero, and a copy writes nothing.
    if let Ok(maps) = std::fs::read_to_string("/proc/self/maps") {
        let _ = std::fs::write(format!("{path}.maps"), maps);
    }
    tracing::info!(path, words = end, "demo: samples written");
}

extern "C" fn handler(_: libc::c_int, _: *mut libc::siginfo_t, context: *mut libc::c_void) {
    if !RUNNING.load(Ordering::Relaxed) {
        return;
    }
    let context = unsafe { &*(context as *const libc::ucontext_t) };
    #[cfg(target_arch = "aarch64")]
    let (pc, mut fp, lr) = (context.uc_mcontext.pc as usize, context.uc_mcontext.regs[29] as usize, context.uc_mcontext.regs[30] as usize);
    #[cfg(target_arch = "x86_64")]
    let (pc, mut fp, lr) = (context.uc_mcontext.gregs[libc::REG_RIP as usize] as usize, context.uc_mcontext.gregs[libc::REG_RBP as usize] as usize, 0usize);

    let mut frames = [0usize; MAX_DEPTH];
    let mut n = 0;
    frames[n] = pc;
    n += 1;
    // A leaf function on aarch64 may not have pushed its frame yet; the link register covers it.
    if lr != 0 {
        frames[n] = lr;
        n += 1;
    }
    let (lo, hi) = (STACK_LO.load(Ordering::Relaxed), STACK_HI.load(Ordering::Relaxed));
    while n < MAX_DEPTH && fp >= lo && fp + 16 <= hi && fp % 8 == 0 {
        let next = unsafe { *(fp as *const usize) };
        let ret = unsafe { *((fp + 8) as *const usize) };
        if ret == 0 {
            break;
        }
        frames[n] = ret;
        n += 1;
        if next <= fp {
            break;
        }
        fp = next;
    }
    let at = NEXT.fetch_add(n + 1, Ordering::Relaxed);
    if at + n + 1 > WORDS {
        return;
    }
    unsafe {
        let buf = &raw mut BUF;
        (*buf)[at] = n;
        for (i, frame) in frames[..n].iter().enumerate() {
            (*buf)[at + 1 + i] = *frame;
        }
    }
}
