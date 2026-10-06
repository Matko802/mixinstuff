
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

#[derive(Clone, Debug)]
pub struct Paths {
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub stream_cache_dir: PathBuf,
    pub auth_file: PathBuf,
    pub prefs_file: PathBuf,
    pub config_file: PathBuf,
    music_override: Option<PathBuf>,
}

impl Paths {
    pub fn discover() -> Self {
        let data_dir = glib::user_data_dir().join("musishark");
        let cache_dir = glib::user_cache_dir().join("musishark");
        let stream_cache_dir = cache_dir.join("streams");
        for dir in [&data_dir, &stream_cache_dir] {
            if let Err(err) = std::fs::create_dir_all(dir) {
                tracing::warn!(?dir, %err, "could not create directory");
            }
        }
        Self {
            auth_file: data_dir.join("headers_auth.json"),
            prefs_file: data_dir.join("prefs.json"),
            config_file: data_dir.join("config.json"),
            data_dir,
            cache_dir,
            stream_cache_dir,
            music_override: std::env::var_os("MUSISHARK_MUSIC_DIR").map(PathBuf::from),
        }
    }

    #[cfg(test)]
    pub fn for_tests(root: &Path) -> Self {
        let data_dir = root.join("data");
        let cache_dir = root.join("cache");
        std::fs::create_dir_all(&data_dir).expect("test data dir");
        Self {
            auth_file: data_dir.join("headers_auth.json"),
            prefs_file: data_dir.join("prefs.json"),
            config_file: data_dir.join("config.json"),
            stream_cache_dir: cache_dir.join("streams"),
            data_dir,
            cache_dir,
            music_override: Some(root.join("Music/Musishark")),
        }
    }

    pub fn music_dir(&self) -> PathBuf {
        if let Some(dir) = &self.music_override {
            return dir.clone();
        }
        glib::user_special_dir(glib::UserDirectory::Music).unwrap_or_else(|| glib::home_dir().join("Music")).join("Musishark")
    }

    pub fn playlist_cover_path(&self, title: &str) -> Option<PathBuf> {
        if title.is_empty() {
            return None;
        }
        Some(self.music_dir().join("Playlists").join(format!("{}.jpg", sanitize_filename(title))))
    }

    pub fn local_playlist_cover(&self, title: &str) -> Option<PathBuf> {
        self.playlist_cover_path(title).filter(|p| p.is_file())
    }

    pub fn read_prefs(&self) -> Map<String, Value> {
        read_object(&self.prefs_file)
    }

    pub fn read_config(&self) -> Map<String, Value> {
        read_object(&self.config_file)
    }

    pub fn update_prefs(&self, apply: impl FnOnce(&mut Map<String, Value>)) {
        let mut prefs = self.read_prefs();
        apply(&mut prefs);
        let write = serde_json::to_vec(&Value::Object(prefs)).map_err(std::io::Error::other).and_then(|bytes| std::fs::write(&self.prefs_file, bytes));
        if let Err(err) = write {
            tracing::warn!(%err, "could not save prefs.json");
        }
    }
}

fn read_object(path: &Path) -> Map<String, Value> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Map::new();
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    }
}

pub fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name.chars().filter(|c| !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')).collect();
    let mut trimmed = cleaned.trim_matches(|c| c == '.' || c == ' ').to_owned();
    while trimmed.len() > 200 {
        trimmed.pop();
    }
    let trimmed = trimmed.trim_matches(|c| c == '.' || c == ' ');
    if trimmed.is_empty() { "Unknown".to_owned() } else { trimmed.to_owned() }
}
