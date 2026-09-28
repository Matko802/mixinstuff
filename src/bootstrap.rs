//! Process-level setup that must run before GTK, GStreamer or any thread exists.
//! Mirrors the top of src/main.py.

use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::paths::Paths;

/// Cap glibc malloc arenas at 2.
/// GStreamer decodes on short-lived streaming threads and glibc gives each one
/// its own arena. Those arenas never shrink, so RSS climbed per track change.
/// Python had to re-exec with MALLOC_ARENA_MAX set; Rust can call mallopt
/// directly because main runs before any thread is spawned.
pub fn cap_malloc_arenas() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        // SAFETY: mallopt only writes allocator tunables; called before threads exist.
        let rc = unsafe { libc::mallopt(libc::M_ARENA_MAX, 2) };
        if rc != 1 {
            tracing::warn!("mallopt(M_ARENA_MAX) rejected");
        }
    }
}

/// Raise the open-file soft limit toward 65536.
/// Long sessions leaked into the default 1024 limit and network calls died.
pub fn raise_fd_limit() {
    #[cfg(unix)]
    {
        const TARGET: libc::rlim_t = 65_536;
        let mut lim = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        // SAFETY: rlimit is a plain C struct and the pointer is valid for the call.
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) } != 0 {
            return;
        }
        let target = if lim.rlim_max == libc::RLIM_INFINITY { TARGET } else { lim.rlim_max.min(TARGET) };
        if lim.rlim_cur >= target {
            return;
        }
        let old = lim.rlim_cur;
        lim.rlim_cur = target;
        // SAFETY: same struct, now populated.
        if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &lim) } == 0 {
            tracing::info!(old, new = target, "raised open-file soft limit");
        }
    }
}

/// Give the process the AppUserModelID the installer registers, before GTK
/// opens a window. Windows uses it to group the taskbar entry with the Start
/// menu shortcut and to name the app in the media flyout.
pub fn set_app_user_model_id() {
    #[cfg(windows)]
    // SAFETY: a static NUL-terminated string, set before any window exists.
    if let Err(err) = unsafe { windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(windows::core::w!("com.pocoguy.Muse")) } {
        tracing::warn!(%err, "AppUserModelID not set");
    }
}

/// Put the install's own folder first on PATH on Windows.
/// yt-dlp looks up node.exe and ffmpeg.exe on PATH, and the installer puts them beside mixtapes.exe.
pub fn prefer_bundled_programs() {
    #[cfg(windows)]
    {
        let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(std::path::Path::to_path_buf)) else {
            return;
        };
        let rest = std::env::var_os("PATH").unwrap_or_default();
        let Ok(joined) = std::env::join_paths(std::iter::once(dir).chain(std::env::split_paths(&rest))) else {
            return;
        };
        // SAFETY: called from main before any other thread exists.
        unsafe { std::env::set_var("PATH", joined) };
    }
}

/// Lay text out through fontconfig on Windows, like the Linux build, instead of Pango's win32 backend.
/// The bundled etc/fonts/fonts.conf adds the install's Adwaita fonts and the Windows font folders.
/// Both variables are read once when Pango creates its font map, so this runs before GTK loads.
pub fn use_bundled_fonts() {
    #[cfg(windows)]
    {
        let Some(root) = std::env::current_exe().ok().and_then(|exe| exe.parent()?.parent().map(std::path::Path::to_path_buf)) else {
            return;
        };
        let config = root.join("etc").join("fonts").join("fonts.conf");
        if !config.is_file() {
            return;
        }
        for (key, value) in [("PANGOCAIRO_BACKEND", std::ffi::OsString::from("fc")), ("FONTCONFIG_FILE", config.into_os_string())] {
            if std::env::var_os(key).is_none() {
                set_crt_env(key, &value);
            }
        }
    }
}

/// Let GTK render on the GPU on Windows. GTK 4.24's OpenGL and Vulkan renderers
/// need DirectComposition there, which is opt-in through GDK_DEBUG, so without
/// it every window falls back to the CPU renderer and animations crawl.
pub fn enable_gpu_rendering() {
    #[cfg(windows)]
    {
        let current = std::env::var("GDK_DEBUG").unwrap_or_default();
        if current.split([',', ':', ' ']).any(|flag| flag == "dcomp") {
            return;
        }
        let value = if current.is_empty() { "dcomp".to_owned() } else { format!("{current},dcomp") };
        set_crt_env("GDK_DEBUG", std::ffi::OsStr::new(&value));
    }
}

