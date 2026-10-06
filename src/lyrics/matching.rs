
use std::sync::LazyLock;

use regex::Regex;

use super::script::is_non_latin_char;

const VERSION_MARKERS: [&str; 17] = [
    "live", "remix", "instrumental", "acoustic", "karaoke", "cover", "nightcore", "sped up", "spedup", "slowed", "reverb", "8d audio", "demo", "rehearsal", "unplugged", "orchestral",
    "piano version",
];

const GENERIC_TITLE_WORDS: [&str; 13] = ["intro", "outro", "interlude", "skit", "prelude", "overture", "untitled", "bonus", "bonustrack", "instrumental", "reprise", "epilogue", "prologue"];

const MATCH_DURATION_SLACK: u32 = 20;

const DURATION_TOLERANCE: u32 = 5;

static ARTIST_SPLIT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\s*(?:,|&|;|/|\bfeat\.?\b|\bft\.?\b|\bfeaturing\b|\bwith\b|\bx\b|\band\b|\bund\b)\s*").unwrap());
static NOT_GENERIC_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9 ]").unwrap());
static TRACK_NUMBER_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^track\s*0*\d+$").unwrap());
static LENTICULAR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[【〔〖][^】〕〗]*[】〕〗]").unwrap());
static SPACES_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
static ANY_PARENS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*[\(（\[][^)）\]]*[\)）\]]\s*").unwrap());
static TRAILING_PARENS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*[\(\[][^)\]]*[\)\]]\s*$").unwrap());

pub trait Candidate {
    fn name(&self) -> &str;
    fn artist(&self) -> &str;
    fn duration(&self) -> u32;
}

