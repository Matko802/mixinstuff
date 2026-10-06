
use crate::model::{LikeStatus, RepeatMode, Track, VideoId};

pub const PREVIOUS_RESTART_THRESHOLD: f64 = 5.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Load(usize),
    Restart,
    Stop,
    Extend,
    Stay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Bounds {
    pub can_next: bool,
    pub can_previous: bool,
}

#[derive(Default)]
pub struct Queue {
    tracks: Vec<Track>,
    original: Vec<Track>,
    current: Option<usize>,
    shuffle: bool,
    repeat: RepeatMode,
    source_id: Option<String>,
    infinite: bool,
}

impl Queue {

    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    pub fn track_at(&self, index: usize) -> Option<&Track> {
        self.tracks.get(index)
    }

    pub fn current(&self) -> Option<usize> {
        self.current
    }

    pub fn current_track(&self) -> Option<&Track> {
        self.current.and_then(|i| self.tracks.get(i))
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    pub fn shuffle(&self) -> bool {
        self.shuffle
    }

    pub fn repeat(&self) -> RepeatMode {
        self.repeat
    }

    pub fn source_id(&self) -> Option<&str> {
        self.source_id.as_deref()
    }

    pub fn is_infinite(&self) -> bool {
        self.infinite
    }

    pub fn bounds(&self, position: f64) -> Bounds {
        let Some(current) = self.current else { return Bounds::default() };
        Bounds {
            can_next: current + 1 < self.tracks.len() || (self.repeat == RepeatMode::All && !self.is_empty()) || self.can_extend(),
            can_previous: current > 0 || position > PREVIOUS_RESTART_THRESHOLD,
        }
    }

    pub fn armable_next(&self) -> Option<usize> {
        let current = self.current?;
        match self.repeat {
            RepeatMode::Track => Some(current),
            _ if current + 1 < self.tracks.len() => Some(current + 1),
            RepeatMode::All if !self.is_empty() => Some(0),
            _ => None,
        }
    }

    fn can_extend(&self) -> bool {
        self.infinite && self.source_id.is_some() && !self.is_empty()
    }


    pub fn advance(&mut self) -> Step {
        let Some(current) = self.current else { return self.exhausted() };
        if current + 1 < self.tracks.len() {
            return self.go(current + 1);
        }
        if self.repeat == RepeatMode::All && !self.is_empty() {
            return self.go(0);
        }
        self.exhausted()
    }

    pub fn finished(&mut self) -> Step {
        match (self.repeat, self.current) {
            (RepeatMode::Track, Some(current)) => Step::Load(current),
            _ => self.advance(),
        }
    }

    pub fn back(&mut self, position: f64) -> Step {
        let Some(current) = self.current else { return Step::Stay };
        if position > PREVIOUS_RESTART_THRESHOLD || current == 0 {
            return Step::Restart;
        }
        self.go(current - 1)
    }

    pub fn jump(&mut self, index: usize) -> Step {
        if index >= self.tracks.len() { Step::Stay } else { self.go(index) }
    }

    pub fn failed_current(&mut self) -> Step {
        match self.current {
            Some(current) if current + 1 < self.tracks.len() => self.go(current + 1),
            _ => {
                self.current = None;
                Step::Stop
            }
        }
    }

    pub fn adopt(&mut self, index: usize) -> Option<&Track> {
        if index < self.tracks.len() {
            self.current = Some(index);
        }
        self.current_track()
    }

    fn go(&mut self, index: usize) -> Step {
        self.current = Some(index);
        Step::Load(index)
    }

    fn exhausted(&mut self) -> Step {
        if self.can_extend() {
            return Step::Extend;
        }
        self.current = None;
        Step::Stop
    }


    pub fn replace(&mut self, tracks: Vec<Track>, start_index: usize, shuffle: bool, source_id: Option<String>, infinite: bool) -> Step {
        self.original = tracks.clone();
        self.tracks = tracks;
        self.shuffle = shuffle;
        self.source_id = source_id;
        self.infinite = infinite;
        if shuffle {
            let mut rest = std::mem::take(&mut self.tracks);
            let first = (start_index < rest.len()).then(|| rest.remove(start_index));
            shuffle_in_place(&mut rest);
            if let Some(first) = first {
                rest.insert(0, first);
            }
            self.tracks = rest;
            self.current = (!self.tracks.is_empty()).then_some(0);
        } else {
            self.current = (start_index < self.tracks.len()).then_some(start_index);
        }
        match self.current {
            Some(index) => Step::Load(index),
            None => Step::Stop,
        }
    }

    pub fn stage(&mut self, tracks: Vec<Track>, start_index: usize) {
        self.original = tracks.clone();
        self.tracks = tracks;
        self.shuffle = false;
        self.source_id = None;
        self.infinite = false;
        self.current = (start_index < self.tracks.len()).then_some(start_index);
    }

    pub fn clear(&mut self) {
        let repeat = self.repeat;
        *self = Self { repeat, ..Self::default() };
    }

    pub fn insert(&mut self, tracks: Vec<Track>, play_next: bool) -> Step {
        if tracks.is_empty() {
            return Step::Stay;
        }
        let at = match (play_next, self.current) {
            (true, Some(current)) => current + 1,
            _ => self.tracks.len(),
        };
        let original_at = at.min(self.original.len());
        for (offset, track) in tracks.into_iter().enumerate() {
            self.tracks.insert(at + offset, track.clone());
            self.original.insert((original_at + offset).min(self.original.len()), track);
        }
        match self.current {
            Some(_) => Step::Stay,
            None => self.go(0),
        }
    }

    pub fn append(&mut self, tracks: Vec<Track>) {
        if tracks.is_empty() {
            return;
        }
        self.original.extend(tracks.iter().cloned());
        if !self.shuffle {
            self.tracks.extend(tracks);
            return;
        }
        match self.current {
            Some(current) if current + 1 < self.tracks.len() => {
                let mut upcoming = self.tracks.split_off(current + 1);
                upcoming.extend(tracks);
                shuffle_in_place(&mut upcoming);
                self.tracks.extend(upcoming);
            }
            Some(_) => self.tracks.extend(tracks),
            None => {
                self.tracks.extend(tracks);
                shuffle_in_place(&mut self.tracks);
            }
        }
    }

    pub fn clear_current(&mut self) {
        self.current = None;
    }

    pub fn remove(&mut self, index: usize) -> Step {
        if index >= self.tracks.len() {
            return Step::Stay;
        }
        let removed = self.tracks.remove(index);
        if let Some(position) = self.original.iter().position(|t| *t == removed) {
            self.original.remove(position);
        }
        match self.current {
            Some(current) if index < current => {
                self.current = Some(current - 1);
                Step::Stay
            }
            Some(current) if index == current => {
                if current < self.tracks.len() {
                    Step::Load(current)
                } else {
                    self.current = None;
                    Step::Stop
                }
            }
            _ => Step::Stay,
        }
    }

    pub fn move_item(&mut self, from: usize, to: usize) -> bool {
        if from >= self.tracks.len() || to >= self.tracks.len() || from == to {
            return false;
        }
        let item = self.tracks.remove(from);
        let insert_at = if from < to { to - 1 } else { to };
        self.tracks.insert(insert_at, item);
        self.current = self.current.map(|current| {
            if current == from {
                insert_at
            } else if from < current && current <= insert_at {
                current - 1
            } else if insert_at <= current && current < from {
                current + 1
            } else {
                current
            }
        });
        true
    }


    pub fn adopt_source(&mut self, source_id: String, infinite: bool) {
        self.source_id = Some(source_id);
        self.infinite = infinite;
    }

    pub fn set_repeat(&mut self, mode: RepeatMode) {
        self.repeat = mode;
    }

    pub fn set_shuffle(&mut self, on: bool) -> bool {
        if on == self.shuffle {
            return self.shuffle;
        }
        let playing = self.current_track().cloned();
        if on {
            let mut rest: Vec<Track> = self.tracks.iter().filter(|t| Some(*t) != playing.as_ref()).cloned().collect();
            shuffle_in_place(&mut rest);
            match playing {
                Some(track) => {
                    rest.insert(0, track);
                    self.current = Some(0);
                }
                None => self.current = None,
            }
            self.tracks = rest;
        } else {
            self.tracks = self.original.clone();
            self.current = playing
                .as_ref()
                .and_then(|p| self.tracks.iter().position(|t| t == p))
                .or((!self.tracks.is_empty()).then_some(0));
        }
        self.shuffle = on;
        self.shuffle
    }

    pub fn toggle_shuffle(&mut self) -> bool {
        self.set_shuffle(!self.shuffle)
    }


    pub fn swap_current(&mut self, previous: &VideoId, replacement: Track) -> bool {
        let Some(slot) = self.current.and_then(|i| self.tracks.get_mut(i)) else { return false };
        if slot.video_id != *previous {
            return false;
        }
        if let Some(original) = self.original.iter_mut().find(|t| t.video_id == *previous) {
            *original = replacement.clone();
        }
        *slot = replacement;
        true
    }

    pub fn refine_current(&mut self, refined: &Track) -> bool {
        let Some(slot) = self.current.and_then(|i| self.tracks.get_mut(i)) else { return false };
        if slot.video_id != refined.video_id {
            return false;
        }
        *slot = refined.clone();
        true
    }

    pub fn refresh_metadata(&mut self, fresh: &Track) -> bool {
        let mut found = false;
        for track in self.tracks.iter_mut().chain(self.original.iter_mut()).filter(|t| t.video_id == fresh.video_id) {
            found = true;
            if !fresh.title.is_empty() {
                track.title = fresh.title.clone();
            }
            if !fresh.artists.is_empty() {
                track.artists = fresh.artists.clone();
                track.artist = fresh.artist.clone();
            }
            if fresh.album.is_some() {
                track.album = fresh.album.clone();
            }
            if fresh.thumb.is_some() {
                track.thumb = fresh.thumb.clone();
            }
        }
        found
    }

    pub fn set_like_status(&mut self, video_id: &VideoId, status: LikeStatus) {
        for track in self.tracks.iter_mut().chain(self.original.iter_mut()) {
            if track.video_id == *video_id {
                track.like_status = status;
            }
        }
    }
}

fn shuffle_in_place(items: &mut [Track]) {
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15)
        | 1;
    for i in (1..items.len()).rev() {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let j = (seed % (i as u64 + 1)) as usize;
        items.swap(i, j);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: &str) -> Track {
        Track { video_id: VideoId(id.into()), title: id.into(), ..Track::default() }
    }

