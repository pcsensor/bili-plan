//! Jellyfin 集成：拉取课程合集结构，复用核心 `Group` / `EpisodeItem`
//! 数据契约，与 B 站适配器对齐。
//!
//! # 输入与鉴权
//! - 输入：Jellyfin 网页链接（`?id=` / `?parentId=` / `?seasonId=` / `?seriesId=`，
//!   覆盖 details 页与 `#/list?parentId=` 列表页两种形态）或裸 item ID
//! - 鉴权：`Authorization: MediaBrowser Token="..."` + `X-Emby-Token` 头双发
//!
//! # 分派（与 B 站 5 类结构对应）
//! - `Series`：每个 Season 一个 `Group`（对应 B 站「单分栏多 P 多科目」/「多分栏」）
//! - `Season`：整个季一个 `Group`（对应 B 站「单分栏普通合集」）
//! - `CollectionFolder` / `Folder` / `UserView` / `BoxSet` / `Playlist`：
//!   递归拉子 Episode+Movie，每个子 Series 一个 `Group`（对应 B 站「多分栏合集」）
//! - `Movie` / `Episode` / `Video`：单视频
//!
//! # 时长单位
//! Jellyfin 用 100ns 为单位的 `RunTimeTicks`；换算成秒（`ticks / 10_000_000`）。

use std::time::Duration;

use crate::error::{Error, Result};
use crate::parse::{EpisodeItem, Group};
use serde::Deserialize;

const TICKS_PER_SEC: i64 = 10_000_000;
const DEFAULT_TIMEOUT_SECS: u64 = 15;
const DEFAULT_RETRIES: u32 = 3;
/// Authorization 前缀，版本号取 `Cargo.toml` 同步，避免手动维护两处。
const AUTH_PREFIX: &str = concat!(
    "MediaBrowser Client=\"bili-planner\", Device=\"bili-planner\", Version=\"",
    env!("CARGO_PKG_VERSION"),
    "\""
);

// ---------------------------------------------------------------------------
// serde 模型（PascalCase 字段，缺失字段默认值，与 model.rs 宽松反序列化思路一致）
// ---------------------------------------------------------------------------

/// Jellyfin Item 响应。未知字段忽略，关键字段全部 `Option` + `#[serde(default)]`，
/// 应对 Jellyfin 不同接口字段集合的差异。
#[derive(Deserialize, Default, Clone)]
pub struct Item {
    #[serde(default, rename = "Id")]
    pub id: Option<String>,
    #[serde(default, rename = "Name")]
    pub name: Option<String>,
    #[serde(default, rename = "Type")]
    pub type_name: Option<String>,
    #[serde(default, rename = "RunTimeTicks")]
    pub run_time_ticks: Option<i64>,
    #[serde(default, rename = "IndexNumber")]
    pub index_number: Option<i64>,
    /// `Recursive=true` 列Episode时返回，便于在「文件夹」场景按子合集分组。
    #[serde(default, rename = "SeasonName")]
    pub season_name: Option<String>,
    /// `Recursive=true` 列Episode时返回，便于在「文件夹」场景按子合集分组。
    #[serde(default, rename = "SeriesName")]
    pub series_name: Option<String>,
    /// Jellyfin 返回 `IsFolder: true/false`——用于 folder 两层抓取时区分
    /// 子项要进一步展开（true）还是直接当视频（false）。
    #[serde(default, rename = "IsFolder")]
    pub is_folder: bool,
}

/// `/Items`、`/Shows/{id}/Episodes` 等列表接口的响应包裹。
#[derive(Deserialize, Default)]
pub struct ItemsResponse {
    #[serde(default, rename = "Items")]
    pub items: Vec<Item>,
}

// ---------------------------------------------------------------------------
// 链接解析
// ---------------------------------------------------------------------------

