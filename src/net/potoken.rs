
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rustypipe_botguard::Botguard;
use tokio::sync::oneshot;

use crate::paths::Paths;

const EARLY: Duration = Duration::from_secs(300);
const RETRY_AFTER: Duration = Duration::from_secs(60);


struct Token {
    value: String,
    expires: SystemTime,
}

type Reply = oneshot::Sender<Option<(String, SystemTime)>>;

pub struct PoTokens {
    snapshot: PathBuf,
    minted: Mutex<HashMap<String, Token>>,
    minter: Mutex<Option<mpsc::Sender<(String, Reply)>>>,
}

impl PoTokens {
    pub fn new(paths: &Paths) -> Self {
        Self { snapshot: paths.cache_dir.join("botguard_snapshot.bin"), minted: Mutex::new(HashMap::new()), minter: Mutex::new(None) }
    }

    pub async fn for_video(&self, video_id: &str) -> Option<String> {
        if let Some(token) = self.cached(video_id) {
            return Some(token);
        }
        let (reply, answer) = oneshot::channel();
        self.minter().send((video_id.to_owned(), reply)).ok()?;
        let (value, expires) = answer.await.ok()??;
        tracing::debug!(video_id, "po token minted");
        self.minted.lock().unwrap().insert(video_id.to_owned(), Token { value: value.clone(), expires });
        Some(value)
    }

    fn cached(&self, video_id: &str) -> Option<String> {
        let mut minted = self.minted.lock().unwrap();
        match minted.get(video_id) {
            Some(token) if token.expires > SystemTime::now() => Some(token.value.clone()),
            Some(_) => {
                minted.remove(video_id);
                None
            }
            None => None,
        }
    }

    fn minter(&self) -> mpsc::Sender<(String, Reply)> {
        let mut minter = self.minter.lock().unwrap();
        if let Some(sender) = minter.as_ref() {
            return sender.clone();
        }
        let (sender, requests) = mpsc::channel();
        let snapshot = self.snapshot.clone();
        let spawned = std::thread::Builder::new().name("botguard".into()).spawn(move || Minter::new(snapshot).run(requests));
        if let Err(err) = spawned {
            tracing::warn!(%err, "botguard thread did not start");
        }
        *minter = Some(sender.clone());
        sender
    }
}

struct Minter {
    snapshot: PathBuf,
    botguard: Option<Botguard>,
    snapshot_used: bool,
    failed_at: Option<Instant>,
}

impl Minter {
    fn new(snapshot: PathBuf) -> Self {
        Self { snapshot, botguard: None, snapshot_used: false, failed_at: None }
    }

    fn run(mut self, requests: mpsc::Receiver<(String, Reply)>) {
        let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(runtime) => runtime,
            Err(err) => {
                tracing::warn!(%err, "botguard runtime did not start");
                return;
            }
        };
        while let Ok((video_id, reply)) = requests.recv() {
            let token = runtime.block_on(self.mint(&video_id));
            let _ = reply.send(token);
        }
    }

    async fn mint(&mut self, video_id: &str) -> Option<(String, SystemTime)> {
        self.ensure().await?;
        let botguard = self.botguard.as_mut()?;
        let expires = expiry(botguard.valid_until().unix_timestamp())?;
        match botguard.mint_token(video_id).await {
            Ok(value) => Some((value, expires)),
            Err(err) => {
                tracing::warn!(%err, video_id, "po token minting failed");
                self.botguard = None;
                None
            }
        }
    }

    async fn ensure(&mut self) -> Option<()> {
        if let Some(botguard) = &self.botguard {
            if expiry(botguard.valid_until().unix_timestamp()).is_some_and(|at| at > SystemTime::now()) {
                return Some(());
            }
            self.botguard = None;
        }
        if self.failed_at.is_some_and(|at| at.elapsed() < RETRY_AFTER) {
            return None;
        }
        let started = Instant::now();
        let made = if self.snapshot_used { self.fresh().await } else { self.first().await };
        match made {
            Some(botguard) => {
                tracing::info!(took_ms = started.elapsed().as_millis() as u64, from_snapshot = botguard.is_from_snapshot(), "botguard ready");
                self.botguard = Some(botguard);
                self.failed_at = None;
                Some(())
            }
            None => {
                self.failed_at = Some(Instant::now());
                None
            }
        }
    }

    async fn first(&mut self) -> Option<Botguard> {
        self.snapshot_used = true;
        if let Some(dir) = self.snapshot.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let botguard = log_error(Botguard::builder().snapshot_path(&self.snapshot).init().await)?;
        if botguard.is_from_snapshot() {
            return Some(botguard);
        }
        if !botguard.write_snapshot().await {
            return self.fresh().await;
        }
        log_error(Botguard::builder().snapshot_path(&self.snapshot).init().await)
    }

    async fn fresh(&mut self) -> Option<Botguard> {
        log_error(Botguard::builder().init().await)
    }
}