    #[test]
    fn the_audio_twin_takes_the_playing_slot_under_its_own_id() {
        let mut q = queue(&["video", "other"], 0);
        let mut twin = track("audio");
        twin.title = "Album master".into();

        assert!(q.swap_current(&VideoId("video".into()), twin));
        assert_eq!(q.current_track().map(|t| t.video_id.0.clone()), Some("audio".into()));
        assert_eq!(q.current_track().map(|t| t.title.clone()), Some("Album master".into()));
        assert_eq!(q.len(), 2, "the swap replaces the entry, it does not add one");

        assert!(!q.swap_current(&VideoId("video".into()), track("late")));
        assert_eq!(q.current_track().map(|t| t.video_id.0.clone()), Some("audio".into()));
    }

    fn queue(ids: &[&str], current: usize) -> Queue {
        let mut q = Queue::default();
        q.stage(ids.iter().map(|id| track(id)).collect(), current);
        q
    }

    fn ids(q: &Queue) -> Vec<String> {
        q.tracks().iter().map(|t| t.video_id.0.clone()).collect()
    }

    #[test]
    fn advance_walks_forward_then_stops() {
        let mut q = queue(&["a", "b"], 0);
        assert_eq!(q.advance(), Step::Load(1));
        assert_eq!(q.advance(), Step::Stop);
        assert_eq!(q.current(), None);
    }

