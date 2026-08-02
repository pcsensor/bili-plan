//! Bilibili API 响应模型（宽松反序列化：未知字段忽略、缺省默认）。

use serde::de::Deserializer;
use serde::{Deserialize, Serialize};

/// 兼容数字或字符串的整数字段（B 站个别字段可能返回字符串）。
fn de_i64<'de, D>(d: D) -> Result<i64, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Num {
        I(i64),
        F(f64),
        S(String),
    }
    Ok(match Num::deserialize(d)? {
        Num::I(i) => i,
        Num::F(f) => f as i64,
        Num::S(s) => s.trim().parse().unwrap_or(0),
    })
}

/// view API 响应。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ViewResponse {
    #[serde(default)]
    pub code: i64,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub data: Option<ViewData>,
}

/// view API 的 data 部分。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ViewData {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default, deserialize_with = "de_i64")]
    pub duration: i64,
    #[serde(default)]
    pub pages: Option<Vec<Page>>,
    #[serde(rename = "ugc_season", default)]
    pub ugc_season: Option<UgcSeason>,
}

/// 分 P（video pages）。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Page {
    #[serde(default, deserialize_with = "de_i64")]
    pub page: i64,
    #[serde(default)]
    pub part: Option<String>,
    #[serde(default, deserialize_with = "de_i64")]
    pub duration: i64,
}

/// ugc_season（合集）结构。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct UgcSeason {
    #[serde(default, deserialize_with = "de_i64")]
    pub id: i64,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub sections: Option<Vec<Section>>,
}

/// 合集分栏。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Section {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub episodes: Option<Vec<Episode>>,
}

/// 合集条目（视频）。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Episode {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default, deserialize_with = "de_i64")]
    pub duration: i64,
    #[serde(default)]
    pub pages: Option<Vec<Page>>,
    #[serde(default)]
    pub arc: Option<ArcDur>,
}

/// 视频的 arc 信息（整集时长）。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ArcDur {
    #[serde(default, deserialize_with = "de_i64")]
    pub duration: i64,
}

/// seasons_archives_list API 响应。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ArchivesResponse {
    #[serde(default)]
    pub code: i64,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub data: Option<ArchivesData>,
}

/// seasons_archives_list 的 data 部分。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ArchivesData {
    #[serde(default)]
    pub archives: Option<Vec<ArchiveItem>>,
    #[serde(default)]
    pub page: Option<PageInfo>,
}

/// 扁平合集条目。
#[derive(Debug, Clone, Deserialize, Default, Serialize)]
pub struct ArchiveItem {
    #[serde(default)]
    pub title: String,
    #[serde(default, deserialize_with = "de_i64")]
    pub duration: i64,
}

/// 分页信息。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct PageInfo {
    #[serde(default, deserialize_with = "de_i64")]
    pub total: i64,
    #[serde(default, deserialize_with = "de_i64")]
    pub count: i64,
    #[serde(default, deserialize_with = "de_i64")]
    pub page_size: i64,
    #[serde(default, deserialize_with = "de_i64")]
    pub size: i64,
    #[serde(default, deserialize_with = "de_i64")]
    pub page_num: i64,
    #[serde(default, deserialize_with = "de_i64")]
    pub num: i64,
}
