use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::model::{ItemKind, LikeStatus, MediaItem, Person, Track};
use crate::net::home::HomeSection;
use crate::net::search::SearchResults;
use crate::net::ytmusic::NetError;
use crate::paths::Paths;

const DEFAULT_SERVER: &str = "https://watchshark.duckdns.org";
const SERVER_PREF: &str = "watchshark_server";
const TIMEOUT: Duration = Duration::from_secs(15);

pub const TRACK_PREFIX: &str = "ws:";

pub fn server_pref(paths: &Paths) -> String {
    let raw = paths.read_prefs().get(SERVER_PREF).and_then(|v| v.as_str()).unwrap_or(DEFAULT_SERVER).trim().to_owned();
    normalize_server(&raw).unwrap_or_else(|| DEFAULT_SERVER.to_owned())
}

pub fn set_server_pref(paths: &Paths, server: &str) -> Result<String, String> {
    let clean = normalize_server(server).ok_or_else(|| "Enter a server URL like https://watchshark.duckdns.org".to_owned())?;
    paths.update_prefs(|p| {
        p.insert(SERVER_PREF.to_owned(), clean.clone().into());
    });
    Ok(clean)
}

fn normalize_server(raw: &str) -> Option<String> {
    let clean = raw.trim().trim_end_matches('/').to_owned();
    if clean.starts_with("http://") || clean.starts_with("https://") {
        if clean.contains('.') || clean.contains("localhost") {
            return Some(clean);
        }
    }
    None
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WsId {
    Track(i64),
    User(String),
}

pub fn is_watchshark_id(id: &str) -> bool {
    id.starts_with(TRACK_PREFIX)
}

pub fn parse_ws_id(id: &str) -> Option<WsId> {
    let rest = id.strip_prefix(TRACK_PREFIX)?;
    match rest.split_once(':') {
        Some(("user", name)) if !name.is_empty() => Some(WsId::User(name.to_owned())),
        Some(_) => None,
        None => rest.parse::<i64>().ok().map(WsId::Track),
    }
}

pub fn track_video_id(id: i64) -> String {
    format!("{TRACK_PREFIX}{id}")
}

pub fn user_item_id(username: &str) -> String {
    format!("{TRACK_PREFIX}user:{username}")
}

#[derive(Serialize, Deserialize)]
struct CredsFile {
    server: String,
    login: String,
    password: String,
}

fn creds_path(paths: &Paths) -> std::path::PathBuf {
    paths.data_dir.join("watchshark.json")
}

fn read_creds(paths: &Paths) -> Option<CredsFile> {
    std::fs::read_to_string(creds_path(paths)).ok().and_then(|t| serde_json::from_str(&t).ok()).filter(|c: &CredsFile| !c.login.is_empty())
}

fn write_creds(paths: &Paths, creds: Option<&CredsFile>) {
    let path = creds_path(paths);
    match creds {
        Some(creds) => {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(data) = serde_json::to_vec(creds) {
                let _ = std::fs::write(&path, data);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
                }
            }
        }
        None => {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub struct WatchShark {
    http: reqwest::Client,
    paths: Paths,
    server: RwLock<String>,
    username: RwLock<Option<String>>,
    session: RwLock<Option<String>>,
}

impl WatchShark {
    pub fn new(paths: &Paths) -> Arc<Self> {
        let http = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (X11; Linux x86_64; rv:147.0) Gecko/20100101 Firefox/147.0")
            .timeout(TIMEOUT)
            .pool_max_idle_per_host(4)
            .build()
            .unwrap_or_default();
        Arc::new(Self { http, paths: paths.clone(), server: RwLock::new(server_pref(paths)), username: RwLock::new(None), session: RwLock::new(None) })
    }

    pub async fn server(&self) -> String {
        self.server.read().await.clone()
    }

    pub fn try_read_server(&self) -> String {
        self.server.try_read().map(|s| s.clone()).unwrap_or_else(|_| DEFAULT_SERVER.to_owned())
    }

    pub fn try_username(&self) -> Option<String> {
        self.username.try_read().ok().and_then(|u| u.clone())
    }

    pub async fn set_server(&self, server: &str) -> Result<String, String> {
        let clean = set_server_pref(&self.paths, server)?;
        *self.server.write().await = clean.clone();
        *self.session.write().await = None;
        *self.username.write().await = None;
        Ok(clean)
    }

    pub fn has_account(&self) -> bool {
        read_creds(&self.paths).is_some()
    }

    pub async fn username(&self) -> Option<String> {
        self.username.read().await.clone()
    }

    async fn base(&self) -> String {
        self.server.read().await.clone()
    }

    async fn get(&self, path: &str, params: &[(&str, String)]) -> Result<serde_json::Value, NetError> {
        let base = self.base().await;
        let query: Vec<(String, String)> = params.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect();
        let response = self.http.get(format!("{base}{path}")).query(&query).send().await.map_err(NetError::Transport)?;
        check(response).await
    }

    async fn authed(&self) -> Result<String, NetError> {
        if let Some(token) = self.session.read().await.clone() {
            return Ok(token);
        }
        let Some(creds) = read_creds(&self.paths) else { return Err(NetError::Unauthenticated) };
        if creds.server != self.base().await {
            return Err(NetError::Unauthenticated);
        }
        let (name, token) = self.login_inner(&creds.login, &creds.password).await?;
        *self.username.write().await = Some(name);
        *self.session.write().await = Some(token.clone());
        Ok(token)
    }

    async fn login_inner(&self, login: &str, password: &str) -> Result<(String, String), NetError> {
        let base = self.base().await;
        let response = self
            .http
            .post(format!("{base}/api/auth/login"))
            .json(&serde_json::json!({"login": login, "password": password}))
            .send()
            .await
            .map_err(NetError::Transport)?;
        let token = response
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find_map(|c| c.split(';').next().and_then(|pair| pair.trim().strip_prefix("ws_token=")).map(str::to_owned))
            .filter(|t| !t.is_empty())
            .ok_or_else(|| NetError::Message("WatchShark login answered without a session".into()))?;
        let data = check(response).await?;
        let name = data.pointer("/user/username").and_then(|u| u.as_str()).filter(|u| !u.is_empty()).map(str::to_owned).ok_or_else(|| NetError::Message("WatchShark rejected the login".into()))?;
        Ok((name, token))
    }

    pub async fn login(&self, server: &str, login: &str, password: &str) -> Result<String, NetError> {
        let clean = normalize_server(server).ok_or_else(|| NetError::Message("Enter a server URL like https://watchshark.duckdns.org".into()))?;
        *self.server.write().await = clean.clone();
        let _ = set_server_pref(&self.paths, &clean);
        let (name, token) = self.login_inner(login, password).await?;
        write_creds(&self.paths, Some(&CredsFile { server: clean, login: login.trim().to_owned(), password: password.to_owned() }));
        *self.username.write().await = Some(name.clone());
        *self.session.write().await = Some(token);
        Ok(name)
    }

    pub async fn logout(&self) {
        if let Some(token) = self.session.read().await.clone() {
            let base = self.base().await;
            let _ = self.http.post(format!("{base}/api/auth/logout")).header(reqwest::header::COOKIE, format!("ws_token={token}")).send().await;
        }
        write_creds(&self.paths, None);
        *self.session.write().await = None;
        *self.username.write().await = None;
    }

    pub async fn me(&self) -> Result<Option<String>, NetError> {
        let data = self.get("/api/me", &[]).await?;
        Ok(data.get("user").and_then(|u| u.get("username")).and_then(|u| u.as_str()).filter(|u| !u.is_empty()).map(str::to_owned))
    }

    fn media_url(&self, base: &str, path: Option<&str>) -> Option<String> {
        let path = path.filter(|p| !p.is_empty())?;
        if path.starts_with("http") {
            return Some(path.to_owned());
        }
        Some(format!("{base}{path}"))
    }

    fn parse_video(&self, base: &str, v: &serde_json::Value) -> Option<Track> {
        let id = v.get("id")?.as_i64()?;
        if v.get("kind").and_then(|k| k.as_str()).is_some_and(|k| k != "music") {
            return None;
        }
        let title = v.get("title")?.as_str()?;
        if title.is_empty() {
            return None;
        }
        let artist = v.get("username").and_then(|u| u.as_str()).unwrap_or("Unknown artist").to_owned();
        let mut t = Track::default();
        t.video_id = crate::model::VideoId(track_video_id(id));
        t.title = title.to_owned();
        t.artist = artist.clone();
        t.artists = vec![Person { name: artist.clone(), id: Some(user_item_id(&artist)) }];
        t.thumb = self.media_url(base, v.get("thumbnail").and_then(|t| t.as_str()));
        t.like_status = if v.get("liked").and_then(|l| l.as_bool()).unwrap_or(false) { LikeStatus::Like } else { LikeStatus::Indifferent };
        t.is_available = v.get("status").and_then(|s| s.as_str()).is_none_or(|s| s == "ready");
        Some(t)
    }

    fn track_item(&self, base: &str, v: &serde_json::Value) -> Option<MediaItem> {
        let t = self.parse_video(base, v)?;
        let plays = v.get("views").and_then(|n| n.as_u64()).map(crate::model::compact_count);
        Some(MediaItem {
            kind: ItemKind::Song,
            id: t.video_id.0.clone(),
            title: t.title.clone(),
            artists: t.artists.clone(),
            thumb: t.thumb.clone(),
            views: plays,
            like_status: Some(t.like_status),
            ..MediaItem::default()
        })
    }

    pub async fn search(&self, query: &str) -> Result<SearchResults, NetError> {
        let base = self.base().await;
        let response = self.get("/api/videos", &[("kind", "music".into()), ("q", query.to_owned()), ("limit", "24".into())]).await?;
        let items: Vec<MediaItem> = response.get("videos").and_then(|v| v.as_array()).map(|v| v.iter().filter_map(|e| self.track_item(&base, e)).collect()).unwrap_or_default();
        if items.is_empty() {
            return Err(NetError::Message("No WatchShark results".into()));
        }
        let top = items.first().cloned();
        Ok(SearchResults { top_result: top, items })
    }

    pub async fn home_sections(&self) -> Result<Vec<HomeSection>, NetError> {
        let base = self.base().await;
        let popular = self.get("/api/videos", &[("kind", "music".into()), ("sort", "popular".into()), ("limit", "20".into())]).await;
        let recent = self.get("/api/videos", &[("kind", "music".into()), ("limit", "20".into())]).await;
        let mut sections = Vec::new();
        if let Ok(response) = popular {
            let items: Vec<MediaItem> = response.get("videos").and_then(|v| v.as_array()).map(|v| v.iter().filter_map(|e| self.track_item(&base, e)).collect()).unwrap_or_default();
            if !items.is_empty() {
                sections.push(HomeSection { title: "Popular".to_owned(), items, strapline_thumb: None, strapline: Some("WatchShark".into()) });
            }
        }
        if let Ok(response) = recent {
            let items: Vec<MediaItem> = response.get("videos").and_then(|v| v.as_array()).map(|v| v.iter().filter_map(|e| self.track_item(&base, e)).collect()).unwrap_or_default();
            if !items.is_empty() {
                sections.push(HomeSection { title: "Fresh uploads".to_owned(), items, strapline_thumb: None, strapline: Some("WatchShark".into()) });
            }
        }
        if sections.is_empty() {
            return Err(NetError::Message("WatchShark has no music yet".into()));
        }
        Ok(sections)
    }

    pub async fn video(&self, id: i64) -> Result<Track, NetError> {
        let base = self.base().await;
        let response = self.get(&format!("/api/videos/{id}"), &[]).await?;
        response.get("video").and_then(|v| self.parse_video(&base, v)).ok_or_else(|| NetError::Message("WatchShark video unavailable".into()))
    }

    pub async fn stream_url(&self, id: i64) -> Result<(String, Option<String>), NetError> {
        let base = self.base().await;
        let response = self.get(&format!("/api/videos/{id}"), &[]).await?;
        let video = response.get("video").ok_or_else(|| NetError::Message("WatchShark video unavailable".into()))?;
        let src = video.get("src").and_then(|s| s.as_str()).filter(|s| !s.is_empty()).ok_or_else(|| NetError::Message("WatchShark video has no file".into()))?;
        let ext = video.get("mimetype").and_then(|m| m.as_str()).and_then(ext_of_mime).map(str::to_owned);
        Ok((format!("{base}{src}"), ext))
    }

    pub async fn channel_tracks(&self, username: &str) -> Result<(String, Vec<Track>), NetError> {
        let base = self.base().await;
        let response = self.get(&format!("/api/channel/{username}"), &[]).await?;
        let name = response.pointer("/user/username").and_then(|u| u.as_str()).unwrap_or(username).to_owned();
        let tracks = response
            .get("videos")
            .and_then(|v| v.as_array())
            .map(|v| v.iter().filter_map(|e| self.parse_video(&base, e)).collect())
            .unwrap_or_default();
        Ok((name, tracks))
    }

    pub async fn related(&self, track: &Track) -> Result<Vec<Track>, NetError> {
        let mut out: Vec<Track> = Vec::new();
        if let Some(artist) = track.artists.first().and_then(|a| a.id.clone()).and_then(|id| parse_ws_id(&id)).and_then(|parsed| match parsed {
            WsId::User(name) => Some(name),
            _ => None,
        }) {
            if let Ok((_, mut mine)) = self.channel_tracks(&artist).await {
                mine.retain(|t| t.video_id != track.video_id);
                out.append(&mut mine);
            }
        }
        if out.len() < 10 {
            let base = self.base().await;
            if let Ok(response) = self.get("/api/videos", &[("kind", "music".into()), ("sort", "popular".into()), ("limit", "20".into())]).await {
                let mut popular: Vec<Track> = response.get("videos").and_then(|v| v.as_array()).map(|v| v.iter().filter_map(|e| self.parse_video(&base, e)).collect()).unwrap_or_default();
                popular.retain(|t| t.video_id != track.video_id && !out.iter().any(|o| o.video_id == t.video_id));
                out.append(&mut popular);
            }
        }
        out.truncate(25);
        Ok(out)
    }

    pub async fn set_like(&self, id: i64, like: bool) -> Result<(), NetError> {
        let base = self.base().await;
        let token = self.authed().await?;
        let cookie = format!("ws_token={token}");
        let current: bool = self
            .http
            .get(format!("{base}/api/videos/{id}"))
            .header(reqwest::header::COOKIE, &cookie)
            .send()
            .await
            .map_err(NetError::Transport)?
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|v| v.pointer("/video/liked").and_then(|l| l.as_bool()))
            .unwrap_or(false);
        if current == like {
            return Ok(());
        }
        let response = self.http.post(format!("{base}/api/videos/{id}/like")).header(reqwest::header::COOKIE, cookie).send().await.map_err(NetError::Transport)?;
        if response.status().as_u16() == 401 {
            *self.session.write().await = None;
            return Err(NetError::Unauthenticated);
        }
        check(response).await.map(|_| ())
    }
}

fn ext_of_mime(mime: &str) -> Option<&str> {
    if mime.contains("ogg") || mime.contains("opus") {
        Some("ogg")
    } else if mime.contains("mpeg") || mime.contains("mp3") {
        Some("mp3")
    } else if mime.contains("mp4") || mime.contains("m4a") {
        Some("m4a")
    } else if mime.contains("wav") || mime.contains("wave") {
        Some("wav")
    } else if mime.contains("webm") {
        Some("webm")
    } else {
        None
    }
}

async fn check(response: reqwest::Response) -> Result<serde_json::Value, NetError> {
    let status = response.status().as_u16();
    if status == 401 || status == 403 {
        return Err(NetError::Unauthenticated);
    }
    if status == 429 || status >= 500 {
        return Err(NetError::Http { status, message: "WatchShark is rate limiting".into() });
    }
    let data: serde_json::Value = response.json().await.map_err(|_| NetError::Http { status, message: "Unreadable WatchShark response".into() })?;
    if status >= 400 {
        return Err(NetError::Message(data.get("error").and_then(|e| e.as_str()).unwrap_or("WatchShark request failed").to_owned()));
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video_json() -> serde_json::Value {
        serde_json::json!({
            "id": 29, "title": "Music straight from hell", "description": "d",
            "username": "matko802", "user_id": 1,
            "src": "/v/abc.ogg", "thumbnail": "/t/abc.jpg",
            "mimetype": "audio/ogg", "size": 100, "views": 1500,
            "likes": 3, "liked": true, "comments": 0,
            "status": "ready", "avatar": "/a/x.jpg", "kind": "music"
        })
    }

    fn client(dir: &tempfile::TempDir) -> (Paths, Arc<WatchShark>) {
        let paths = Paths::for_tests(dir.path());
        let ws = WatchShark::new(&paths);
        (paths, ws)
    }

    #[test]
    fn a_video_maps_to_the_queue_model() {
        let dir = tempfile::tempdir().unwrap();
        let (_, ws) = client(&dir);
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let base = rt.block_on(ws.base());
        let t = ws.parse_video(&base, &video_json()).unwrap();
        assert_eq!(t.video_id.as_str(), "ws:29");
        assert!(is_watchshark_id("ws:29"));
        assert_eq!(t.title, "Music straight from hell");
        assert_eq!(t.artist, "matko802");
        assert_eq!(t.like_status, LikeStatus::Like);
        assert!(t.thumb.unwrap().ends_with("/t/abc.jpg"));
        assert!(t.is_available);
    }

    #[test]
    fn non_music_kinds_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let (_, ws) = client(&dir);
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let base = rt.block_on(ws.base());
        let mut v = video_json();
        v["kind"] = serde_json::json!("video");
        assert!(ws.parse_video(&base, &v).is_none());
    }

    #[test]
    fn ids_split_into_track_and_user() {
        assert_eq!(parse_ws_id("ws:29"), Some(WsId::Track(29)));
        assert_eq!(parse_ws_id("ws:user:matko802"), Some(WsId::User("matko802".to_owned())));
        assert_eq!(parse_ws_id("sc:123"), None);
        assert!(!is_watchshark_id("VLOLAK"));
    }

    #[test]
    fn server_urls_are_normalized() {
        assert_eq!(normalize_server("https://watchshark.duckdns.org/"), Some("https://watchshark.duckdns.org".to_owned()));
        assert_eq!(normalize_server("http://localhost:3000"), Some("http://localhost:3000".to_owned()));
        assert_eq!(normalize_server("not a url"), None);
        assert_eq!(normalize_server("watchshark.duckdns.org"), None);
    }

    #[test]
    fn mime_types_map_to_extensions() {
        assert_eq!(ext_of_mime("audio/ogg"), Some("ogg"));
        assert_eq!(ext_of_mime("audio/mpeg"), Some("mp3"));
        assert_eq!(ext_of_mime("video/mp4"), Some("m4a"));
        assert_eq!(ext_of_mime("application/octet-stream"), None);
    }
}