    #[test]
    fn advance_wraps_under_repeat_all_but_not_repeat_track() {
        let mut q = queue(&["a", "b"], 1);
        q.set_repeat(RepeatMode::All);
        assert_eq!(q.advance(), Step::Load(0));
        q.set_repeat(RepeatMode::Track);
        q.jump(1);
        assert_eq!(q.advance(), Step::Stop);
    }

    #[test]
    fn a_finished_track_repeats_itself_under_repeat_track() {
        let mut q = queue(&["a", "b"], 0);
        q.set_repeat(RepeatMode::Track);
        assert_eq!(q.finished(), Step::Load(0));
        q.set_repeat(RepeatMode::Off);
        assert_eq!(q.finished(), Step::Load(1));
    }

    #[test]
    fn an_exhausted_radio_asks_for_more_instead_of_stopping() {
        let mut q = Queue::default();
        q.replace(vec![track("a")], 0, false, Some("RDAMVMa".into()), true);
        assert_eq!(q.advance(), Step::Extend);
        assert_eq!(q.current(), Some(0), "an extendable queue keeps its position");
    }

    #[test]
    fn back_restarts_past_the_threshold_and_at_the_first_track() {
        let mut q = queue(&["a", "b"], 1);
        assert_eq!(q.back(PREVIOUS_RESTART_THRESHOLD + 0.1), Step::Restart);
        assert_eq!(q.back(0.5), Step::Load(0));
        assert_eq!(q.back(0.5), Step::Restart, "nothing before the first track");
    }

    #[test]
    fn bounds_match_what_advance_and_back_will_do() {
        let mut q = queue(&["a", "b"], 0);
        assert_eq!(q.bounds(0.0), Bounds { can_next: true, can_previous: false });
        assert!(q.bounds(PREVIOUS_RESTART_THRESHOLD + 1.0).can_previous, "restart counts as going back");
        q.jump(1);
        assert_eq!(q.bounds(0.0), Bounds { can_next: false, can_previous: true });
        q.set_repeat(RepeatMode::All);
        assert!(q.bounds(0.0).can_next, "repeat-all always has a next");
    }