/// 从用户输入中识别 Jellyfin item id。
///
/// 识别 query string 中的 `id` / `parentId` / `seasonId` / `seriesId` 参数
/// （大小写不敏感）——支持 `?id=xxx`（详情页）、`?parentId=xxx`（列表页
/// /`list` 视图，用户实际链接格式）、`?seasonId=` 等多种 Jellyfin web 客户端
/// URL 形态。优先返回第一个出现的 `id`/`parentId` 参数值，避免与 `serverId`
/// 等无关字段混淆。当输入中无 query string 时回退为「裸 ID」。
pub fn extract_item_id(text: &str) -> Option<String> {
    // 定位 query string 起点：第一个 '?'（同时覆盖 'path?x=1' 与 '#/path?x=1'
    // 两种 hashbang 形式——'?' 始终是真正的 query 起始边界）。
    if let Some(qpos) = text.find('?') {
        let rest = &text[qpos + 1..];
        let rest = rest.split_whitespace().next().unwrap_or(rest);
        for pair in rest.split('&') {
            // 兼容双编码 '&amp;' 前缀。
            let pair = pair.strip_prefix("amp;").unwrap_or(pair);
            let eq_pos = match pair.find('=') {
                Some(p) => p,
                None => continue,
            };
            let key = pair[..eq_pos].to_lowercase();
            if matches!(key.as_str(), "id" | "parentid" | "seasonid" | "seriesid") {
                let value = &pair[eq_pos + 1..];
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
        return None;
    }
    // 无 query string：当作裸 item ID 回退（无 '/'、无空格才算合法）。
    let trimmed = text.trim();
    if !trimmed.is_empty() && !trimmed.contains('/') && !trimmed.contains(' ') {
        return Some(trimmed.to_string());
    }
    None
}

/// 从完整链接中提取 `scheme://host[:port]`；裸 ID 输入返回 `None`。
///
/// UI 可在用户粘贴链接时用它自动填充服务器地址字段，省去手填。
pub fn extract_base_url(text: &str) -> Option<String> {
    let scheme_pos = text.find("://")?;
    let after = &text[scheme_pos + 3..];
    let end = after.find(['/', '?', '#']).unwrap_or(after.len());
    if end == 0 {
        return None;
    }
    Some(format!("{}://{}", &text[..scheme_pos], &after[..end]))
}

// ---------------------------------------------------------------------------
// Jellyfin 客户端
// ---------------------------------------------------------------------------

/// Jellyfin REST 客户端：base_url + API Token。
#[derive(Clone)]
pub struct JellyfinClient {
    pub base_url: String,
    pub token: String,
}

/// 过滤 token 中的特殊字符，避免破坏 HTTP 头（Authorization 值含 `"` 会越界）。
fn sanitize_token(token: &str) -> String {
    token
        .chars()
        .filter(|c| !matches!(c, '"' | '\\' | '\n' | '\r' | '\t'))
        .collect()
}

impl JellyfinClient {
    pub fn new(base_url: String, token: String) -> Self {
        Self { base_url, token }
    }

    /// GET 请求并解析 JSON（失败重试 `DEFAULT_RETRIES` 次，间隔 1.2s * attempt，与 api.rs 对齐）。
    fn get_json(&self, path: &str) -> Result<serde_json::Value> {
        let url = format!("{}{}", self.base_url.trim_end_matches('/'), path);
        let safe_token = sanitize_token(&self.token);
        let auth = format!("{AUTH_PREFIX}, Token=\"{}\"", safe_token);
        let mut last_err = String::from("未知错误");
        for attempt in 1..=DEFAULT_RETRIES {
            match self.try_get(&url, &auth, &safe_token) {
                Ok(v) => return Ok(v),
                Err(e) => {
                    last_err = e;
                    if attempt < DEFAULT_RETRIES {
                        std::thread::sleep(Duration::from_secs_f64(1.2 * attempt as f64));
                    }
                }
            }
        }
        Err(Error::network(format!(
            "Jellyfin 网络请求失败（已重试 {DEFAULT_RETRIES} 次）：{last_err}"
        )))
    }

    fn try_get(
        &self,
        url: &str,
        auth: &str,
        safe_token: &str,
    ) -> std::result::Result<serde_json::Value, String> {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(DEFAULT_TIMEOUT_SECS)))
            .build()
            .new_agent();
        let mut resp = agent
            .get(url)
            .header("User-Agent", crate::api::DEFAULT_UA)
            .header("Accept", "application/json")
            .header("Authorization", auth)
            .header("X-Emby-Token", safe_token)
            .call()
            .map_err(|e| e.to_string())?;
        let raw = resp
            .body_mut()
            .read_to_string()
            .map_err(|e| e.to_string())?;
        let obj: serde_json::Value =
            serde_json::from_str(&raw).map_err(|e| format!("JSON 解析失败：{e}"))?;
        Ok(obj)
    }

    /// 获取单个 Item。
    pub fn get_item(&self, id: &str) -> Result<Item> {
        let path = format!("/Items?Ids={id}");
        let obj = self.get_json(&path)?;
        let resp: ItemsResponse = serde_json::from_value(obj)
            .map_err(|e| Error::network(format!("Jellyfin item 响应解析失败：{e}")))?;
        resp.items
            .into_iter()
            .next()
            .ok_or_else(|| Error::api("Jellyfin 未返回对应 Item（检查 ID 或 Token 权限）"))
    }

    /// 拉取 Series 下所有 Episode（带 `SeasonName` 字段，便于按季分组）。
    pub fn fetch_series_episodes(&self, series_id: &str) -> Result<Vec<Item>> {
        let path = format!("/Shows/{series_id}/Episodes");
        let obj = self.get_json(&path)?;
        let resp: ItemsResponse = serde_json::from_value(obj)
            .map_err(|e| Error::network(format!("Jellyfin episodes 响应解析失败：{e}")))?;
        Ok(resp.items)
    }

    /// 拉取某 ParentId（如 Season）下所有 Episode/Movie/Video。
    pub fn fetch_children_episodes(&self, parent_id: &str) -> Result<Vec<Item>> {
        let path = format!(
            "/Items?ParentId={parent_id}&Recursive=true\
             &IncludeItemTypes=Episode,Movie,Video&Limit=10000"
        );
        let obj = self.get_json(&path)?;
        let resp: ItemsResponse = serde_json::from_value(obj)
            .map_err(|e| Error::network(format!("Jellyfin children 响应解析失败：{e}")))?;
        Ok(resp.items)
    }

    /// 递归拉取某 ParentId 下全部 `Episode`/`Movie`/`Video`——
    /// `Video` 是 Jellyfin 非标准严格元数据下的常见类型（用户的库即如此）。
    pub fn fetch_episodes_recursive(&self, parent_id: &str) -> Result<Vec<Item>> {
        let path = format!(
            "/Items?ParentId={parent_id}&Recursive=true\
             &IncludeItemTypes=Episode,Movie,Video&Limit=10000"
        );
        let obj = self.get_json(&path)?;
        let resp: ItemsResponse = serde_json::from_value(obj)
            .map_err(|e| Error::network(format!("Jellyfin recursive 响应解析失败：{e}")))?;
        Ok(resp.items)
    }

    /// 拉取直属子项（非递归）——folder 两层抓取的第一步，用于区分
    /// 子项是要继续展开（`IsFolder=true`）还是直接当视频。
    pub fn fetch_children(&self, parent_id: &str) -> Result<Vec<Item>> {
        let path = format!("/Items?ParentId={parent_id}&Limit=10000");
        let obj = self.get_json(&path)?;
        let resp: ItemsResponse = serde_json::from_value(obj)
            .map_err(|e| Error::network(format!("Jellyfin children 响应解析失败：{e}")))?;
        Ok(resp.items)
    }

    /// 文件夹场景的两层抓取：对每个子 `Folder` 递归拉视频并填 `SeriesName=子文件夹名`；
    /// 顶层直属视频（`Video`/`Episode`/`Movie`）归入以父文件夹名为名的「散件」组。
    ///
    /// 这样模拟 B 站「多分栏合集」语义——子文件夹=分栏=科目，散件归一科。
    /// 不依赖 Jellyfin 是否返回 `SeriesName` 字段（用户的库里都为 null）。
    pub fn fetch_folder_videos(&self, folder_id: &str, folder_name: &str) -> Result<Vec<Item>> {
        let children = self.fetch_children(folder_id)?;
        let mut all: Vec<Item> = Vec::new();
        for child in children.iter() {
            if child.is_folder {
                let sub_id = match child.id.as_deref() {
                    Some(s) if !s.is_empty() => s,
                    _ => continue,
                };
                let sub_name = child.name.clone().unwrap_or_default();
                let grandson = self.fetch_episodes_recursive(sub_id)?;
                for mut ep in grandson {
                    ep.series_name = Some(sub_name.clone());
                    all.push(ep);
                }
            } else {
                let ctype = child.type_name.as_deref().unwrap_or("");
                if matches!(ctype, "Video" | "Episode" | "Movie")
                    && ticks_to_secs(child.run_time_ticks) > 0
                {
                    let mut ep = child.clone();
                    ep.series_name = Some(folder_name.to_string());
                    all.push(ep);
                }
            }
        }
        Ok(all)
    }
}