fn log_error(made: Result<Botguard, rustypipe_botguard::Error>) -> Option<Botguard> {
    match made {
        Ok(botguard) => Some(botguard),
        Err(err) => {
            tracing::warn!(%err, "botguard challenge failed; uploaded songs and gated formats will not play");
            None
        }
    }
}

fn expiry(valid_until_unix: i64) -> Option<SystemTime> {
    let seconds = u64::try_from(valid_until_unix).ok()?;
    (UNIX_EPOCH + Duration::from_secs(seconds)).checked_sub(EARLY)
}

pub fn extractor_arg(token: &str) -> String {
    format!("youtube:po_token=web_music.gvs+{token}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_dropped_before_they_actually_expire() {
        let stated = UNIX_EPOCH + Duration::from_secs(1789420443);
        let expires = expiry(1789420443).expect("expiry");
        assert_eq!(stated.duration_since(expires).unwrap(), EARLY);
        assert!(expiry(-1).is_none());
    }

    #[test]
    fn the_argument_names_the_client_the_token_is_for() {
        assert_eq!(extractor_arg("abc"), "youtube:po_token=web_music.gvs+abc");
    }

    #[tokio::test]
    #[ignore]
    async fn an_upload_resolves() {
        use crate::net::stream::StreamResolver;
        let paths = Paths::discover();
        let client = crate::net::ytmusic::YtMusic::new(&paths).unwrap();
        let songs = crate::net::playlists::get_upload_songs(&client.api()).await.expect("upload songs");
        let song = songs.first().expect("at least one upload");
        println!("upload {} {:?}", song.video_id, song.title);
        let tokens = std::sync::Arc::new(PoTokens::new(&paths));
        let resolver = crate::net::stream::YtDlpResolver::new(&paths, tokens.clone());
        let info = resolver.resolve(song.video_id.clone(), client.media_auth()).await.expect("resolved");
        println!("format {:?} {:?}", info.format_id, info.acodec);
        assert!(info.uri.starts_with("https://"));
    }

    #[tokio::test]
    #[ignore]
    async fn mints_a_token() {
        let rss = || std::fs::read_to_string("/proc/self/status").ok().and_then(|s| s.lines().find(|l| l.starts_with("VmRSS")).map(str::to_owned)).unwrap_or_default();
        let dir = tempfile::tempdir().unwrap();
        let tokens = PoTokens::new(&Paths::for_tests(dir.path()));
        println!("before: {}", rss());
        let started = Instant::now();
        let first = tokens.for_video("dQw4w9WgXcQ").await.expect("token");
        println!("first token {} chars in {:?}", first.len(), started.elapsed());
        let started = Instant::now();
        let second = tokens.for_video("J7p4bzqLvCw").await.expect("token");
        println!("second token {} chars in {:?}", second.len(), started.elapsed());
        println!("after: {}", rss());
        assert!(started.elapsed() < Duration::from_secs(2), "the runtime is kept between tokens");
        assert_eq!(tokens.for_video("dQw4w9WgXcQ").await.as_deref(), Some(first.as_str()), "a minted token is reused");
        assert!(dir.path().join("cache/botguard_snapshot.bin").is_file(), "the first challenge is saved for the next start");
    }
}
