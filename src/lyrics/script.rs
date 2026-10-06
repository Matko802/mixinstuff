
pub fn is_cjk_char(ch: char) -> bool {
    matches!(ch as u32, 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF)
}

pub fn needs_reading(ch: char) -> bool {
    matches!(ch as u32, 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF)
}

pub fn is_non_latin_char(ch: char) -> bool {
    matches!(ch as u32, 0x0400..=0x04FF | 0x0590..=0x05FF | 0x0600..=0x06FF | 0x0E00..=0x0E7F | 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF)
}

pub fn is_kana(ch: char) -> bool {
    matches!(ch as u32, 0x3040..=0x30FF)
}

pub fn is_han(ch: char) -> bool {
    matches!(ch as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF)
}

pub fn space_between(left: &str, right: &str) -> bool {
    match (left.chars().next_back(), right.chars().next()) {
        (Some(l), Some(r)) => !(is_cjk_char(l) && is_cjk_char(r)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spaces_go_between_words_but_not_between_syllables() {
        assert!(space_between("hello", "world"));
        assert!(!space_between("千", "本"));
        assert!(!space_between("사랑", "해"));
        assert!(space_between("桜", "night"));
        assert!(space_between("night", "桜"));
        assert!(!space_between("", "a"));
        assert!(!space_between("a", ""));
    }

    #[test]
    fn script_classes() {
        assert!(is_non_latin_char('д') && !needs_reading('д'));
        assert!(is_non_latin_char('한') && !needs_reading('한') && is_cjk_char('한'));
        assert!(needs_reading('桜') && is_han('桜') && !is_kana('桜'));
        assert!(needs_reading('ミ') && is_kana('ミ'));
        assert!(!is_non_latin_char('a') && !is_non_latin_char('é'));
    }
}
