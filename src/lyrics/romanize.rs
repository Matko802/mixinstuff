
use regex::Regex;
use std::sync::LazyLock;

use pinyin::ToPinyin;
use unicode_normalization::UnicodeNormalization;

use super::model::{LyricLine, LyricsResult};
use super::script::{is_han, is_kana, needs_reading};

const HANGUL_INITIALS: [&str; 19] = ["g", "kk", "n", "d", "tt", "r", "m", "b", "pp", "s", "ss", "", "j", "jj", "ch", "k", "t", "p", "h"];
const HANGUL_MEDIALS: [&str; 21] = ["a", "ae", "ya", "yae", "eo", "e", "yeo", "ye", "o", "wa", "wae", "oe", "yo", "u", "wo", "we", "wi", "yu", "eu", "ui", "i"];
const HANGUL_FINALS: [&str; 28] = ["", "k", "k", "ks", "n", "nj", "nh", "t", "l", "lk", "lm", "lb", "ls", "lt", "lp", "lh", "m", "p", "ps", "s", "ss", "ng", "j", "ch", "k", "t", "p", "h"];

const CYRILLIC_VOWELS: &str = "аеёиоуыэюяіїєaeiouy";

const ROMANIZATION_CONSISTENCY_FLOOR: f64 = 0.5;

const MAX_JOIN: usize = 4;

static PARENTHETICAL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[（(\[][^）)\]]*[）)\]]").unwrap());

pub fn romanize_korean(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut converted = false;
    for ch in text.chars() {
        let code = ch as u32;
        if (0xAC00..=0xD7A3).contains(&code) {
            let i = (code - 0xAC00) as usize;
            out.push_str(HANGUL_INITIALS[i / 588]);
            out.push_str(HANGUL_MEDIALS[(i % 588) / 28]);
            out.push_str(HANGUL_FINALS[i % 28]);
            converted = true;
        } else {
            out.push(ch);
        }
    }
    converted.then_some(out)
}

fn cyrillic_value(lower: char) -> Option<&'static str> {
    Some(match lower {
        'а' => "a",
        'б' => "b",
        'в' => "v",
        'г' => "g",
        'д' => "d",
        'е' => "e",
        'ё' => "yo",
        'ж' => "zh",
        'з' => "z",
        'и' => "i",
        'й' => "y",
        'к' => "k",
        'л' => "l",
        'м' => "m",
        'н' => "n",
        'о' => "o",
        'п' => "p",
        'р' => "r",
        'с' => "s",
        'т' => "t",
        'у' => "u",
        'ф' => "f",
        'х' => "kh",
        'ц' => "ts",
        'ч' => "ch",
        'ш' => "sh",
        'щ' => "shch",
        'ъ' => "",
        'ы' => "y",
        'ь' => "",
        'э' => "e",
        'ю' => "yu",
        'я' => "ya",
        'і' => "i",
        'ї' => "yi",
        'є' => "ye",
        'ґ' => "g",
        'ў' => "w",
        'ѓ' => "gj",
        'ќ' => "kj",
        'ђ' => "dj",
        'ћ' => "c",
        'ј' => "j",
        'љ' => "lj",
        'њ' => "nj",
        'џ' => "dz",
        'ѕ' => "dz",
        'һ' => "h",
        'ә' => "a",
        'ө' => "o",
        'ү' => "u",
        'ң' => "ng",
        'қ' => "q",
        'ғ' => "gh",
        'ұ' => "u",
        _ => return None,
    })
}

fn lower(ch: char) -> char {
    ch.to_lowercase().next().unwrap_or(ch)
}

pub fn romanize_cyrillic(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut converted = false;
    for (i, &ch) in chars.iter().enumerate() {
        let low = lower(ch);
        let Some(mut roman) = cyrillic_value(low) else {
            out.push(ch);
            continue;
        };
        converted = true;
        if low == 'е' {
            let prev = i.checked_sub(1).map(|p| lower(chars[p]));
            let opens = prev.is_none_or(|p| !p.is_alphabetic() || CYRILLIC_VOWELS.contains(p) || p == 'ъ' || p == 'ь');
            roman = if opens { "ye" } else { "e" };
        }
        if roman.is_empty() || !ch.is_uppercase() {
            out.push_str(roman);
            continue;
        }
        let shouting = |c: Option<&char>| c.is_some_and(|c| c.is_alphabetic() && c.is_uppercase());
        if shouting(i.checked_sub(1).and_then(|p| chars.get(p))) || shouting(chars.get(i + 1)) {
            out.push_str(&roman.to_uppercase());
        } else {
            let mut letters = roman.chars();
            if let Some(first) = letters.next() {
                out.extend(first.to_uppercase());
                out.push_str(letters.as_str());
            }
        }
    }
    converted.then_some(out)
}

pub fn romanize_locally(text: &str) -> Option<String> {
    romanize_korean(text).or_else(|| romanize_cyrillic(text))
}

