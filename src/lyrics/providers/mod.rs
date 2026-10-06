
pub mod apple;
pub mod betterlyrics;
pub mod binilyrics;
pub mod lrclib;
pub mod netease;
pub mod ytm;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::model::{LyricsMatch, LyricsResult, RANK_LINE, RANK_PLAIN, RANK_WORD};
use crate::net::ytmusic::YtMusic;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Provider {
    AppleMusic,
    BetterLyrics,
    BiniLyrics,
    NetEase,
    Lrclib,
    YouTubeMusic,
}

impl Provider {
    pub const ALL: [Provider; 6] = [Provider::AppleMusic, Provider::BetterLyrics, Provider::BiniLyrics, Provider::NetEase, Provider::Lrclib, Provider::YouTubeMusic];

    pub const MATCH_BROWSERS: [Provider; 3] = [Provider::AppleMusic, Provider::NetEase, Provider::Lrclib];

    pub const ROMANIZATION_SOURCES: [Provider; 2] = [Provider::NetEase, Provider::AppleMusic];

    pub fn name(self) -> &'static str {
        match self {
            Provider::AppleMusic => "Apple Music",
            Provider::BetterLyrics => "BetterLyrics",
            Provider::BiniLyrics => "BiniLyrics",
            Provider::NetEase => "NetEase",
            Provider::Lrclib => "LRCLIB",
            Provider::YouTubeMusic => "YouTube Music",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.name() == name)
    }

    pub fn native_rank(self) -> u8 {
        match self {
            Provider::AppleMusic | Provider::BetterLyrics | Provider::BiniLyrics => RANK_WORD,
            Provider::NetEase | Provider::Lrclib => RANK_LINE,
            Provider::YouTubeMusic => RANK_PLAIN,
        }
    }

    pub fn takes_video_id_only(self) -> bool {
        self == Provider::YouTubeMusic
    }

    pub fn supports_matches(self) -> bool {
        Self::MATCH_BROWSERS.contains(&self)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Request<'a> {
    pub video_id: &'a str,
    pub title: &'a str,
    pub artist: &'a str,
    pub duration: u32,
    pub strict: bool,
}

pub type Fetched<'a> = Pin<Box<dyn Future<Output = Option<LyricsResult>> + Send + 'a>>;
pub type Matched<'a> = Pin<Box<dyn Future<Output = Vec<LyricsMatch>> + Send + 'a>>;

pub trait Source: Send + Sync {
    fn fetch<'a>(&'a self, provider: Provider, request: Request<'a>) -> Fetched<'a>;

    fn matches<'a>(&'a self, provider: Provider, request: Request<'a>, limit: usize) -> Matched<'a>;
}

#[derive(Default)]
pub struct Cooldowns {
    until: Mutex<HashMap<Provider, Instant>>,
}

impl Cooldowns {
    pub fn ready(&self, provider: Provider) -> bool {
        self.ready_at(provider, Instant::now())
    }

    pub fn trip(&self, provider: Provider, window: Duration, reason: &str) {
        self.trip_at(provider, Instant::now(), window, reason);
    }

    fn ready_at(&self, provider: Provider, now: Instant) -> bool {
        self.until.lock().unwrap().get(&provider).is_none_or(|deadline| now >= *deadline)
    }

    fn trip_at(&self, provider: Provider, now: Instant, window: Duration, reason: &str) {
        let deadline = now + window;
        let mut until = self.until.lock().unwrap();
        if until.get(&provider).is_none_or(|current| deadline > *current) {
            until.insert(provider, deadline);
            tracing::info!(provider = provider.name(), seconds = window.as_secs(), reason, "lyrics provider backing off");
        }
    }
}

#[derive(Debug)]
pub enum HttpError {
    Status(u16),
    Timeout,
    Other(String),
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpError::Status(code) => write!(f, "HTTP {code}"),
            HttpError::Timeout => f.write_str("timed out"),
            HttpError::Other(message) => f.write_str(message),
        }
    }
}

impl From<reqwest::Error> for HttpError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() { HttpError::Timeout } else { HttpError::Other(err.to_string()) }
    }
}

pub async fn get_text(http: &reqwest::Client, url: &str, query: &[(&str, &str)], headers: &[(&str, &str)], timeout: Duration) -> Result<String, HttpError> {
    let url = reqwest::Url::parse_with_params(url, query).map_err(|e| HttpError::Other(e.to_string()))?;
    let mut request = http.get(url).timeout(timeout);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = request.send().await?;
    let status = response.status();
    if !status.is_success() {
        return Err(HttpError::Status(status.as_u16()));
    }
    Ok(String::from_utf8_lossy(&response.bytes().await?).into_owned())
}

pub async fn get_json(http: &reqwest::Client, url: &str, query: &[(&str, &str)], headers: &[(&str, &str)], timeout: Duration) -> Result<Value, HttpError> {
    let body = get_text(http, url, query, headers, timeout).await?;
    serde_json::from_str(&body).map_err(|e| HttpError::Other(e.to_string()))
}

pub fn text_at<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

pub fn seconds_at(value: &Value, key: &str) -> u32 {
    value.get(key).and_then(Value::as_f64).filter(|v| *v > 0.0).map_or(0, |v| v as u32)
}

