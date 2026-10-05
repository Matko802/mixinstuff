//! YouTube and YouTube Music links, turned into what the app opens: a song to
//! play, or a playlist, album or artist page. The common shapes parse here
//! without a request. Anything else on a YouTube host, such as an @handle,
//! goes to InnerTube's `navigation/resolve_url`, which answers with the
//! endpoint the web app would follow.

use serde_json::json;

use crate::net::ytmusic::NetError;
use crate::net::browse::Browse;
use crate::net::items::owned_at;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Link {
    /// A song or video, in the context of a playlist when the link names one.
    Song { video_id: String, playlist_id: Option<String> },
    /// A playlist, or an album's OLAK5uy_ playlist, opened as a page.
    Playlist(String),
    /// An album page, MPREb_ id.
    Album(String),
    Artist(String),
}

/// The app's own link scheme. The desktop entry registers it, so the system
/// opens `mixinstuff://open?url=<link>` here. `mixinstuff://music.youtube.com/...`
/// reads as the same link over https.
pub const SCHEME: &str = "mixinstuff";

const HOSTS: [&str; 6] = ["music.youtube.com", "youtube.com", "www.youtube.com", "m.youtube.com", "youtu.be", "www.youtu.be"];

/// Whether the text is a link to a YouTube host, the test the search field uses.
pub fn is_youtube_url(text: &str) -> bool {
    // A bare "youtube.com" typed into search is a search, not a link to nowhere.
    parse_url(text).is_some_and(|url| url.host_str().is_some_and(|host| HOSTS.contains(&host)) && (url.path() != "/" || url.query().is_some()))
}

fn parse_url(text: &str) -> Option<reqwest::Url> {
    let text = text.trim();
    if let Some(rest) = text.strip_prefix(SCHEME).and_then(|r| r.strip_prefix(':')) {
        return unwrap_scheme(rest.trim_start_matches('/'));
    }
    let with_scheme = if text.starts_with("http://") || text.starts_with("https://") { text.to_owned() } else { format!("https://{text}") };
    reqwest::Url::parse(&with_scheme).ok()
}

/// What a `mixinstuff:` link carries: the `url` of `open?url=`, or the rest of
/// the link as an https address.
fn unwrap_scheme(rest: &str) -> Option<reqwest::Url> {
    if rest.starts_with("open") {
        let wrapper = reqwest::Url::parse(&format!("{SCHEME}://{rest}")).ok()?;
        let inner = wrapper.query_pairs().find(|(k, _)| k == "url").map(|(_, v)| v.into_owned())?;
        // One level only: a link that wraps itself goes nowhere.
        return if inner.starts_with(SCHEME) { None } else { parse_url(&inner) };
    }
    reqwest::Url::parse(&format!("https://{rest}")).ok()
}

/// The link read without the network, or None when only YouTube can tell.
pub fn parse(text: &str) -> Option<Link> {
    let url = parse_url(text)?;
    let host = url.host_str()?;
    if !HOSTS.contains(&host) {
        return None;
    }
    let query = |key: &str| url.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned()).filter(|v| !v.is_empty());
    let segments: Vec<&str> = url.path_segments().map(|s| s.filter(|p| !p.is_empty()).collect()).unwrap_or_default();
    if host.ends_with("youtu.be") {
        let video_id = segments.first()?.to_string();
        return Some(Link::Song { video_id, playlist_id: query("list") });
    }
    match segments.as_slice() {
        ["watch"] => Some(Link::Song { video_id: query("v")?, playlist_id: query("list") }),
        ["shorts" | "live" | "embed", id] => Some(Link::Song { video_id: (*id).to_owned(), playlist_id: None }),
        ["playlist"] => Some(Link::Playlist(query("list")?)),
        ["channel", id] => Some(Link::Artist((*id).to_owned())),
        ["browse", id] => from_browse_id(id),
        _ => None,
    }
}

fn from_browse_id(id: &str) -> Option<Link> {
    if id.starts_with("MPREb") {
        Some(Link::Album(id.to_owned()))
    } else if id.starts_with("MPSP") {
        // A podcast show opens on the playlist page, which knows the id.
        Some(Link::Playlist(id.to_owned()))
    } else if id.starts_with("UC") {
        Some(Link::Artist(id.to_owned()))
    } else {
        id.strip_prefix("VL").map(|playlist| Link::Playlist(playlist.to_owned()))
    }
}