    #[test]
    fn a_failed_track_does_not_wrap_or_extend() {
        let mut q = Queue::default();
        q.replace(vec![track("a"), track("b")], 1, false, Some("RDAMVMa".into()), true);
        q.set_repeat(RepeatMode::All);
        assert_eq!(q.failed_current(), Step::Stop);
        assert_eq!(q.current(), None);
    }

    #[test]
    fn removing_the_playing_track_loads_what_slid_into_its_place() {
        let mut q = queue(&["a", "b", "c"], 1);
        assert_eq!(q.remove(1), Step::Load(1));
        assert_eq!(ids(&q), ["a", "c"]);
        assert_eq!(q.remove(0), Step::Stay, "removing behind the play-head shifts the index");
        assert_eq!(q.current(), Some(0));
        assert_eq!(q.remove(0), Step::Stop, "the last track leaves nothing to play");
    }

    #[test]
    fn moving_a_row_carries_the_play_head_with_it() {
        let mut q = queue(&["a", "b", "c"], 0);
        assert!(q.move_item(0, 2));
        assert_eq!(ids(&q), ["b", "a", "c"]);
        assert_eq!(q.current(), Some(1), "the playing track kept playing");
        assert!(!q.move_item(0, 0));
        assert!(!q.move_item(9, 0));
    }

    #[test]
    fn insert_plays_next_without_disturbing_the_current_track() {
        let mut q = queue(&["a", "b"], 0);
        assert_eq!(q.insert(vec![track("x")], true), Step::Stay);
        assert_eq!(ids(&q), ["a", "x", "b"]);
        assert_eq!(q.current(), Some(0));
        assert_eq!(q.insert(vec![track("z")], false), Step::Stay);
        assert_eq!(ids(&q), ["a", "x", "b", "z"]);
    }

    #[test]
    fn insert_into_an_empty_queue_starts_playing() {
        let mut q = Queue::default();
        assert_eq!(q.insert(vec![track("a")], false), Step::Load(0));
    }

    #[test]
    fn shuffle_keeps_the_playing_track_and_restores_the_order() {
        let mut q = queue(&["a", "b", "c", "d", "e"], 2);
        let before = ids(&q);
        assert!(q.set_shuffle(true));
        assert_eq!(q.current(), Some(0));
        assert_eq!(q.current_track().map(|t| t.video_id.0.clone()), Some("c".to_owned()));
        assert_eq!(q.len(), 5);
        assert!(!q.set_shuffle(false));
        assert_eq!(ids(&q), before, "the original order comes back");
        assert_eq!(q.current_track().map(|t| t.video_id.0.clone()), Some("c".to_owned()));
    }

    #[test]
    fn a_radio_extension_never_reorders_what_already_played() {
        let mut q = queue(&["a", "b", "c"], 1);
        q.set_shuffle(true);
        let played: Vec<String> = ids(&q)[..=q.current().unwrap()].to_vec();
        q.append(vec![track("x"), track("y")]);
        assert_eq!(&ids(&q)[..played.len()], played.as_slice());
        assert_eq!(q.len(), 5);
    }

    #[test]
    fn armable_next_arms_the_same_track_under_repeat_track() {
        let mut q = queue(&["a", "b"], 0);
        assert_eq!(q.armable_next(), Some(1));
        q.set_repeat(RepeatMode::Track);
        assert_eq!(q.armable_next(), Some(0));
        q.set_repeat(RepeatMode::All);
        q.jump(1);
        assert_eq!(q.armable_next(), Some(0));
        q.set_repeat(RepeatMode::Off);
        assert_eq!(q.armable_next(), None);
    }

    #[test]
    fn refine_only_touches_the_track_it_names() {
        let mut q = queue(&["a", "b"], 0);
        let mut refined = track("a");
        refined.title = "Proper title".into();
        assert!(q.refine_current(&refined));
        assert_eq!(q.current_track().unwrap().title, "Proper title");
        assert!(!q.refine_current(&track("b")), "a stale resolution is ignored");
    }

    #[test]
    fn clear_keeps_repeat_but_nothing_else() {
        let mut q = queue(&["a"], 0);
        q.set_repeat(RepeatMode::All);
        q.clear();
        assert!(q.is_empty());
        assert_eq!(q.current(), None);
        assert_eq!(q.repeat(), RepeatMode::All);
    }
}