pub fn romanize_with_dictionary(text: &str) -> Option<String> {
    let out = if text.chars().any(is_kana) {
        kakasi::convert(text).romaji.split_whitespace().collect::<Vec<_>>().join(" ")
    } else if text.chars().any(is_han) {
        pinyin_of(text)
    } else {
        return None;
    };
    let out = out.trim();
    (!out.is_empty() && out != text).then(|| out.to_owned())
}

fn pinyin_of(text: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    let mut run = String::new();
    for (ch, reading) in text.chars().zip(text.to_pinyin()) {
        match reading {
            Some(reading) => {
                if !run.trim().is_empty() {
                    words.push(run.trim().to_owned());
                }
                run.clear();
                words.push(reading.with_tone().to_owned());
            }
            None => run.push(ch),
        }
    }
    if !run.trim().is_empty() {
        words.push(run.trim().to_owned());
    }
    words.join(" ")
}

pub fn fill_romanization_gaps(result: &mut LyricsResult) {
    let want: Vec<usize> = result.lines.iter().enumerate().filter(|(_, l)| l.text.chars().any(needs_reading)).map(|(i, _)| i).collect();
    if want.is_empty() {
        return;
    }
    let have = want.iter().filter(|&&i| result.lines[i].has_romanization()).count();
    let replace_all = (have as f64) < want.len() as f64 * ROMANIZATION_CONSISTENCY_FLOOR;

    let mut filled = 0;
    for &i in &want {
        let line = &mut result.lines[i];
        if line.has_romanization() && !replace_all {
            continue;
        }
        if let Some(roman) = romanize_with_dictionary(&line.text) {
            line.romanization = Some(roman);
            filled += 1;
        }
    }
    if filled > 0 {
        tracing::debug!(filled, of = want.len(), regenerated = replace_all, "romanization generated from the dictionary");
    }
}