// ---------------------------------------------------------------------------
// 分派与分组（纯逻辑，可独立单元测试）
// ---------------------------------------------------------------------------

/// `RunTimeTicks` → 秒。
pub fn ticks_to_secs(ticks: Option<i64>) -> i64 {
    ticks.unwrap_or(0) / TICKS_PER_SEC
}

/// 单集标题：有 `IndexNumber`（>0）时拼成 `第N集 标题`，否则用 `Name`。
pub fn episode_title(it: &Item) -> String {
    let name = it.name.clone().unwrap_or_default().trim().to_string();
    match it.index_number {
        Some(n) if n > 0 => {
            if name.is_empty() {
                format!("第{n}集")
            } else {
                format!("第{n}集 {name}")
            }
        }
        _ => name,
    }
}

/// 按 `SeasonName` 顺序分组，保持原响应顺序（Jellyfin 默认按季+集序返回）。
/// 缺失 `SeasonName` 的回退到「未分季」组。
pub fn group_episodes_by_season(items: &[Item]) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    let mut current: Option<String> = None;
    for it in items {
        let season = it
            .season_name
            .clone()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "未分季".to_string());
        if current.as_deref() != Some(&season) {
            current = Some(season.clone());
            groups.push(Group {
                name: season,
                episodes: Vec::new(),
            });
        }
        groups
            .last_mut()
            .expect("groups 非空")
            .episodes
            .push(EpisodeItem {
                title: episode_title(it),
                duration: ticks_to_secs(it.run_time_ticks),
            });
    }
    groups
}