/// The link, asking YouTube when the shape alone does not say.
pub async fn resolve(api: &dyn Browse, text: &str) -> Result<Option<Link>, NetError> {
    if let Some(link) = parse(text) {
        return Ok(Some(link));
    }
    let Some(mut url) = parse_url(text).filter(|_| is_youtube_url(text)) else { return Ok(None) };
    // The music client resolves only its own host. A youtube.com handle comes back as a plain URL.
    let _ = url.set_host(Some("music.youtube.com"));
    let response = api.post("navigation/resolve_url", json!({ "url": url.as_str() })).await?;
    if let Some(video_id) = owned_at(&response, "/endpoint/watchEndpoint/videoId") {
        return Ok(Some(Link::Song { video_id, playlist_id: owned_at(&response, "/endpoint/watchEndpoint/playlistId") }));
    }
    if let Some(playlist) = owned_at(&response, "/endpoint/watchPlaylistEndpoint/playlistId") {
        return Ok(Some(Link::Playlist(playlist)));
    }
    Ok(owned_at(&response, "/endpoint/browseEndpoint/browseId").and_then(|id| from_browse_id(&id)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(video_id: &str, playlist_id: Option<&str>) -> Option<Link> {
        Some(Link::Song { video_id: video_id.to_owned(), playlist_id: playlist_id.map(str::to_owned) })
    }

    #[test]
    fn songs_from_every_host() {
        assert_eq!(parse("https://music.youtube.com/watch?v=dQw4w9WgXcQ&si=abc"), song("dQw4w9WgXcQ", None));
        assert_eq!(parse("https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=PLx"), song("dQw4w9WgXcQ", Some("PLx")));
        assert_eq!(parse("https://youtu.be/dQw4w9WgXcQ?si=abc"), song("dQw4w9WgXcQ", None));
        assert_eq!(parse("music.youtube.com/watch?v=dQw4w9WgXcQ"), song("dQw4w9WgXcQ", None), "no scheme, as pasted from an address bar");
        assert_eq!(parse("https://youtube.com/shorts/abcdefghijk"), song("abcdefghijk", None));
    }

    #[test]
    fn pages_from_their_paths() {
        assert_eq!(parse("https://music.youtube.com/playlist?list=OLAK5uy_abc"), Some(Link::Playlist("OLAK5uy_abc".into())));
        assert_eq!(parse("https://music.youtube.com/browse/MPREb_abc"), Some(Link::Album("MPREb_abc".into())));
        assert_eq!(parse("https://music.youtube.com/browse/VLPLabc"), Some(Link::Playlist("PLabc".into())));
        assert_eq!(parse("https://music.youtube.com/channel/UCabc"), Some(Link::Artist("UCabc".into())));
        assert_eq!(parse("https://music.youtube.com/browse/UCabc"), Some(Link::Artist("UCabc".into())));
        assert_eq!(parse("https://music.youtube.com/browse/MPSPPLabc"), Some(Link::Playlist("MPSPPLabc".into())));
    }

    #[test]
    fn the_apps_own_scheme_carries_a_link() {
        let wrapped = "mixinstuff://open?url=https%3A%2F%2Fmusic.youtube.com%2Fwatch%3Fv%3DdQw4w9WgXcQ%26list%3DPLx";
        assert_eq!(parse(wrapped), song("dQw4w9WgXcQ", Some("PLx")));
        assert!(is_youtube_url(wrapped));
        assert_eq!(parse("mixinstuff://music.youtube.com/browse/MPREb_abc"), Some(Link::Album("MPREb_abc".into())));
        assert_eq!(parse("mixinstuff://open?url=mixinstuff%3A%2F%2Fopen"), None, "no link inside a link");
        assert_eq!(parse("mixinstuff://open?url=https%3A%2F%2Fexample.com%2F"), None);
    }

    #[test]
    fn other_text_is_not_a_link() {
        assert_eq!(parse("never gonna give you up"), None);
        assert_eq!(parse("https://example.com/watch?v=dQw4w9WgXcQ"), None);
        assert!(!is_youtube_url("queen"));
        assert!(!is_youtube_url("youtube.com"));
        assert!(is_youtube_url("https://www.youtube.com/@RickAstleyYT"));
        assert_eq!(parse("https://www.youtube.com/@RickAstleyYT"), None, "a handle needs YouTube to resolve");
    }

    /// `cargo test -- --ignored a_handle_resolves --nocapture`
    #[tokio::test]
    #[ignore]
    async fn a_handle_resolves() {
        let dir = tempfile::tempdir().unwrap();
        let client = crate::net::ytmusic::YtMusic::new(&crate::paths::Paths::for_tests(dir.path())).unwrap();
        let link = resolve(&client.api(), "https://www.youtube.com/@RickAstleyYT").await.unwrap();
        println!("{link:?}");
        assert!(matches!(link, Some(Link::Artist(id)) if id.starts_with("UC")));
    }
}