pub fn norm_lyric_text(text: &str) -> String {
    let folded: String = text.nfkc().collect();
    PARENTHETICAL_RE.replace_all(&folded, "").to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

pub fn merge_romanization_by_text(lines: &mut [LyricLine], source: &[LyricLine]) -> usize {
    let pairs: Vec<(String, String)> = source
        .iter()
        .map(|l| (norm_lyric_text(&l.text), l.romanization.as_deref().unwrap_or("").trim().to_owned()))
        .filter(|(text, roman)| !text.is_empty() && !roman.is_empty())
        .collect();
    if pairs.is_empty() {
        return 0;
    }

    let mut matched = 0;
    for line in lines.iter_mut().filter(|l| !l.has_romanization()) {
        let key = norm_lyric_text(&line.text);
        if key.is_empty() {
            continue;
        }
        if let Some((_, roman)) = pairs.iter().find(|(text, _)| *text == key) {
            line.romanization = Some(roman.clone());
            matched += 1;
            continue;
        }
        if let Some(joined) = joined_run(&key, &pairs) {
            line.romanization = Some(joined);
            matched += 1;
        }
    }
    matched
}

fn joined_run(key: &str, pairs: &[(String, String)]) -> Option<String> {
    for start in 0..pairs.len() {
        if !key.starts_with(pairs[start].0.as_str()) {
            continue;
        }
        let mut acc = String::new();
        let mut parts: Vec<&str> = Vec::new();
        for (text, roman) in pairs.iter().skip(start).take(MAX_JOIN) {
            acc.push_str(text);
            parts.push(roman);
            if acc == key {
                return Some(parts.join(" "));
            }
            if !key.starts_with(acc.as_str()) {
                break;
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, roman: Option<&str>) -> LyricLine {
        LyricLine { romanization: roman.map(str::to_owned), ..LyricLine::new(Some(0.0), text) }
    }

    #[test]
    fn korean_decomposes_arithmetically() {
        assert_eq!(romanize_korean("사랑해").as_deref(), Some("saranghae"));
        assert_eq!(romanize_korean("한국 love").as_deref(), Some("hanguk love"));
        assert_eq!(romanize_korean("hello"), None);
        assert_eq!(romanize_korean(""), None);
    }

    #[test]
    fn cyrillic_follows_the_ye_rule_and_keeps_case() {
        assert_eq!(romanize_cyrillic("Группа крови").as_deref(), Some("Gruppa krovi"));
        assert_eq!(romanize_cyrillic("если").as_deref(), Some("yesli"));
        assert_eq!(romanize_cyrillic("поезд").as_deref(), Some("poyezd"));
        assert_eq!(romanize_cyrillic("лето").as_deref(), Some("leto"));
        assert_eq!(romanize_cyrillic("объезд").as_deref(), Some("obyezd"));
        assert_eq!(romanize_cyrillic("ДОЖДЬ").as_deref(), Some("DOZHD"));
        assert_eq!(romanize_cyrillic("Жизнь").as_deref(), Some("Zhizn"));
        assert_eq!(romanize_cyrillic("Щ").as_deref(), Some("Shch"));
        assert_eq!(romanize_cyrillic("Їжак").as_deref(), Some("Yizhak"));
        assert_eq!(romanize_cyrillic("rain"), None);
    }

    #[test]
    fn local_romanization_prefers_korean_then_cyrillic() {
        assert_eq!(romanize_locally("비").as_deref(), Some("bi"));
        assert_eq!(romanize_locally("мир").as_deref(), Some("mir"));
        assert_eq!(romanize_locally("千本桜"), None);
    }

    #[test]
    fn the_dictionary_reads_japanese_and_chinese() {
        let japanese = romanize_with_dictionary("こんにちは世界").unwrap();
        assert!(japanese.contains("sekai"), "{japanese}");
        assert_eq!(romanize_with_dictionary("你好 ICBM 世界").as_deref(), Some("nǐ hǎo ICBM shì jiè"));
        assert_eq!(romanize_with_dictionary("hello"), None);
        assert_eq!(romanize_with_dictionary("사랑"), None);
        assert_eq!(romanize_with_dictionary(""), None);
    }

    #[test]
    fn gaps_are_filled_around_what_the_provider_gave() {
        let mut result = LyricsResult::from_lines(vec![line("你好", Some("official one")), line("世界", Some("official two")), line("我们", None), line("la la", None)], "X");
        fill_romanization_gaps(&mut result);
        assert_eq!(result.lines[0].romanization.as_deref(), Some("official one"));
        assert_eq!(result.lines[2].romanization.as_deref(), Some("wǒ men"));
        assert_eq!(result.lines[3].romanization, None);
    }

    #[test]
    fn thin_provider_coverage_is_regenerated_whole() {
        let mut result = LyricsResult::from_lines(vec![line("你好", Some("official")), line("世界", None), line("我们", None)], "X");
        fill_romanization_gaps(&mut result);
        assert_eq!(result.lines[0].romanization.as_deref(), Some("nǐ hǎo"));
        assert_eq!(result.lines[1].romanization.as_deref(), Some("shì jiè"));
    }

    #[test]
    fn latin_lyrics_are_left_alone() {
        let mut result = LyricsResult::from_lines(vec![line("hello", None)], "X");
        let before = result.clone();
        fill_romanization_gaps(&mut result);
        assert_eq!(result, before);
    }

    #[test]
    fn lyric_text_is_folded_for_comparison() {
        assert_eq!(norm_lyric_text("悪霊退散　ＩＣＢＭ"), norm_lyric_text("悪霊退散 ICBM"));
        assert_eq!(norm_lyric_text("磊々落々(らいらいらくらく)"), norm_lyric_text("磊々落々"));
        assert_eq!(norm_lyric_text("Hello, World!"), "helloworld");
        assert_eq!(norm_lyric_text(""), "");
    }

    #[test]
    fn readings_merge_by_text_not_by_time() {
        let mut ours = vec![line("千本桜　夜ニ紛レ", None), line("la la", None), line("君ノ声モ届カナイヨ", Some("kept"))];
        let theirs = vec![
            LyricLine { romanization: Some("kimi no koe".into()), ..LyricLine::new(Some(90.0), "君ノ声モ届カナイヨ") },
            LyricLine { romanization: Some(" senbonzakura yoru ni magire ".into()), ..LyricLine::new(Some(50.0), "千本桜 夜ニ紛レ") },
        ];
        assert_eq!(merge_romanization_by_text(&mut ours, &theirs), 1);
        assert_eq!(ours[0].romanization.as_deref(), Some("senbonzakura yoru ni magire"));
        assert_eq!(ours[1].romanization, None);
        assert_eq!(ours[2].romanization.as_deref(), Some("kept"));
    }

    #[test]
    fn one_of_our_lines_can_be_several_of_theirs() {
        let mut ours = vec![line("千本桜夜ニ紛レ君ノ声", None), line("千本桜別", None)];
        let theirs = vec![line("千本桜", Some("senbonzakura")), line("夜ニ紛レ", Some("yoru ni magire")), line("君ノ声", Some("kimi no koe"))];
        assert_eq!(merge_romanization_by_text(&mut ours, &theirs), 1);
        assert_eq!(ours[0].romanization.as_deref(), Some("senbonzakura yoru ni magire kimi no koe"));
        assert_eq!(ours[1].romanization, None);
    }

    #[test]
    fn a_wrong_song_contributes_nothing() {
        let mut ours = vec![line("千本桜", None)];
        let theirs = vec![line("夢ならばどれほどよかったでしょう", Some("yume naraba"))];
        assert_eq!(merge_romanization_by_text(&mut ours, &theirs), 0);
        assert_eq!(merge_romanization_by_text(&mut ours, &[line("千本桜", None)]), 0);
    }
}