pub fn norm_for_match(text: &str) -> String {
    text.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

pub fn titles_match(expected: &str, found: &str) -> bool {
    contains_either_way(&norm_for_match(expected), &norm_for_match(found))
}

pub fn artist_matches(expected: &str, found: &str) -> bool {
    contains_either_way(&norm_for_match(expected), &norm_for_match(found))
}

fn contains_either_way(a: &str, b: &str) -> bool {
    !a.is_empty() && !b.is_empty() && (a.contains(b) || b.contains(a))
}

pub fn version_mismatch(query_title: &str, candidate_name: &str) -> bool {
    let query = query_title.to_lowercase();
    let candidate = candidate_name.to_lowercase();
    VERSION_MARKERS.iter().any(|m| candidate.contains(m) && !query.contains(m))
}

pub fn duration_ok(expected: u32, found: u32) -> bool {
    expected == 0 || found == 0 || expected.abs_diff(found) <= DURATION_TOLERANCE
}

pub fn split_artists(artist: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in ARTIST_SPLIT_RE.split(artist).map(str::trim).chain([artist.trim()]) {
        if !part.is_empty() && !out.iter().any(|seen| seen == part) {
            out.push(part.to_owned());
        }
    }
    out
}

pub fn is_generic_title(title: &str) -> bool {
    let lowered = title.to_lowercase();
    let norm = NOT_GENERIC_RE.replace_all(&lowered, "");
    let norm = norm.trim();
    if norm.is_empty() {
        return true;
    }
    let compact = norm.replace(' ', "");
    GENERIC_TITLE_WORDS.contains(&compact.as_str()) || TRACK_NUMBER_RE.is_match(norm)
}

fn script_halves(text: &str) -> Vec<String> {
    let runs: Vec<&str> = text.split_whitespace().collect();
    if runs.len() < 2 {
        return Vec::new();
    }
    let (native, latin): (Vec<&str>, Vec<&str>) = runs.iter().partition(|run| run.chars().any(is_non_latin_char));
    if native.is_empty() || latin.is_empty() {
        return Vec::new();
    }
    vec![native.join(" "), latin.join(" ")]
}

pub fn title_variants(title: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |text: &str| {
        let text = text.trim();
        if !text.is_empty() && !out.iter().any(|seen| seen == text) {
            out.push(text.to_owned());
        }
    };
    if title.is_empty() {
        return out;
    }
    add(title);

    let debracketed = LENTICULAR_RE.replace_all(title, " ");
    let debracketed = SPACES_RE.replace_all(&debracketed, " ");
    let debracketed = debracketed.trim();
    if !debracketed.is_empty() && debracketed != title {
        add(debracketed);
        let no_parens = ANY_PARENS_RE.replace_all(debracketed, " ");
        let no_parens = SPACES_RE.replace_all(&no_parens, " ");
        let no_parens = no_parens.trim();
        add(no_parens);
        for half in script_halves(no_parens) {
            add(&half);
        }
    }

    if title.contains(" - ") {
        let parts: Vec<&str> = title.split(" - ").map(str::trim).filter(|p| !p.is_empty()).collect();
        for part in parts.iter().rev() {
            add(part);
        }
    }

    add(&TRAILING_PARENS_RE.replace(title, ""));
    out
}

pub fn candidate_matches(title: &str, artists: &[String], duration: u32, candidate: &impl Candidate) -> bool {
    if !duration_ok(duration, candidate.duration()) {
        return false;
    }
    if version_mismatch(title, candidate.name()) {
        return false;
    }
    if artists.iter().any(|a| artist_matches(a, candidate.artist())) {
        return true;
    }
    if !titles_match(title, candidate.name()) {
        return false;
    }
    artists.is_empty() || (duration != 0 && candidate.duration() != 0)
}

pub fn prefer_artist_matches<C: Candidate>(items: Vec<C>, artists: &[String]) -> Vec<C> {
    if artists.is_empty() || !items.iter().any(|it| artists.iter().any(|a| artist_matches(a, it.artist()))) {
        return items;
    }
    items.into_iter().filter(|it| artists.iter().any(|a| artist_matches(a, it.artist()))).collect()
}

pub fn gate<C: Candidate>(items: Vec<C>, title: &str, artist: &str, duration: u32) -> Vec<C> {
    let artists = split_artists(artist);
    let gated: Vec<C> = items.into_iter().filter(|c| candidate_matches(title, &artists, duration, c)).collect();
    prefer_artist_matches(gated, &artists)
}

pub fn rank_matches<C: Candidate>(items: Vec<C>, title: &str, artist: &str, duration: u32) -> Vec<C> {
    let artists = split_artists(artist);
    let plausible = |item: &C| {
        if titles_match(title, item.name()) {
            return true;
        }
        duration != 0 && item.duration() != 0 && duration.abs_diff(item.duration()) <= MATCH_DURATION_SLACK
    };
    let score = |item: &C| {
        let mut value = 0i32;
        if duration != 0 && item.duration() != 0 {
            value += match duration.abs_diff(item.duration()) {
                0..=2 => 100,
                3..=5 => 60,
                6..=15 => 20,
                _ => 0,
            };
        }
        if artists.iter().any(|a| artist_matches(a, item.artist())) {
            value += 50;
        }
        if titles_match(title, item.name()) {
            value += 30;
        }
        if version_mismatch(title, item.name()) {
            value -= 40;
        }
        -value
    };
    let mut items: Vec<C> = items.into_iter().filter(plausible).collect();
    items.sort_by_key(score);
    items
}

pub fn match_detail(artist: &str, duration: u32) -> String {
    let mut bits: Vec<String> = Vec::new();
    if !artist.is_empty() {
        bits.push(artist.to_owned());
    }
    if duration != 0 {
        bits.push(format!("{}:{:02}", duration / 60, duration % 60));
    }
    bits.join(" · ")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    pub struct Hit(pub &'static str, pub &'static str, pub u32);

    impl Candidate for Hit {
        fn name(&self) -> &str {
            self.0
        }
        fn artist(&self) -> &str {
            self.1
        }
        fn duration(&self) -> u32 {
            self.2
        }
    }

    fn names(hits: &[Hit]) -> Vec<&'static str> {
        hits.iter().map(|h| h.0).collect()
    }

    #[test]
    fn normalizing_keeps_every_script() {
        assert_eq!(norm_for_match("Daft-Punk!"), "daftpunk");
        assert_eq!(norm_for_match("初音ミク"), "初音ミク");
        assert!(artist_matches("初音ミク", "初音ミク"));
        assert!(artist_matches("Daft Punk", "daft-punk feat. Pharrell"));
        assert!(!artist_matches("", "Daft Punk"));
        assert!(!artist_matches("Daft Punk", "Justice"));
    }

    #[test]
    fn titles_match_by_containment() {
        assert!(titles_match("Lemon", "Lemon (Special Edition)"));
        assert!(titles_match("Lemon (Special Edition)", "lemon"));
        assert!(!titles_match("Lemon", "Peace Sign"));
        assert!(!titles_match("", "Lemon"));
    }

    #[test]
    fn a_different_take_is_a_mismatch_unless_asked_for() {
        assert!(version_mismatch("Creep", "Creep (Live at Glastonbury)"));
        assert!(version_mismatch("Creep", "Creep - Acoustic"));
        assert!(!version_mismatch("Creep (Live)", "Creep (Live at Glastonbury)"));
        assert!(!version_mismatch("Creep", "Creep (2009 Remaster)"));
    }

    #[test]
    fn duration_gate() {
        assert!(duration_ok(200, 205));
        assert!(!duration_ok(200, 206));
        assert!(duration_ok(0, 999));
        assert!(duration_ok(200, 0));
    }

    #[test]
    fn artists_split_on_every_joiner_and_keep_the_whole() {
        assert_eq!(split_artists("Hatsune Miku, WhiteFlame"), ["Hatsune Miku", "WhiteFlame", "Hatsune Miku, WhiteFlame"]);
        assert_eq!(split_artists("CTS feat. 初音ミク"), ["CTS", ". 初音ミク", "CTS feat. 初音ミク"]);
        assert!(split_artists("CTS feat. 初音ミク").iter().any(|a| artist_matches(a, "初音ミク")));
        assert_eq!(split_artists("Simon and Garfunkel"), ["Simon", "Garfunkel", "Simon and Garfunkel"]);
        assert_eq!(split_artists("A x B & C / D; E"), ["A", "B", "C", "D", "E", "A x B & C / D; E"]);
        assert_eq!(split_artists("Brandy"), ["Brandy"]);
        assert_eq!(split_artists("Alexander"), ["Alexander"]);
        assert!(split_artists("").is_empty());
    }

    #[test]
    fn generic_titles() {
        for title in ["Intro", "OUTRO", "Bonus Track", "Track 01", "track7", "", "!!!", "千本桜"] {
            assert!(is_generic_title(title), "{title}");
        }
        for title in ["Introduction to Love", "Track Star", "Lemon", "千本桜 Senbonzakura"] {
            assert!(!is_generic_title(title), "{title}");
        }
    }

    #[test]
    fn variants_of_a_plain_title() {
        assert_eq!(title_variants("Lemon"), ["Lemon"]);
        assert!(title_variants("").is_empty());
    }

    #[test]
    fn variants_put_the_translation_first() {
        assert_eq!(title_variants("イガク - Medicine"), ["イガク - Medicine", "Medicine", "イガク"]);
    }

    #[test]
    fn variants_drop_a_trailing_parenthetical() {
        assert_eq!(title_variants("Popular (feat. Playboi Carti)"), ["Popular (feat. Playboi Carti)", "Popular"]);
        assert_eq!(title_variants("Song [Remastered 2009]"), ["Song [Remastered 2009]", "Song"]);
        assert_eq!(title_variants("U-Turn"), ["U-Turn"]);
    }

    #[test]
    fn variants_strip_lenticular_credits_and_split_the_scripts() {
        let got = title_variants("【初音ミク(40㍍)】 トリノコシティ Torinoko City【オリジナル】");
        assert_eq!(got, ["【初音ミク(40㍍)】 トリノコシティ Torinoko City【オリジナル】", "トリノコシティ Torinoko City", "トリノコシティ", "Torinoko City"]);
    }

    #[test]
    fn the_gate_wants_an_artist_or_a_corroborated_title() {
        let artists = split_artists("The Weeknd");
        assert!(candidate_matches("千本桜", &split_artists("Hatsune Miku"), 245, &Hit("Senbonzakura", "WhiteFlame feat. Hatsune Miku", 244)));
        assert!(candidate_matches("Popular", &artists, 215, &Hit("Popular", "Ariana Grande", 214)));
        assert!(!candidate_matches("Popular", &artists, 0, &Hit("Popular", "Ariana Grande", 214)));
        assert!(candidate_matches("Popular", &[], 0, &Hit("Popular", "Ariana Grande", 214)));
        assert!(!candidate_matches("Popular", &artists, 215, &Hit("Popular", "The Weeknd", 260)));
        assert!(!candidate_matches("Popular", &artists, 215, &Hit("Popular (Live)", "The Weeknd", 215)));
        assert!(!candidate_matches("千本桜", &split_artists("Hatsune Miku"), 245, &Hit("Sakura Biyori and Time Machine", "Someone", 245)));
    }

    #[test]
    fn a_recognised_artist_narrows_the_list() {
        let hits = vec![Hit("Popular", "Ariana Grande", 214), Hit("Popular (feat. Playboi Carti)", "The Weeknd", 215)];
        assert_eq!(names(&prefer_artist_matches(hits.clone(), &split_artists("The Weeknd"))), ["Popular (feat. Playboi Carti)"]);
        assert_eq!(prefer_artist_matches(hits.clone(), &split_artists("Nobody")).len(), 2);
        assert_eq!(prefer_artist_matches(hits.clone(), &[]).len(), 2);
        assert_eq!(names(&gate(hits, "Popular", "The Weeknd", 215)), ["Popular (feat. Playboi Carti)"]);
    }

    #[test]
    fn the_match_list_is_looser_but_not_blind() {
        let hits = vec![
            Hit("Rips in Jeans", "Niko B", 150),
            Hit("Why's this dealer? (Live)", "Niko B", 180),
            Hit("Why's this dealer?", "Niko B", 171),
            Hit("Something Else", "Other", 175),
            Hit("Why's this dealer?", "Tribute Band", 240),
        ];
        let ranked = rank_matches(hits, "Why's this dealer?", "Niko B", 172);
        assert_eq!(names(&ranked), ["Why's this dealer?", "Why's this dealer? (Live)", "Something Else", "Why's this dealer?"]);
        assert_eq!(ranked[0].2, 171);
        assert_eq!(ranked[3].1, "Tribute Band");
        assert!(rank_matches(vec![Hit("Rips in Jeans", "Niko B", 150)], "Why's this dealer?", "Niko B", 0).is_empty());
    }

    #[test]
    fn match_detail_shows_what_is_known() {
        assert_eq!(match_detail("Kenshi Yonezu", 255), "Kenshi Yonezu · 4:15");
        assert_eq!(match_detail("", 65), "1:05");
        assert_eq!(match_detail("Kenshi Yonezu", 0), "Kenshi Yonezu");
        assert_eq!(match_detail("", 0), "");
    }
}
