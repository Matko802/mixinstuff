use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::model::{ItemKind, MediaItem, Person, Track};
use crate::net::home::HomeSection;
use crate::net::search::SearchResults;
use crate::net::ytmusic::NetError;
use crate::paths::Paths;

const API_V2: &str = "https://api-v2.soundcloud.com";
const WEB: &str = "https://soundcloud.com";
const UA: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:147.0) Gecko/20100101 Firefox/147.0";
const TIMEOUT: Duration = Duration::from_secs(15);
const CLIENT_ID_TTL: Duration = Duration::from_secs(30 * 24 * 3600);

pub const TRACK_PREFIX: &str = "sc:";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScId {
    Track(u64),
    Playlist(u64),
    User(u64),
}

pub fn is_soundcloud_id(id: &str) -> bool {
    id.starts_with(TRACK_PREFIX)
}

pub fn parse_sc_id(id: &str) -> Option<ScId> {
    let rest = id.strip_prefix(TRACK_PREFIX)?;
    let (kind, num) = match rest.split_once(':') {
        Some((k, n)) => (Some(k), n),
        None => (None, rest),
    };
    let num: u64 = num.parse().ok()?;
    match kind {
        None => Some(ScId::Track(num)),
        Some("playlist") => Some(ScId::Playlist(num)),
        Some("user") => Some(ScId::User(num)),
        _ => None,
    }
}

pub fn track_video_id(id: u64) -> String {
    format!("{TRACK_PREFIX}{id}")
}

#[derive(Serialize, Deserialize)]
struct TokenFile {
    token: String,
}

#[derive(Serialize, Deserialize)]
struct ClientIdFile {
    client_id: String,
    fetched_at: u64,
}

fn token_path(paths: &Paths) -> std::path::PathBuf {
    paths.data_dir.join("soundcloud.json")
}

