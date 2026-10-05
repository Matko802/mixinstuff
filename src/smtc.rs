//! System media controls outside Linux, where `mpris.rs` serves MPRIS. On
//! Windows this is the System Media Transport Controls: the media flyout, the
//! lock screen and the keyboard's media keys. Elsewhere it keeps the two calls
//! main.rs makes and does nothing.

#[cfg(not(windows))]
pub use stub::Mpris;
#[cfg(windows)]
pub use win::Mpris;

#[cfg(not(windows))]
mod stub {
    use std::rc::Rc;

    use crate::App;

    pub struct Mpris;

    impl Mpris {
        pub fn start(_ctx: &Rc<App>) -> Rc<Self> {
            Rc::new(Self)
        }

        pub fn shutdown(&self) {}
    }
}

#[cfg(windows)]
mod win {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use gtk::glib;
    use gtk::prelude::*;
    use windows::Foundation::{TimeSpan, TypedEventHandler, Uri};
    use windows::Media::{
        MediaPlaybackStatus, MediaPlaybackType, PlaybackPositionChangeRequestedEventArgs, SystemMediaTransportControls,
        SystemMediaTransportControlsButton, SystemMediaTransportControlsButtonPressedEventArgs,
        SystemMediaTransportControlsTimelineProperties,
    };
    use windows::Storage::Streams::RandomAccessStreamReference;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::WinRT::ISystemMediaTransportControlsInterop;
    use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DefWindowProcW, RegisterClassW, WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSW};
    use windows::core::{HSTRING, w};

    use crate::App;
    use crate::model::PlaybackStatus;
    use crate::player::Player;

    /// The flyout's progress bar only needs a nudge now and then while playing.
    const TIMELINE_INTERVAL: Duration = Duration::from_secs(5);

    /// What Windows asks for, sent from its own threads to the GTK thread.
    #[derive(Debug)]
    enum Command {
        Play,
        Pause,
        Next,
        Previous,
        Stop,
        Seek(f64),
    }

    pub struct Mpris {
        controls: Option<SystemMediaTransportControls>,
        player: Rc<Player>,
        last_timeline: Cell<Option<Instant>>,
    }

    impl Mpris {
        /// Register with the media flyout. Failure is logged and the app carries on.
        pub fn start(ctx: &Rc<App>) -> Rc<Self> {
            let (tx, rx) = async_channel::unbounded::<Command>();
            let controls = match create_controls(tx) {
                Ok(controls) => {
                    tracing::info!("media transport controls registered");
                    Some(controls)
                }
                Err(err) => {
                    tracing::warn!(%err, "media transport controls unavailable");
                    None
                }
            };
            let this = Rc::new(Self { controls, player: ctx.player.clone(), last_timeline: Cell::new(None) });
            if this.controls.is_some() {
                this.pump_commands(rx);
                this.watch_state();
                this.refresh_status();
                this.refresh_metadata();
                this.refresh_timeline(true);
                this.refresh_bounds();
            }
            this
        }

        pub fn shutdown(&self) {
            if let Some(controls) = &self.controls {
                let _ = controls.SetPlaybackStatus(MediaPlaybackStatus::Closed);
                let _ = controls.DisplayUpdater().and_then(|updater| updater.ClearAll());
                let _ = controls.SetIsEnabled(false);
            }
        }

        fn pump_commands(self: &Rc<Self>, rx: async_channel::Receiver<Command>) {
            let player = self.player.clone();
            glib::spawn_future_local(async move {
                while let Ok(command) = rx.recv().await {
                    tracing::debug!(?command, "media transport command");
                    match command {
                        // Play on a stopped player starts the staged track, as with MPRIS.
                        Command::Play => match player.state().status() {
                            PlaybackStatus::Stopped => player.toggle_play(),
                            _ => player.play(),
                        },
                        Command::Pause => player.pause(),
                        Command::Next => player.next(),
                        Command::Previous => player.previous(),
                        Command::Stop => player.stop(),
                        Command::Seek(seconds) => player.seek(seconds),
                    }
                }
            });
        }

        fn watch_state(self: &Rc<Self>) {
            let state = self.player.state();
            let connect = |name: &str, f: fn(&Rc<Mpris>)| {
                let weak = Rc::downgrade(self);
                state.connect_notify_local(Some(name), move |_, _| {
                    if let Some(this) = weak.upgrade() {
                        f(&this);
                    }
                });
            };
            connect("status", |this| {
                this.refresh_status();
                this.refresh_timeline(true);
            });
            for name in ["title", "artist", "thumbnail-url", "video-id"] {
                connect(name, |this| this.refresh_metadata());
            }
            connect("duration", |this| this.refresh_timeline(true));
            connect("position", |this| this.refresh_timeline(false));
            for name in ["current-index", "queue-length"] {
                connect(name, |this| this.refresh_bounds());
            }
            let weak = Rc::downgrade(self);
            state.connect_seeked(move |_| {
                if let Some(this) = weak.upgrade() {
                    this.refresh_timeline(true);
                }
            });
        }

        fn refresh_status(&self) {
            let Some(controls) = &self.controls else { return };
            let status = match self.player.state().status() {
                PlaybackStatus::Playing => MediaPlaybackStatus::Playing,
                PlaybackStatus::Paused => MediaPlaybackStatus::Paused,
                PlaybackStatus::Loading => MediaPlaybackStatus::Changing,
                PlaybackStatus::Stopped if self.player.current_track().is_some() => MediaPlaybackStatus::Stopped,
                PlaybackStatus::Stopped => MediaPlaybackStatus::Closed,
            };
            if let Err(err) = controls.SetPlaybackStatus(status) {
                tracing::debug!(%err, "media transport status not set");
            }
        }

        /// Title, artists, album and cover for the flyout.
        fn refresh_metadata(&self) {
            let Some(controls) = &self.controls else { return };
            let state = self.player.state();
            let track = self.player.current_track();
            let artists = match track.as_ref().map(|t| &t.artists).filter(|a| !a.is_empty()) {
                Some(list) => list.iter().map(|a| a.name.as_str()).filter(|n| !n.is_empty()).collect::<Vec<_>>().join(", "),
                None => state.artist(),
            };
            let album = track.as_ref().and_then(|t| t.album.as_ref()).map(|a| a.name.clone()).unwrap_or_default();
            let thumbnail = state.thumbnail_url();
            let updated = (|| -> windows::core::Result<()> {
                let updater = controls.DisplayUpdater()?;
                updater.SetType(MediaPlaybackType::Music)?;
                let music = updater.MusicProperties()?;
                music.SetTitle(&HSTRING::from(state.title()))?;
                music.SetArtist(&HSTRING::from(artists))?;
                music.SetAlbumTitle(&HSTRING::from(album))?;
                let cover = if thumbnail.starts_with("http") {
                    Uri::CreateUri(&HSTRING::from(thumbnail)).and_then(|uri| RandomAccessStreamReference::CreateFromUri(&uri)).ok()
                } else {
                    None
                };
                updater.SetThumbnail(cover.as_ref())?;
                updater.Update()
            })();
            if let Err(err) = updated {
                tracing::debug!(%err, "media transport metadata not set");
            }
        }

        /// The progress bar. Position ticks arrive several times a second, so
        /// those only go out every few seconds; seeks and state changes go at once.
        fn refresh_timeline(&self, force: bool) {
            let Some(controls) = &self.controls else { return };
            if !force && self.last_timeline.get().is_some_and(|at| at.elapsed() < TIMELINE_INTERVAL) {
                return;
            }
            self.last_timeline.set(Some(Instant::now()));
            let state = self.player.state();
            let duration = state.duration().max(0.0);
            let ticks = |seconds: f64| TimeSpan { Duration: (seconds * 10_000_000.0) as i64 };
            let updated = (|| -> windows::core::Result<()> {
                let timeline = SystemMediaTransportControlsTimelineProperties::new()?;
                timeline.SetStartTime(ticks(0.0))?;
                timeline.SetMinSeekTime(ticks(0.0))?;
                timeline.SetEndTime(ticks(duration))?;
                timeline.SetMaxSeekTime(ticks(duration))?;
                timeline.SetPosition(ticks(state.position().clamp(0.0, duration)))?;
                controls.UpdateTimelineProperties(&timeline)
            })();
            if let Err(err) = updated {
                tracing::debug!(%err, "media transport timeline not set");
            }
        }

        fn refresh_bounds(&self) {
            let Some(controls) = &self.controls else { return };
            let bounds = self.player.bounds();
            let _ = controls.SetIsNextEnabled(bounds.can_next);
            // Previous also restarts the current track, so it always has something to do.
            let _ = controls.SetIsPreviousEnabled(bounds.can_previous || self.player.current_track().is_some());
        }
    }

    /// The controls belong to a window. A hidden one of our own keeps them off
    /// GTK's windows, which come and go; Windows names the entry from the
    /// process's AppUserModelID and the exe's file details.
    fn create_controls(tx: async_channel::Sender<Command>) -> windows::core::Result<SystemMediaTransportControls> {
        let hwnd = hidden_window()?;
        let interop = windows::core::factory::<SystemMediaTransportControls, ISystemMediaTransportControlsInterop>()?;
        // SAFETY: hwnd is a live window created on this thread.
        let controls: SystemMediaTransportControls = unsafe { interop.GetForWindow(hwnd)? };
        controls.SetIsEnabled(true)?;
        controls.SetIsPlayEnabled(true)?;
        controls.SetIsPauseEnabled(true)?;
        controls.SetIsStopEnabled(true)?;
        controls.SetIsNextEnabled(true)?;
        controls.SetIsPreviousEnabled(true)?;
        controls.SetPlaybackStatus(MediaPlaybackStatus::Closed)?;

        let buttons = tx.clone();
        controls.ButtonPressed(&TypedEventHandler::<SystemMediaTransportControls, SystemMediaTransportControlsButtonPressedEventArgs>::new(move |_, args| {
            let command = match args.ok()?.Button()? {
                SystemMediaTransportControlsButton::Play => Command::Play,
                SystemMediaTransportControlsButton::Pause => Command::Pause,
                SystemMediaTransportControlsButton::Next => Command::Next,
                SystemMediaTransportControlsButton::Previous => Command::Previous,
                SystemMediaTransportControlsButton::Stop => Command::Stop,
                _ => return Ok(()),
            };
            let _ = buttons.try_send(command);
            Ok(())
        }))?;
        controls.PlaybackPositionChangeRequested(&TypedEventHandler::<SystemMediaTransportControls, PlaybackPositionChangeRequestedEventArgs>::new(move |_, args| {
            let position = args.ok()?.RequestedPlaybackPosition()?;
            let _ = tx.try_send(Command::Seek(position.Duration as f64 / 10_000_000.0));
            Ok(())
        }))?;
        Ok(controls)
    }

    fn hidden_window() -> windows::core::Result<HWND> {
        unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
            // SAFETY: forwarding the arguments Windows passed in.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        let class_name = w!("MusisharkMediaControls");
        // SAFETY: plain Win32 calls with a static class name; the window is never
        // shown and lives as long as the process.
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let class = WNDCLASSW { lpfnWndProc: Some(window_proc), hInstance: instance.into(), lpszClassName: class_name, ..Default::default() };
            RegisterClassW(&class);
            // A top-level window, not a message-only one: GetForWindow needs a real window.
            CreateWindowExW(WINDOW_EX_STYLE(0), class_name, w!("Musishark"), WINDOW_STYLE(0), 0, 0, 0, 0, None, None, Some(instance.into()), None)
        }
    }
}