pub fn title_and_artist(title: &str, artist: &str) -> String {
    let title = title.trim();
    let artist = artist.trim();
    if artist.is_empty() { title.to_owned() } else { format!("{title} {artist}") }
}

pub fn variants_or_title(title: &str) -> Vec<String> {
    let variants = crate::lyrics::matching::title_variants(title);
    if variants.is_empty() { vec![title.to_owned()] } else { variants }
}

pub fn label_or_unknown(name: &str) -> String {
    if name.is_empty() { "Unknown".to_owned() } else { name.to_owned() }
}

pub struct Live {
    http: reqwest::Client,
    ytm: Arc<YtMusic>,
    cooldowns: Arc<Cooldowns>,
    apple: apple::AppleMusic,
}

impl Live {
    pub fn new(http: reqwest::Client, ytm: Arc<YtMusic>, cooldowns: Arc<Cooldowns>) -> Self {
        Self { apple: apple::AppleMusic::new(http.clone()), http, ytm, cooldowns }
    }
}

impl Source for Live {
    fn fetch<'a>(&'a self, provider: Provider, request: Request<'a>) -> Fetched<'a> {
        Box::pin(async move {
            match provider {
                Provider::AppleMusic => self.apple.fetch(request).await,
                Provider::BetterLyrics => betterlyrics::fetch(&self.http, request).await,
                Provider::BiniLyrics => binilyrics::fetch(&self.http, request).await,
                Provider::NetEase => netease::fetch(&self.http, request).await,
                Provider::Lrclib => lrclib::fetch(&self.http, &self.cooldowns, request).await,
                Provider::YouTubeMusic => {
                    let web = self.ytm.api();
                    let mobile = ytm::MobileClient::new(self.http.clone());
                    ytm::fetch(&web, &mobile, request.video_id).await
                }
            }
        })
    }

    fn matches<'a>(&'a self, provider: Provider, request: Request<'a>, limit: usize) -> Matched<'a> {
        Box::pin(async move {
            match provider {
                Provider::AppleMusic => self.apple.matches(request, limit).await,
                Provider::NetEase => netease::matches(&self.http, request, limit).await,
                Provider::Lrclib => lrclib::matches(&self.http, request, limit).await,
                _ => Vec::new(),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::prefs::DEFAULT_PROVIDER_ORDER;

    #[test]
    fn the_catalog_and_the_default_queue_name_the_same_providers() {
        let names: Vec<&str> = Provider::ALL.iter().map(|p| p.name()).collect();
        assert_eq!(names, DEFAULT_PROVIDER_ORDER);
        for provider in Provider::ALL {
            assert_eq!(Provider::from_name(provider.name()), Some(provider));
        }
        assert_eq!(Provider::from_name("Musixmatch"), None);
    }

    #[test]
    fn capabilities() {
        assert_eq!(Provider::AppleMusic.native_rank(), 3);
        assert_eq!(Provider::Lrclib.native_rank(), 2);
        assert_eq!(Provider::YouTubeMusic.native_rank(), 1);
        assert!(Provider::YouTubeMusic.takes_video_id_only());
        assert!(!Provider::NetEase.takes_video_id_only());
        assert!(Provider::Lrclib.supports_matches());
        assert!(!Provider::BiniLyrics.supports_matches());
    }

    #[test]
    fn a_cooldown_ends_and_never_shortens() {
        let cooldowns = Cooldowns::default();
        let now = Instant::now();
        assert!(cooldowns.ready_at(Provider::Lrclib, now));
        cooldowns.trip_at(Provider::Lrclib, now, Duration::from_secs(120), "HTTP 429");
        assert!(!cooldowns.ready_at(Provider::Lrclib, now + Duration::from_secs(119)));
        assert!(cooldowns.ready_at(Provider::NetEase, now), "one provider's window is its own");
        cooldowns.trip_at(Provider::Lrclib, now + Duration::from_secs(10), Duration::from_secs(60), "timeout");
        assert!(!cooldowns.ready_at(Provider::Lrclib, now + Duration::from_secs(100)));
        assert!(cooldowns.ready_at(Provider::Lrclib, now + Duration::from_secs(120)));
        cooldowns.trip_at(Provider::Lrclib, now + Duration::from_secs(100), Duration::from_secs(120), "HTTP 429");
        assert!(!cooldowns.ready_at(Provider::Lrclib, now + Duration::from_secs(200)));
    }

    #[test]
    fn json_helpers_tolerate_what_catalogs_send() {
        let value = serde_json::json!({"a": 233.7, "b": null, "c": "x", "d": -4, "e": 12});
        assert_eq!(seconds_at(&value, "a"), 233);
        assert_eq!(seconds_at(&value, "b"), 0);
        assert_eq!(seconds_at(&value, "c"), 0);
        assert_eq!(seconds_at(&value, "d"), 0);
        assert_eq!(seconds_at(&value, "e"), 12);
        assert_eq!(text_at(&value, "c"), "x");
        assert_eq!(text_at(&value, "a"), "");
        assert_eq!(title_and_artist(" Lemon ", " Kenshi Yonezu "), "Lemon Kenshi Yonezu");
        assert_eq!(title_and_artist("Lemon", ""), "Lemon");
    }
}
