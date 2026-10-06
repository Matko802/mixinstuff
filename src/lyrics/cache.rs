
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use super::model::LyricsResult;
use crate::paths::Paths;

pub const PIPELINE_VERSION: u64 = 12;

const MAX_ENTRIES: usize = 2000;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entry {
    pub preferred_source: Option<String>,
    pub results: Vec<(String, LyricsResult)>,
}

impl Entry {
    pub fn get(&self, source: &str) -> Option<&LyricsResult> {
        self.results.iter().find(|(name, _)| name == source).map(|(_, res)| res)
    }

    fn insert(&mut self, source: String, result: LyricsResult) {
        match self.results.iter_mut().find(|(name, _)| *name == source) {
            Some(slot) => slot.1 = result,
            None => self.results.push((source, result)),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    preferred_source: Option<String>,
    #[serde(default)]
    results: Ordered,
    #[serde(default)]
    pipeline: Option<u64>,
}

#[derive(Default)]
struct Ordered(Vec<(String, Value)>);

impl Serialize for Ordered {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (key, value) in &self.0 {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Ordered {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Entries;
        impl<'de> Visitor<'de> for Entries {
            type Value = Ordered;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object of lyrics results")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Ordered, A::Error> {
                let mut out = Vec::new();
                while let Some(pair) = map.next_entry::<String, Value>()? {
                    out.push(pair);
                }
                Ok(Ordered(out))
            }

            fn visit_unit<E>(self) -> Result<Ordered, E> {
                Ok(Ordered::default())
            }
        }
        deserializer.deserialize_any(Entries)
    }
}

pub struct LyricsCache {
    dir: PathBuf,
    mem: Mutex<HashMap<String, Entry>>,
    writer: Mutex<()>,
    max_entries: usize,
    writes: std::sync::atomic::AtomicUsize,
}

const EVICT_EVERY: usize = 25;

#[allow(dead_code)]
impl LyricsCache {
    pub fn new(paths: &Paths) -> Self {
        let dir = paths.cache_dir.join("lyrics");
        if let Err(err) = std::fs::create_dir_all(&dir) {
            tracing::warn!(?dir, %err, "could not create the lyrics cache directory");
        }
        Self { dir, mem: Mutex::new(HashMap::new()), writer: Mutex::new(()), max_entries: MAX_ENTRIES, writes: std::sync::atomic::AtomicUsize::new(0) }
    }

    fn path_for(&self, video_id: &str) -> PathBuf {
        self.dir.join(format!("{video_id}.json"))
    }

    pub fn load(&self, video_id: &str) -> Option<Entry> {
        if video_id.is_empty() {
            return None;
        }
        let mut mem = self.mem.lock().unwrap();
        if let Some(entry) = mem.get(video_id) {
            return Some(entry.clone());
        }
        let text = std::fs::read_to_string(self.path_for(video_id)).ok()?;
        let stored: Stored = serde_json::from_str(&text).ok()?;
        let entry = entry_from(stored);
        mem.insert(video_id.to_owned(), entry.clone());
        Some(entry)
    }

    pub fn get_result(&self, video_id: &str, order: Option<&[String]>, accept_rank: u8) -> Option<LyricsResult> {
        let entry = self.load(video_id)?;
        if entry.results.is_empty() {
            return None;
        }
        if let Some(pinned) = entry.preferred_source.as_deref().filter(|p| !p.is_empty())
            && let Some(result) = entry.get(pinned)
        {
            return Some(result.clone());
        }
        if let Some(order) = order.filter(|o| !o.is_empty()) {
            let mut fallback: Option<&LyricsResult> = None;
            for name in order {
                let credited = || (name == "YouTube Music").then(|| entry.results.iter().find(|(source, _)| !crate::lyrics::prefs::DEFAULT_PROVIDER_ORDER.contains(&source.as_str())).map(|(_, result)| result)).flatten();
                let Some(result) = entry.get(name).or_else(credited).filter(|r| !r.lines.is_empty()) else { continue };
                if result.rank() >= accept_rank {
                    return Some(result.clone());
                }
                if result.rank() > fallback.map_or(0, LyricsResult::rank) {
                    fallback = Some(result);
                }
            }
            return fallback.cloned();
        }
        self.get_alternatives(video_id).into_iter().next().map(|(_, result)| result)
    }

    pub fn get_alternatives(&self, video_id: &str) -> Vec<(String, LyricsResult)> {
        let mut items = self.load(video_id).map(|e| e.results).unwrap_or_default();
        items.sort_by_key(|(_, result)| std::cmp::Reverse(result.rank()));
        items
    }

    pub fn get_preferred(&self, video_id: &str) -> Option<String> {
        self.load(video_id)?.preferred_source
    }

    pub fn has_source(&self, video_id: &str, source: &str) -> bool {
        self.load(video_id).is_some_and(|e| e.get(source).is_some())
    }

    pub fn add_result(&self, video_id: &str, result: &LyricsResult, user_choice: bool) {
        if video_id.is_empty() || result.lines.is_empty() {
            return;
        }
        let _writing = self.writer.lock().unwrap();
        let mut result = result.clone();
        if user_choice {
            result.user_choice = true;
        }
        let mut entry = self.load(video_id).unwrap_or_default();
        entry.insert(source_key(&result), result);
        self.write(video_id, entry);
    }

    pub fn add_results(&self, video_id: &str, results: &[LyricsResult]) {
        if video_id.is_empty() || results.is_empty() {
            return;
        }
        let _writing = self.writer.lock().unwrap();
        let mut entry = self.load(video_id).unwrap_or_default();
        for result in results.iter().filter(|r| !r.lines.is_empty()) {
            entry.insert(source_key(result), result.clone());
        }
        self.write(video_id, entry);
    }

    pub fn set_preferred(&self, video_id: &str, source: Option<&str>) {
        if video_id.is_empty() {
            return;
        }
        let _writing = self.writer.lock().unwrap();
        let mut entry = self.load(video_id).unwrap_or_default();
        entry.preferred_source = source.map(str::to_owned);
        self.write(video_id, entry);
    }

    pub fn clear_user_choice(&self, video_id: &str) {
        let _writing = self.writer.lock().unwrap();
        let Some(mut entry) = self.load(video_id) else { return };
        entry.preferred_source = None;
        entry.results.retain(|(_, result)| !result.user_choice);
        self.write(video_id, entry);
    }

    pub fn invalidate(&self, video_id: &str) {
        if video_id.is_empty() {
            return;
        }
        self.mem.lock().unwrap().remove(video_id);
        let _ = std::fs::remove_file(self.path_for(video_id));
    }

    pub fn clear_all(&self) -> usize {
        self.mem.lock().unwrap().clear();
        let Ok(dir) = std::fs::read_dir(&self.dir) else { return 0 };
        dir.flatten().filter(|f| f.path().extension().is_some_and(|e| e == "json")).filter(|f| std::fs::remove_file(f.path()).is_ok()).count()
    }

    fn write(&self, video_id: &str, entry: Entry) {
        let mut results = Ordered::default();
        for (name, result) in &entry.results {
            match serde_json::to_value(result) {
                Ok(value) => results.0.push((name.clone(), value)),
                Err(err) => tracing::warn!(%err, video_id, "lyrics result would not serialize"),
            }
        }
        let stored = Stored { preferred_source: entry.preferred_source.clone(), results, pipeline: Some(PIPELINE_VERSION) };
        let written = serde_json::to_vec(&stored).map_err(std::io::Error::other).and_then(|bytes| std::fs::write(self.path_for(video_id), bytes));
        if let Err(err) = written {
            tracing::warn!(%err, video_id, "lyrics cache write failed");
            return;
        }
        let in_memory = {
            let mut mem = self.mem.lock().unwrap();
            mem.insert(video_id.to_owned(), entry);
            mem.len()
        };
        let nth = self.writes.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if nth % EVICT_EVERY == 0 || in_memory > self.max_entries {
            self.evict_old();
        }
    }

    fn evict_old(&self) {
        let Ok(dir) = std::fs::read_dir(&self.dir) else { return };
        let mut files: Vec<(std::time::SystemTime, PathBuf)> = dir
            .flatten()
            .map(|f| f.path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
            .collect();
        if files.len() <= self.max_entries {
            return;
        }
        files.sort();
        let excess = files.len() - self.max_entries;
        let mut mem = self.mem.lock().unwrap();
        for (_, path) in files.into_iter().take(excess) {
            let _ = std::fs::remove_file(&path);
            if let Some(video_id) = path.file_stem().and_then(|s| s.to_str()) {
                mem.remove(video_id);
            }
        }
    }
}

fn source_key(result: &LyricsResult) -> String {
    if result.source.is_empty() { "Unknown".to_owned() } else { result.source.clone() }
}

fn entry_from(stored: Stored) -> Entry {
    let current = stored.pipeline == Some(PIPELINE_VERSION);
    let results = stored
        .results
        .0
        .into_iter()
        .filter_map(|(name, value)| Some((name, serde_json::from_value::<LyricsResult>(value).ok()?)))
        .filter(|(_, result)| current || result.user_choice)
        .collect();
    Entry { preferred_source: stored.preferred_source, results }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::model::{LyricLine, LyricPart};
    use serde_json::json;

    fn cache() -> (tempfile::TempDir, LyricsCache) {
        let dir = tempfile::tempdir().unwrap();
        let cache = LyricsCache::new(&Paths::for_tests(dir.path()));
        (dir, cache)
    }

    fn plain(source: &str) -> LyricsResult {
        LyricsResult::from_lines(vec![LyricLine::new(None, "la")], source)
    }

    fn synced(source: &str) -> LyricsResult {
        LyricsResult::from_lines(vec![LyricLine::new(Some(1.0), "la")], source)
    }

    fn word(source: &str) -> LyricsResult {
        let mut result = synced(source);
        result.lines[0].parts.push(LyricPart { start: Some(1.0), end: Some(2.0), text: "la".into(), space_after: false });
        result
    }

    fn order(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn a_youtube_music_credit_answers_for_the_youtube_music_slot() {
        let dir = tempfile::tempdir().unwrap();
        let cache = LyricsCache::new(&crate::paths::Paths::for_tests(dir.path()));
        cache.add_result("v", &plain("Musixmatch"), false);
        let order = vec!["LRCLIB".to_owned(), "YouTube Music".to_owned()];
        assert_eq!(cache.get_result("v", Some(&order), 1).map(|r| r.source), Some("Musixmatch".to_owned()));
        let without = vec!["LRCLIB".to_owned()];
        assert_eq!(cache.get_result("v", Some(&without), 1), None, "a disabled YouTube Music stays disabled");
    }

    #[test]
    fn lives_where_the_python_app_keeps_it() {
        let (dir, cache) = cache();
        cache.add_result("abc", &plain("LRCLIB"), false);
        let path = dir.path().join("cache/lyrics/abc.json");
        let raw: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(raw, json!({"preferred_source": null, "results": {"LRCLIB": {"lines": [{"start": null, "text": "la"}], "synced": false, "source": "LRCLIB"}}, "pipeline": 12}));
    }

    #[test]
    fn reads_a_file_the_python_app_wrote() {
        let (dir, cache) = cache();
        let raw = json!({"preferred_source": "NetEase", "pipeline": 12, "results": {
            "NetEase": {"lines": [{"start": 1.5, "text": "千本桜", "romanization": "senbonzakura"}], "synced": true, "source": "NetEase"},
            "LRCLIB": {"lines": [{"start": 2, "text": "la"}], "synced": true, "source": "LRCLIB"}
        }});
        std::fs::write(dir.path().join("cache/lyrics/vid.json"), raw.to_string()).unwrap();
        let shown = cache.get_result("vid", None, 1).unwrap();
        assert_eq!(shown.source, "NetEase");
        assert_eq!(shown.lines[0].romanization.as_deref(), Some("senbonzakura"));
        assert_eq!(cache.get_preferred("vid").as_deref(), Some("NetEase"));
        assert!(cache.has_source("vid", "LRCLIB"));
    }

    #[test]
    fn a_stale_pipeline_keeps_only_the_pin_and_hand_picked_results() {
        let (dir, cache) = cache();
        let raw = json!({"preferred_source": "Apple Music", "pipeline": 11, "results": {
            "Apple Music": {"lines": [{"start": 1, "text": "a"}], "synced": true, "source": "Apple Music", "user_choice": true},
            "LRCLIB": {"lines": [{"start": 1, "text": "a"}], "synced": true, "source": "LRCLIB"}
        }});
        std::fs::write(dir.path().join("cache/lyrics/old.json"), raw.to_string()).unwrap();
        let entry = cache.load("old").unwrap();
        assert_eq!(entry.preferred_source.as_deref(), Some("Apple Music"));
        assert_eq!(entry.results.len(), 1);
        assert!(entry.get("Apple Music").unwrap().user_choice);
    }

    #[test]
    fn a_broken_file_is_a_miss() {
        let (dir, cache) = cache();
        std::fs::write(dir.path().join("cache/lyrics/bad.json"), "[1, 2").unwrap();
        assert!(cache.load("bad").is_none());
        std::fs::write(dir.path().join("cache/lyrics/list.json"), "[1, 2]").unwrap();
        assert!(cache.load("list").is_none());
        assert!(cache.load("").is_none());
        assert!(cache.get_result("never", None, 1).is_none());
    }

    #[test]
    fn the_order_walk_mirrors_the_chain() {
        let (_dir, cache) = cache();
        cache.add_results("v", &[plain("Apple Music"), synced("LRCLIB"), word("BiniLyrics")]);
        let queue = order(&["Apple Music", "LRCLIB", "BiniLyrics"]);
        assert_eq!(cache.get_result("v", Some(&queue), 2).unwrap().source, "LRCLIB");
        assert_eq!(cache.get_result("v", Some(&queue), 1).unwrap().source, "Apple Music");
        let only_plain = order(&["Apple Music", "NetEase"]);
        assert_eq!(cache.get_result("v", Some(&only_plain), 2).unwrap().source, "Apple Music");
        assert!(cache.get_result("v", Some(&order(&["NetEase"])), 1).is_none());
        assert_eq!(cache.get_result("v", None, 1).unwrap().source, "BiniLyrics");
    }

    #[test]
    fn a_pin_beats_the_order_until_it_is_cleared() {
        let (_dir, cache) = cache();
        cache.add_results("v", &[word("Apple Music"), synced("LRCLIB")]);
        cache.set_preferred("v", Some("LRCLIB"));
        let queue = order(&["Apple Music", "LRCLIB"]);
        assert_eq!(cache.get_result("v", Some(&queue), 2).unwrap().source, "LRCLIB");
        cache.set_preferred("v", None);
        assert_eq!(cache.get_result("v", Some(&queue), 2).unwrap().source, "Apple Music");
        cache.set_preferred("v", Some("NetEase"));
        assert_eq!(cache.get_result("v", Some(&queue), 2).unwrap().source, "Apple Music");
    }

    #[test]
    fn alternatives_are_richest_first_and_stable() {
        let (_dir, cache) = cache();
        cache.add_results("v", &[synced("NetEase"), plain("YouTube Music"), synced("LRCLIB"), word("Apple Music")]);
        let names: Vec<String> = cache.get_alternatives("v").into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["Apple Music", "NetEase", "LRCLIB", "YouTube Music"]);
        assert!(cache.get_alternatives("none").is_empty());
    }

    #[test]
    fn clearing_a_user_choice_keeps_what_the_chain_cached() {
        let (_dir, cache) = cache();
        cache.add_result("v", &synced("LRCLIB"), false);
        cache.add_result("v", &word("Apple Music"), true);
        cache.set_preferred("v", Some("Apple Music"));
        cache.clear_user_choice("v");
        assert_eq!(cache.get_preferred("v"), None);
        let names: Vec<String> = cache.get_alternatives("v").into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["LRCLIB"]);
    }

    #[test]
    fn empty_results_are_not_stored_and_a_nameless_source_is_unknown() {
        let (_dir, cache) = cache();
        cache.add_result("v", &LyricsResult::default(), false);
        assert!(cache.load("v").is_none());
        cache.add_result("v", &plain(""), false);
        assert!(cache.has_source("v", "Unknown"));
    }

    #[test]
    fn a_second_result_from_one_source_replaces_the_first() {
        let (_dir, cache) = cache();
        cache.add_result("v", &plain("LRCLIB"), false);
        cache.add_result("v", &synced("LRCLIB"), false);
        let alternatives = cache.get_alternatives("v");
        assert_eq!(alternatives.len(), 1);
        assert!(alternatives[0].1.synced);
    }

    #[test]
    fn invalidate_and_clear_all_reach_the_disk() {
        let (dir, cache) = cache();
        for id in ["a", "b", "c"] {
            cache.add_result(id, &plain("LRCLIB"), false);
        }
        cache.invalidate("a");
        assert!(cache.load("a").is_none());
        assert!(!dir.path().join("cache/lyrics/a.json").exists());
        assert_eq!(cache.clear_all(), 2);
        assert!(cache.load("b").is_none());
        assert_eq!(cache.clear_all(), 0);
    }

    #[test]
    fn a_fresh_instance_reads_what_another_wrote() {
        let (dir, cache) = cache();
        cache.add_result("v", &word("Apple Music"), true);
        cache.set_preferred("v", Some("Apple Music"));
        let again = LyricsCache::new(&Paths::for_tests(dir.path()));
        let entry = again.load("v").unwrap();
        assert_eq!(entry, cache.load("v").unwrap());
        assert!(entry.get("Apple Music").unwrap().user_choice);
    }

    #[test]
    fn the_oldest_files_are_evicted_past_the_cap() {
        let (dir, mut cache) = cache();
        cache.max_entries = 2;
        for (age, id) in ["old", "mid"].iter().enumerate() {
            cache.add_result(id, &plain("LRCLIB"), false);
            let file = std::fs::File::options().write(true).open(dir.path().join(format!("cache/lyrics/{id}.json"))).unwrap();
            file.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(1000 - age as u64 * 100)).unwrap();
        }
        cache.add_result("new", &plain("LRCLIB"), false);
        assert!(!dir.path().join("cache/lyrics/old.json").exists());
        assert!(cache.load("old").is_none());
        assert!(cache.load("mid").is_some());
        assert!(cache.load("new").is_some());
    }
}