/// 文件夹/课程库场景：按 `SeriesName` 把递归拉到的 Episode 集合分组，
/// 每个子合集（Series）一个 `Group`。响应顺序不保证时用 find-or-insert
/// 兜底（与 `group_episodes_by_season` 的「连续假设」不同）。
/// 缺失 `SeriesName` 时回退用 `SeasonName`；都缺失则归入「未分类」组。
pub fn group_episodes_by_series(items: &[Item]) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for it in items {
        let key = it
            .series_name
            .clone()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                it.season_name
                    .clone()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_else(|| "未分类".to_string());
        match groups.iter().position(|g| g.name == key) {
            Some(p) => groups[p].episodes.push(EpisodeItem {
                title: episode_title(it),
                duration: ticks_to_secs(it.run_time_ticks),
            }),
            None => groups.push(Group {
                name: key,
                episodes: vec![EpisodeItem {
                    title: episode_title(it),
                    duration: ticks_to_secs(it.run_time_ticks),
                }],
            }),
        }
    }
    groups
}

/// 根据 `Item.Type` 分派，返回 `(season_title, groups, structure)`。
///
/// `episodes` 在 `Series` / `Season` 类型时使用；其他类型传入空 `Vec` 即可。
pub fn classify_item(item: &Item, episodes: Vec<Item>) -> Result<(String, Vec<Group>, String)> {
    let item_type = item.type_name.as_deref().unwrap_or("");
    let title = item.name.clone().unwrap_or_default().trim().to_string();
    match item_type {
        "Series" => {
            let groups = group_episodes_by_season(&episodes);
            let total: i64 = groups
                .iter()
                .flat_map(|g| g.episodes.iter().map(|e| e.duration))
                .sum();
            if total <= 0 {
                return Err(Error::data(
                    "Jellyfin 合集缺少时长信息（RunTimeTicks），无法生成计划。请检查元数据。",
                ));
            }
            let structure = if groups.len() > 1 {
                "Jellyfin 多季合集（每季视为一门科目）"
            } else {
                "Jellyfin 单季合集（整个合集视为一门课程）"
            };
            Ok((title, groups, structure.to_string()))
        }
        "Season" => {
            let all: Vec<EpisodeItem> = group_episodes_by_season(&episodes)
                .into_iter()
                .flat_map(|g| g.episodes)
                .collect();
            let total: i64 = all.iter().map(|e| e.duration).sum();
            if total <= 0 {
                return Err(Error::data(
                    "Jellyfin 该季缺少时长信息（RunTimeTicks），无法生成计划。",
                ));
            }
            Ok((
                title.clone(),
                vec![Group {
                    name: title,
                    episodes: all,
                }],
                "Jellyfin 单季合集（整个合集视为一门课程）".to_string(),
            ))
        }
        "Movie" | "Episode" | "Video" => {
            let duration = ticks_to_secs(item.run_time_ticks);
            if duration <= 0 {
                return Err(Error::data(
                    "Jellyfin 该视频缺少 RunTimeTicks 时长信息，无法生成计划。",
                ));
            }
            Ok((
                title.clone(),
                vec![Group {
                    name: "单视频".to_string(),
                    episodes: vec![EpisodeItem { title, duration }],
                }],
                "Jellyfin 单视频".to_string(),
            ))
        }
        // 文件夹/课程库/集合：递归拉取的 Episode+Movie 按 SeriesName 分组，
        // 每个子合集（Series）一个科目——对应 B 站「多分栏合集」语义。
        "CollectionFolder" | "Folder" | "UserView" | "BoxSet" | "Playlist" => {
            let groups = group_episodes_by_series(&episodes);
            let total: i64 = groups
                .iter()
                .flat_map(|g| g.episodes.iter().map(|e| e.duration))
                .sum();
            if total <= 0 {
                return Err(Error::data(
                    "Jellyfin 文件夹下未找到可规划的视频（Episode/Movie）。\
                     请确认该文件夹包含课程视频且元数据含 RunTimeTicks。",
                ));
            }
            let structure = if groups.len() > 1 {
                "Jellyfin 文件夹（每个子合集视为一门科目）"
            } else {
                "Jellyfin 文件夹（整个文件夹视为一门课程）"
            };
            Ok((title, groups, structure.to_string()))
        }
        other => Err(Error::input(format!(
            "不支持的 Jellyfin 项目类型：{other}\
             （仅支持 Series/Season/Movie/Episode/CollectionFolder/Folder/UserView/BoxSet/Playlist）"
        ))),
    }
}

/// 主入口：解析输入 → 拉取 Item → 按 Type 拉对应 episodes → `classify_item`。
pub fn fetch_groups(client: &JellyfinClient, input: &str) -> Result<(String, Vec<Group>, String)> {
    let item_id = extract_item_id(input).ok_or_else(|| {
        Error::input(
            "无法从输入中识别 Jellyfin item id。请粘贴形如 \
             https://host/web/#/list?parentId=xxx 或 https://host/web/#/details?id=xxx \
             的链接，或直接输入 item ID。",
        )
    })?;
    let item = client.get_item(&item_id)?;
    let item_name = item.name.clone().unwrap_or_default();
    let episodes = match item.type_name.as_deref() {
        Some("Series") => client.fetch_series_episodes(&item_id)?,
        Some("Season") => client.fetch_children_episodes(&item_id)?,
        Some("CollectionFolder")
        | Some("Folder")
        | Some("UserView")
        | Some("BoxSet")
        | Some("Playlist") => client.fetch_folder_videos(&item_id, &item_name)?,
        _ => Vec::new(),
    };
    classify_item(&item, episodes)
}