fn client_id_path(paths: &Paths) -> std::path::PathBuf {
    paths.cache_dir.join("soundcloud_client_id.json")
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub struct SoundCloud {
    http: reqwest::Client,
    paths: Paths,
    client_id: RwLock<Option<(String, u64)>>,
    token: RwLock<Option<String>>,
}

impl SoundCloud {
    pub fn new(paths: &Paths) -> Arc<Self> {
        let http = reqwest::Client::builder().user_agent(UA).timeout(TIMEOUT).pool_max_idle_per_host(4).build().unwrap_or_default();
        let token = std::fs::read_to_string(token_path(paths))
            .ok()
            .and_then(|t| serde_json::from_str::<TokenFile>(&t).ok())
            .map(|f| f.token)
            .filter(|t| !t.trim().is_empty());
        let cached = std::fs::read_to_string(client_id_path(paths))
            .ok()
            .and_then(|t| serde_json::from_str::<ClientIdFile>(&t).ok())
            .map(|f| (f.client_id, f.fetched_at));
        Arc::new(Self { http, paths: paths.clone(), client_id: RwLock::new(cached), token: RwLock::new(token) })
    }

    pub fn has_token(&self) -> bool {
        self.token.try_read().map(|t| t.is_some()).unwrap_or(false)
    }

    pub async fn set_token(&self, token: Option<String>) {
        let clean = token.map(|t| t.trim().trim_start_matches("OAuth ").trim_start_matches("oauth ").to_owned()).filter(|t| !t.is_empty());
        *self.token.write().await = clean.clone();
        let path = token_path(&self.paths);
        match clean {
            Some(token) => {
                let data = serde_json::to_vec(&TokenFile { token }).unwrap_or_default();
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(path, data);
            }
            None => {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    async fn auth_header(&self) -> Option<String> {
        self.token.read().await.clone().map(|t| format!("OAuth {t}"))
    }

    async fn client_id(&self) -> Result<String, NetError> {
        if let Some((id, at)) = self.client_id.read().await.clone() {
            if now_secs().saturating_sub(at) < CLIENT_ID_TTL.as_secs() {
                return Ok(id);
            }
        }
        let id = self.scrape_client_id().await?;
        let path = client_id_path(&self.paths);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, serde_json::to_vec(&ClientIdFile { client_id: id.clone(), fetched_at: now_secs() }).unwrap_or_default());
        *self.client_id.write().await = Some((id.clone(), now_secs()));
        Ok(id)
    }

    async fn scrape_client_id(&self) -> Result<String, NetError> {
        let page = self.http.get(WEB).send().await.map_err(|e| NetError::Transport(e))?.text().await.map_err(|e| NetError::Transport(e))?;
        let mut assets = Vec::new();
        let mut rest = page.as_str();
        while let Some(start) = rest.find("<script") {
            rest = &rest[start..];
            let Some(src) = rest.split_once("src=\"").map(|(_, after)| after) else { break };
            let Some((url, _)) = src.split_once('"') else { break };
            if url.starts_with("http") {
                assets.push(url.to_owned());
            }
            rest = src;
        }
        for url in assets.iter().rev() {
            let Ok(js) = self.http.get(url).send().await else { continue };
            let Ok(js) = js.text().await else { continue };
            if let Some(id) = find_client_id(&js) {
                return Ok(id);
            }
        }
        Err(NetError::Message("SoundCloud did not expose a client id".into()))
    }

    async fn get(&self, path: &str, params: &[(&str, String)]) -> Result<serde_json::Value, NetError> {
        let id = self.client_id().await?;
        let mut query: Vec<(String, String)> = params.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect();
        query.push(("client_id".into(), id));
        let mut request = self.http.get(format!("{API_V2}/{path}")).query(&query);
        if let Some(auth) = self.auth_header().await {
            request = request.header("Authorization", auth);
        }
        let response = request.send().await.map_err(|e| NetError::Transport(e))?;
        let status = response.status().as_u16();
        if status == 401 || status == 403 {
            *self.client_id.write().await = None;
            let _ = std::fs::remove_file(client_id_path(&self.paths));
            let id = self.client_id().await?;
            let mut query: Vec<(String, String)> = params.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect();
            query.push(("client_id".into(), id));
            let mut retry = self.http.get(format!("{API_V2}/{path}")).query(&query);
            if let Some(auth) = self.auth_header().await {
                retry = retry.header("Authorization", auth);
            }
            let response = retry.send().await.map_err(|e| NetError::Transport(e))?;
            return check(response).await;
        }
        check(response).await
    }

    async fn post_like(&self, path: &str, like: bool) -> Result<(), NetError> {
        let Some(auth) = self.auth_header().await else { return Err(NetError::Unauthenticated) };
        let id = self.client_id().await?;
        let request = if like { self.http.post(format!("https://api.soundcloud.com{path}")) } else { self.http.delete(format!("https://api.soundcloud.com{path}")) };
        let response = request.query(&[("client_id", id)]).header("Authorization", auth).send().await.map_err(|e| NetError::Transport(e))?;
        check(response).await.map(|_| ())
    }

    pub async fn me(&self) -> Result<String, NetError> {
        if self.auth_header().await.is_none() {
            return Err(NetError::Unauthenticated);
        }
        let user = self.get("me", &[]).await?;
        user.get("username").and_then(|u| u.as_str()).filter(|u| !u.is_empty()).map(str::to_owned).ok_or_else(|| NetError::Message("SoundCloud rejected the token".into()))
    }

    pub async fn search(&self, query: &str) -> Result<SearchResults, NetError> {
        let response = self.get("search", &[("q", query.to_owned()), ("facet", "model".into()), ("limit", "25".into()), ("linked_partitioning", "1".into())]).await?;
        let items: Vec<MediaItem> = response
            .get("collection")
            .and_then(|c| c.as_array())
            .map(|c| c.iter().filter_map(parse_search_item).collect())
            .unwrap_or_default();
        if items.is_empty() {
            return Err(NetError::Message("No SoundCloud results".into()));
        }
        let top = items.first().cloned();
        Ok(SearchResults { top_result: top, items })
    }

    pub async fn charts(&self) -> Result<Vec<HomeSection>, NetError> {
        const GENRES: [(&str, &str); 4] = [("Top tracks", "all-music"), ("Hip-hop", "hiphop"), ("Electronic", "electronic"), ("Pop", "pop")];
        let mut sections = Vec::new();
        for (title, genre) in GENRES {
            let response = self.get("charts", &[("kind", "top".into()), ("genre", format!("soundcloud:genres:{genre}")), ("limit", "20".into())]).await;
            let Ok(response) = response else { continue };
            let items: Vec<MediaItem> = response
                .get("collection")
                .and_then(|c| c.as_array())
                .map(|c| c.iter().filter_map(|e| e.get("track")).filter_map(track_item).collect())
                .unwrap_or_default();
            if !items.is_empty() {
                sections.push(HomeSection { title: title.to_owned(), items, strapline_thumb: None, strapline: Some("SoundCloud charts".into()) });
            }
        }
        if sections.is_empty() {
            return Err(NetError::Message("SoundCloud charts unavailable".into()));
        }
        Ok(sections)
    }

    pub async fn playlist(&self, id: u64) -> Result<(String, Vec<Track>), NetError> {
        let list = self.get(&format!("playlists/{id}"), &[]).await?;
        let title = list.get("title").and_then(|t| t.as_str()).unwrap_or("Playlist").to_owned();
        let tracks = list.get("tracks").and_then(|t| t.as_array()).map(|t| t.iter().filter_map(parse_track).collect()).unwrap_or_default();
        Ok((title, tracks))
    }

    pub async fn user_tracks(&self, id: u64) -> Result<(String, Vec<Track>), NetError> {
        let user = self.get(&format!("users/{id}"), &[]).await?;
        let name = user.get("username").and_then(|u| u.as_str()).unwrap_or("Artist").to_owned();
        let response = self.get(&format!("users/{id}/tracks"), &[("limit", "50".into()), ("linked_partitioning", "1".into())]).await?;
        let tracks = response.get("collection").and_then(|c| c.as_array()).map(|c| c.iter().filter_map(parse_track).collect()).unwrap_or_default();
        Ok((name, tracks))
    }

    pub async fn user_playlists(&self, id: u64) -> Result<Vec<MediaItem>, NetError> {
        let response = self.get(&format!("users/{id}/playlists"), &[("limit", "25".into()), ("linked_partitioning", "1".into())]).await?;
        Ok(response.get("collection").and_then(|c| c.as_array()).map(|c| c.iter().filter_map(playlist_item).collect()).unwrap_or_default())
    }

    pub async fn related(&self, id: u64) -> Result<Vec<Track>, NetError> {
        let response = self.get(&format!("tracks/{id}/related"), &[("limit", "25".into())]).await?;
        Ok(response.get("collection").and_then(|c| c.as_array()).map(|c| c.iter().filter_map(parse_track).collect()).unwrap_or_default())
    }

    pub async fn track_permalink(&self, id: u64) -> Result<String, NetError> {
        let track = self.get(&format!("tracks/{id}"), &[]).await?;
        track.get("permalink_url").and_then(|u| u.as_str()).filter(|u| !u.is_empty()).map(str::to_owned).ok_or_else(|| NetError::Message("Track has no share URL".into()))
    }

    pub async fn like(&self, id: u64) -> Result<(), NetError> {
        self.post_like(&format!("/likes/tracks/{id}"), true).await
    }

    pub async fn unlike(&self, id: u64) -> Result<(), NetError> {
        self.post_like(&format!("/likes/tracks/{id}"), false).await
    }

    pub async fn resolve(&self, url: &str) -> Result<Resolved, NetError> {
        let response = self.get("resolve", &[("url", url.to_owned())]).await?;
        let kind = response.get("kind").and_then(|k| k.as_str()).unwrap_or_default();
        match kind {
            "track" => parse_track(&response).map(Resolved::Track).ok_or_else(|| NetError::Message("Unplayable SoundCloud link".into())),
            "playlist" => {
                let id = response.get("id").and_then(|i| i.as_u64()).ok_or_else(|| NetError::Message("Unplayable SoundCloud link".into()))?;
                let title = response.get("title").and_then(|t| t.as_str()).unwrap_or("Playlist").to_owned();
                Ok(Resolved::Playlist(format!("sc:playlist:{id}"), title))
            }
            "user" => {
                let id = response.get("id").and_then(|i| i.as_u64()).ok_or_else(|| NetError::Message("Unplayable SoundCloud link".into()))?;
                let name = response.get("username").and_then(|u| u.as_str()).unwrap_or("Artist").to_owned();
                Ok(Resolved::Artist(format!("sc:user:{id}"), name))
            }
            _ => Err(NetError::Message("Not a SoundCloud track, set or artist".into())),
        }
    }
}

pub enum Resolved {
    Track(Track),
    Playlist(String, String),
    Artist(String, String),
}

async fn check(response: reqwest::Response) -> Result<serde_json::Value, NetError> {
    let status = response.status().as_u16();
    if status == 401 || status == 403 {
        return Err(NetError::Unauthenticated);
    }
    if status == 429 || status >= 500 {
        return Err(NetError::Http { status, message: "SoundCloud is rate limiting".into() });
    }
    let data: serde_json::Value = response.json().await.map_err(|_| NetError::Http { status, message: "Unreadable SoundCloud response".into() })?;
    if status >= 400 {
        return Err(NetError::Message(data.get("message").and_then(|m| m.as_str()).unwrap_or("SoundCloud request failed").to_owned()));
    }
    Ok(data)
}

fn find_client_id(js: &str) -> Option<String> {
    let mut rest = js;
    while let Some(pos) = rest.find("client_id") {
        rest = &rest[pos + 9..];
        let rest = rest.trim_start_matches(|c| c == ' ' || c == '\t' || c == ':');
        let rest = rest.strip_prefix('"').unwrap_or(rest);
        if rest.len() >= 32 && rest.chars().take(32).all(|c| c.is_ascii_alphanumeric()) {
            let id: String = rest.chars().take(32).collect();
            if rest.chars().nth(32) == Some('"') {
                return Some(id);
            }
        }
    }
    None
}

fn user_of(track: &serde_json::Value) -> (String, Option<u64>) {
    let user = track.get("user");
    let name = user.and_then(|u| u.get("username")).and_then(|u| u.as_str()).unwrap_or("Unknown artist").to_owned();
    let id = user.and_then(|u| u.get("id")).and_then(|u| u.as_u64());
    (name, id)
}

fn artwork(track: &serde_json::Value) -> Option<String> {
    let url = track.get("artwork_url").and_then(|u| u.as_str()).filter(|u| !u.is_empty()).map(str::to_owned).or_else(|| {
        track.get("user").and_then(|u| u.get("avatar_url")).and_then(|u| u.as_str()).filter(|u| !u.is_empty()).map(str::to_owned)
    })?;
    Some(sized_artwork(&url))
}

fn sized_artwork(url: &str) -> String {
    for size in ["-large.", "-t67x67.", "-badge.", "-tiny.", "-small.", "-t300x300."] {
        if url.contains(size) {
            return url.replacen(size, "-t500x500.", 1);
        }
    }
    if let Some(sized) = grow_variant(url) {
        return sized;
    }
    url.to_owned()
}

fn grow_variant(url: &str) -> Option<String> {
    let start = url.find("-t")? + 2;
    let mid = url[start..].find('x')? + start;
    let end = url[mid + 1..].find('.')? + mid + 1;
    if url[start..mid].chars().all(|c| c.is_ascii_digit()) && url[mid + 1..end].chars().all(|c| c.is_ascii_digit()) {
        Some(format!("{}-t500x500.{}", &url[..start - 2], &url[end + 1..]))
    } else {
        None
    }
}

fn duration_secs(track: &serde_json::Value) -> Option<u32> {
    track.get("full_duration").or_else(|| track.get("duration")).and_then(|d| d.as_u64()).map(|ms| (ms / 1000).max(1) as u32)
}

fn playable(track: &serde_json::Value) -> bool {
    track.get("streamable").and_then(|s| s.as_bool()).unwrap_or(true) && track.get("policy").and_then(|p| p.as_str()).is_none_or(|p| p != "BLOCK")
}

pub fn parse_track(track: &serde_json::Value) -> Option<Track> {
    let id = track.get("id")?.as_u64()?;
    let title = track.get("title")?.as_str()?;
    if title.is_empty() {
        return None;
    }
    let (artist, user_id) = user_of(track);
    let mut t = Track::default();
    t.video_id = crate::model::VideoId(track_video_id(id));
    t.title = title.to_owned();
    t.artist = artist.clone();
    t.artists = vec![Person { name: artist, id: user_id.map(|u| format!("sc:user:{u}")) }];
    t.thumb = artwork(track);
    t.duration_seconds = duration_secs(track);
    t.is_available = playable(track);
    Some(t)
}

fn track_item(track: &serde_json::Value) -> Option<MediaItem> {
    let t = parse_track(track)?;
    let plays = track.get("playback_count").and_then(|p| p.as_u64()).map(|p| crate::model::compact_count(p));
    Some(MediaItem {
        kind: ItemKind::Song,
        id: t.video_id.0.clone(),
        title: t.title.clone(),
        artists: t.artists.clone(),
        thumb: t.thumb.clone(),
        duration_seconds: t.duration_seconds,
        views: plays,
        ..MediaItem::default()
    })
}

fn playlist_item(list: &serde_json::Value) -> Option<MediaItem> {
    let id = list.get("id")?.as_u64()?;
    let title = list.get("title")?.as_str()?;
    let (artist, _) = user_of(list);
    let count = list.get("track_count").and_then(|c| c.as_u64()).map(|c| format!("{c} tracks"));
    Some(MediaItem {
        kind: ItemKind::Playlist,
        id: format!("sc:playlist:{id}"),
        title: title.to_owned(),
        artists: vec![Person { name: artist, id: None }],
        thumb: artwork(list),
        count,
        ..MediaItem::default()
    })
}

fn user_item(user: &serde_json::Value) -> Option<MediaItem> {
    let id = user.get("id")?.as_u64()?;
    let name = user.get("username")?.as_str()?;
    let followers = user.get("followers_count").and_then(|f| f.as_u64()).map(|f| format!("{} followers", crate::model::compact_count(f)));
    Some(MediaItem {
        kind: ItemKind::Artist,
        id: format!("sc:user:{id}"),
        title: name.to_owned(),
        thumb: user.get("avatar_url").and_then(|u| u.as_str()).map(sized_artwork),
        subscribers: followers,
        ..MediaItem::default()
    })
}

fn parse_search_item(item: &serde_json::Value) -> Option<MediaItem> {
    match item.get("kind").and_then(|k| k.as_str()) {
        Some("track") => track_item(item),
        Some("playlist") => playlist_item(item),
        Some("user") => user_item(item),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track_json() -> serde_json::Value {
        serde_json::json!({
            "kind": "track",
            "id": 191477804,
            "title": "DESPACITO",
            "duration": 212708,
            "full_duration": 212708,
            "artwork_url": "https://i1.sndcdn.com/artworks-000203619008-9xvrhm-large.jpg",
            "permalink_url": "https://soundcloud.com/traphousemusic1/despacito",
            "playback_count": 19645686,
            "streamable": true,
            "user": {"id": 12345, "username": "traphousemusic1", "avatar_url": "https://i1.sndcdn.com/avatars-abc-large.jpg"}
        })
    }

    #[test]
    fn a_track_maps_to_the_queue_model() {
        let t = parse_track(&track_json()).unwrap();
        assert_eq!(t.video_id.as_str(), "sc:191477804");
        assert!(t.video_id.is_soundcloud());
        assert_eq!(t.title, "DESPACITO");
        assert_eq!(t.artist, "traphousemusic1");
        assert_eq!(t.artists[0].id.as_deref(), Some("sc:user:12345"));
        assert_eq!(t.duration_seconds, Some(212));
        assert_eq!(t.thumb.as_deref(), Some("https://i1.sndcdn.com/artworks-000203619008-9xvrhm-t500x500.jpg"));
        assert!(t.is_available);
    }

    #[test]
    fn a_blocked_track_is_marked_unavailable() {
        let mut v = track_json();
        v["policy"] = serde_json::json!("BLOCK");
        assert!(!parse_track(&v).unwrap().is_available);
    }

    #[test]
    fn ids_split_into_kind_and_number() {
        assert_eq!(parse_sc_id("sc:191477804"), Some(ScId::Track(191477804)));
        assert_eq!(parse_sc_id("sc:playlist:272857198"), Some(ScId::Playlist(272857198)));
        assert_eq!(parse_sc_id("sc:user:23667548"), Some(ScId::User(23667548)));
        assert_eq!(parse_sc_id("VLOLAKabc"), None);
        assert!(is_soundcloud_id("sc:1"));
        assert!(!is_soundcloud_id("abc123"));
    }

    #[test]
    fn artwork_grows_to_the_large_variant() {
        assert_eq!(sized_artwork("https://i1.sndcdn.com/x-large.jpg"), "https://i1.sndcdn.com/x-t500x500.jpg");
        assert_eq!(sized_artwork("https://i1.sndcdn.com/x-t300x300.jpg"), "https://i1.sndcdn.com/x-t500x500.jpg");
        assert_eq!(sized_artwork("https://i1.sndcdn.com/x.jpg"), "https://i1.sndcdn.com/x.jpg");
    }

    #[test]
    fn client_ids_come_out_of_js_bundles() {
        let js = r#"webpackJsonp([1],{123:function(e){e.exports={client_id:"a3e059563d7fd3372b49b37f00a00bcf",other:1}}})"#;
        assert_eq!(find_client_id(js).as_deref(), Some("a3e059563d7fd3372b49b37f00a00bcf"));
        assert_eq!(find_client_id("no id here"), None);
    }

    #[test]
    fn search_items_route_by_kind() {
        let track = track_item(&track_json()).unwrap();
        assert_eq!(track.kind, ItemKind::Song);
        assert_eq!(track.views.as_deref(), Some("19.6M"));
        let user = user_item(&serde_json::json!({"kind": "user", "id": 9, "username": "dj", "avatar_url": "https://i1.sndcdn.com/a-large.jpg", "followers_count": 1500})).unwrap();
        assert_eq!(user.kind, ItemKind::Artist);
        assert_eq!(user.id, "sc:user:9");
        let list = playlist_item(&serde_json::json!({"kind": "playlist", "id": 7, "title": "Mix", "track_count": 12, "artwork_url": null, "user": {"username": "dj"}})).unwrap();
        assert_eq!(list.id, "sc:playlist:7");
        assert_eq!(list.count.as_deref(), Some("12 tracks"));
    }

    #[test]
    fn counts_stay_compact() {
        assert_eq!(crate::model::compact_count(999), "999");
        assert_eq!(crate::model::compact_count(1500), "1.5K");
        assert_eq!(crate::model::compact_count(19645686), "19.6M");
    }
}
