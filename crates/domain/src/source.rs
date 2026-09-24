//! Source tags and playback links shared by the desktop and robot channels.
//!
//! Persisted plans keep the original string tag for compatibility with older
//! clients and with source types introduced by future versions.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Bilibili,
    Jellyfin,
    FnOs,
    Custom,
    Calendar,
    Unknown,
}

impl SourceKind {
    pub fn from_tag(tag: &str) -> Self {
        match tag {
            "bilibili" => Self::Bilibili,
            "jellyfin" => Self::Jellyfin,
            "fnos" => Self::FnOs,
            "custom" => Self::Custom,
            "calendar" => Self::Calendar,
            _ => Self::Unknown,
        }
    }

    pub const fn tag(self) -> &'static str {
        match self {
            Self::Bilibili => "bilibili",
            Self::Jellyfin => "jellyfin",
            Self::FnOs => "fnos",
            Self::Custom => "custom",
            Self::Calendar => "calendar",
            Self::Unknown => "",
        }
    }
}

/// Keep one implementation of Bilibili's page link for desktop and robots.
/// Other sources already provide their own direct URL.
pub fn video_link(source_type: &str, source_url: &str, vid_no: i64) -> String {
    if SourceKind::from_tag(source_type) != SourceKind::Bilibili {
        return source_url.to_string();
    }
    let trimmed = source_url.trim();
    let chars: Vec<char> = trimmed.chars().collect();
    if let Some(bvid) = chars.windows(12).find_map(|part| {
        (part[0] == 'B' && part[1] == 'V' && part[2..].iter().all(|c| c.is_ascii_alphanumeric()))
            .then(|| part.iter().collect::<String>())
    }) {
        return format!("https://www.bilibili.com/video/{bvid}?p={vid_no}");
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        let base = trimmed.split('?').next().unwrap_or(trimmed);
        format!("{base}?p={vid_no}")
    } else {
        format!("https://www.bilibili.com/video/{trimmed}?p={vid_no}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_tags_and_legacy_unknown_are_stable() {
        for kind in [
            SourceKind::Bilibili,
            SourceKind::Jellyfin,
            SourceKind::FnOs,
            SourceKind::Custom,
            SourceKind::Calendar,
        ] {
            assert_eq!(SourceKind::from_tag(kind.tag()), kind);
        }
        assert_eq!(SourceKind::from_tag("future-source"), SourceKind::Unknown);
    }

    #[test]
    fn video_links_handle_bvid_queries_and_other_sources() {
        assert_eq!(
            video_link(
                "bilibili",
                "https://www.bilibili.com/video/BV1ab2345678?t=2",
                3
            ),
            "https://www.bilibili.com/video/BV1ab2345678?p=3"
        );
        assert_eq!(
            video_link("bilibili", "https://example.com/watch?old=1", 2),
            "https://example.com/watch?p=2"
        );
        assert_eq!(
            video_link("jellyfin", "https://example.com/items?id=1", 2),
            "https://example.com/items?id=1"
        );
    }
}
