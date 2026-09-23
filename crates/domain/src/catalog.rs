use serde::Serialize;

/// A video or page that can be assigned to a day.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EpisodeItem {
    pub title: String,
    pub duration: i64,
}

/// A subject or collection of episodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Group {
    pub name: String,
    pub episodes: Vec<EpisodeItem>,
}