/// Sets a variable in both environments a Windows process has. Pango and
/// fontconfig read the C runtime's copy through getenv, which set_var
/// (SetEnvironmentVariableW) leaves untouched.
#[cfg(windows)]
fn set_crt_env(key: &str, value: &std::ffi::OsStr) {
    use std::os::windows::ffi::OsStrExt;
    unsafe extern "C" {
        fn _wputenv_s(name: *const u16, value: *const u16) -> i32;
    }
    let wide = |text: &std::ffi::OsStr| text.encode_wide().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (key_wide, value_wide) = (wide(std::ffi::OsStr::new(key)), wide(value));
    // SAFETY: called from main before any other thread exists; both buffers are
    // NUL-terminated and outlive the call.
    unsafe {
        std::env::set_var(key, value);
        _wputenv_s(key_wide.as_ptr(), value_wide.as_ptr());
    }
}

/// Honor the user's GSK renderer choice from prefs.json before GTK loads.
/// An explicit GSK_RENDERER in the environment wins.
pub fn apply_gsk_renderer_pref(paths: &Paths) {
    if std::env::var_os("GSK_RENDERER").is_some() {
        return;
    }
    let prefs = paths.read_prefs();
    let value = prefs.get("gsk_renderer").and_then(|v| v.as_str()).unwrap_or_default();
    if value.is_empty() || value == "default" {
        // Windows: GTK's GL renderer paints the shadow margin around the
        // window black under DirectComposition. Vulkan keeps it transparent.
        #[cfg(windows)]
        // SAFETY: called from main before any other thread exists.
        unsafe {
            std::env::set_var("GSK_RENDERER", "vulkan")
        };
        return;
    }
    // SAFETY: called from main before any other thread exists.
    unsafe { std::env::set_var("GSK_RENDERER", value) };
    tracing::info!(renderer = value, "applied GSK renderer preference");
}

/// Install the tracing subscriber.
/// config.json's debug_logs flag replaces the Python print() override.
/// RUST_LOG still overrides everything.
pub fn init_logging(paths: &Paths) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(filter_for(debug_logs(paths))));
    let (filter, handle) = tracing_subscriber::reload::Layer::new(filter);
    let _ = FILTER.set(handle);
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_target(false).compact())
        .with(log_file_layer(paths))
        .init();
}

/// Windows release builds have no console, so the log also goes to
/// %LOCALAPPDATA%\muse\mixtapes.log. Appended to, not recreated: a second
/// launch runs this before handing over to the first, and truncating would
/// cut the running instance's log. It starts over past LOG_FILE_LIMIT.
#[cfg(windows)]
fn log_file_layer<S>(paths: &Paths) -> Option<impl tracing_subscriber::Layer<S>>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    const LOG_FILE_LIMIT: u64 = 5 * 1024 * 1024;
    // The data folder, not the cache: GLib puts that in INetCache on Windows.
    let path = paths.data_dir.join("mixtapes.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > LOG_FILE_LIMIT) {
        let _ = std::fs::remove_file(&path);
    }
    let file = std::fs::OpenOptions::new().create(true).append(true).open(&path).ok()?;
    Some(tracing_subscriber::fmt::layer().with_target(false).with_ansi(false).compact().with_writer(std::sync::Mutex::new(file)))
}

#[cfg(not(windows))]
fn log_file_layer(_paths: &Paths) -> Option<tracing_subscriber::layer::Identity> {
    None
}

type FilterHandle = tracing_subscriber::reload::Handle<EnvFilter, tracing_subscriber::Registry>;

/// Lets the settings switch change verbosity without a restart.
static FILTER: std::sync::OnceLock<FilterHandle> = std::sync::OnceLock::new();

fn filter_for(debug: bool) -> &'static str {
    if debug { "mixtapes=debug,info" } else { "mixtapes=info,warn" }
}

pub fn debug_logs(paths: &Paths) -> bool {
    paths.read_config().get("debug_logs").and_then(|v| v.as_bool()).unwrap_or(false)
}

/// Port of logger.set_debug_logs: save the flag and apply it now. RUST_LOG still wins.
pub fn set_debug_logs(paths: &Paths, enabled: bool) {
    let mut config = paths.read_config();
    config.insert("debug_logs".into(), enabled.into());
    let write = serde_json::to_vec(&serde_json::Value::Object(config)).map_err(std::io::Error::other).and_then(|bytes| std::fs::write(&paths.config_file, bytes));
    if let Err(err) = write {
        tracing::warn!(%err, "could not save config.json");
    }
    if std::env::var_os("RUST_LOG").is_some() {
        return;
    }
    if let Some(handle) = FILTER.get() {
        let _ = handle.reload(EnvFilter::new(filter_for(enabled)));
    }
}
