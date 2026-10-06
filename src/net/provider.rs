use crate::paths::Paths;

pub const YOUTUBE: &str = "youtube";
pub const SOUNDCLOUD: &str = "soundcloud";

const YTM_ENABLED_PREF: &str = "provider_ytmusic_enabled";
const SC_ENABLED_PREF: &str = "provider_soundcloud_enabled";
const ACTIVE_PREF: &str = "music_provider_active";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    YouTube,
    SoundCloud,
}

impl Provider {
    pub fn all() -> [Provider; 2] {
        [Provider::YouTube, Provider::SoundCloud]
    }

    pub fn id(self) -> &'static str {
        match self {
            Provider::YouTube => YOUTUBE,
            Provider::SoundCloud => SOUNDCLOUD,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Provider::YouTube => "YouTube Music",
            Provider::SoundCloud => "SoundCloud",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Provider::YouTube => "youtube-music-symbolic",
            Provider::SoundCloud => "soundcloud-symbolic",
        }
    }

    pub fn parse(id: &str) -> Self {
        match id {
            SOUNDCLOUD => Provider::SoundCloud,
            _ => Provider::YouTube,
        }
    }
}

pub fn ytm_enabled(paths: &Paths) -> bool {
    paths.read_prefs().get(YTM_ENABLED_PREF).and_then(|v| v.as_bool()).unwrap_or(true)
}

pub fn set_ytm_enabled(paths: &Paths, enabled: bool) {
    paths.update_prefs(|p| {
        p.insert(YTM_ENABLED_PREF.to_owned(), enabled.into());
    });
}

pub fn sc_enabled(paths: &Paths) -> bool {
    paths.read_prefs().get(SC_ENABLED_PREF).and_then(|v| v.as_bool()).unwrap_or(false)
}

pub fn set_sc_enabled(paths: &Paths, enabled: bool) {
    paths.update_prefs(|p| {
        p.insert(SC_ENABLED_PREF.to_owned(), enabled.into());
    });
}

pub fn enabled_providers(paths: &Paths) -> Vec<Provider> {
    Provider::all().into_iter().filter(|p| match p {
        Provider::YouTube => ytm_enabled(paths),
        Provider::SoundCloud => sc_enabled(paths),
    }).collect()
}

pub fn active(paths: &Paths) -> Provider {
    let want = Provider::parse(paths.read_prefs().get(ACTIVE_PREF).and_then(|v| v.as_str()).unwrap_or(YOUTUBE));
    if enabled_providers(paths).contains(&want) {
        return want;
    }
    if ytm_enabled(paths) { Provider::YouTube } else { Provider::SoundCloud }
}

pub fn set_active(paths: &Paths, provider: Provider) {
    paths.update_prefs(|p| {
        p.insert(ACTIVE_PREF.to_owned(), provider.id().into());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_or_disabled_provider_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_tests(dir.path());
        assert_eq!(active(&paths), Provider::YouTube);
        assert_eq!(enabled_providers(&paths), vec![Provider::YouTube]);
        set_sc_enabled(&paths, true);
        set_active(&paths, Provider::SoundCloud);
        assert_eq!(active(&paths), Provider::SoundCloud);
        set_sc_enabled(&paths, false);
        assert_eq!(active(&paths), Provider::YouTube);
        set_ytm_enabled(&paths, false);
        set_sc_enabled(&paths, true);
        assert_eq!(active(&paths), Provider::SoundCloud);
    }
}
