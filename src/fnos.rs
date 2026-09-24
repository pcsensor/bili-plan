//! 飞牛影视（fnOS Trim Media）集成：拉取影视库剧集结构，复用核心
//! `Group` / `EpisodeItem` 数据契约，与 B 站 / Jellyfin 适配器对齐。
//!
//! # 与其它适配器的差异
//! 飞牛影视**没有公开 API 文档**，本模块的接口路径、请求体与签名算法来自
//! 官方 Web 客户端（`trimemedia-web`）的逆向结果。因此这里对响应字段一律
//! 做宽松反序列化（缺省即忽略），并把签名算法拆成纯函数，用固定向量回归，
//! 这样飞牛改版时能一眼看出是哪一层失效。
//!
//! # 鉴权（`authx` 头）
//! 每个请求都需要：
//!
//! ```text
//! Authx: nonce=<6位数字>&timestamp=<毫秒>&sign=<md5>
//! sign  = md5(api_key _ path _ nonce _ timestamp _ md5(body_bytes) _ api_secret)
//! ```
//!
//! 两个容易踩的细节：
//! 1. `path` 是**不含协议与域名的接口路径**（如 `/v/api/v1/item/list`）。
//! 2. `body_bytes` 是**实际发出的请求体字节**。官方 JS 客户端用
//!    `JSON.stringify`（保留插入序），而 Rust 侧 `serde_json::Value` 默认按键
//!    排序，两者序列化结果不同。本模块统一「先序列化成字节 → 用这份字节签名
//!    → 原样发送同一份字节」，从而与服务端校验自洽。
//!
//! 另外 POST 会在 body 里额外塞一个随机 `nonce` 防重放，它与 `authx` 里的
//! nonce 是**两个独立随机值**（与官方实现一致）。
//!
//! # 输入与结构识别
//! - 输入：飞牛影视网页链接（取 `guid=` / `id=` 查询参数、或路径末段）或裸 guid
//! - `item/list` 返回的条目按 `type` 分派：
//!   - 容器（`Directory`/`Folder`/`TV`/`Season` 等）→ 递归展开，每个容器一门科目
//!   - 叶子（`Movie`/`Episode`/`Video`）→ 一个观看单元
//!
//! # 时长单位
//! `item/list` 的 `duration` 字段单位是**秒**；`runtime` 是分钟，仅在
//! `duration` 缺失或为 0 时兜底换算。

use std::collections::HashSet;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc, Mutex,
};
use std::time::Duration;

use serde::{Deserialize, Deserializer};

use crate::error::{Error, Result};
use crate::parse::{EpisodeItem, Group};

// ---------------------------------------------------------------------------
// 协议常量
// ---------------------------------------------------------------------------

/// 签名密钥，内嵌于官方 Web 客户端。飞牛影视版本升级后若变更会导致
/// `code=5000 invalid sign`，届时需要同步这里。
const API_KEY: &str = "NDzZTVxnRKP8Z0jXg1VAMonaG8akvh";
/// 签名密钥（第二段），来源同上。
const API_SECRET: &str = "16CCEB3D-AB42-077D-36A1-F355324E4237";
/// 登录时上报的应用标识，官方 Web 端固定为该值。
pub const APP_NAME: &str = "trimemedia-web";

const API_LOGIN: &str = "/v/api/v1/login";
const API_ITEM_LIST: &str = "/v/api/v1/item/list";
const API_PLAY_INFO: &str = "/v/api/v1/play/info";
const API_STREAM: &str = "/v/api/v1/stream";

const DEFAULT_TIMEOUT_SECS: u64 = 15;
const DEFAULT_RETRIES: u32 = 3;
/// 容器递归展开的深度上限，防止异常数据导致无限下钻。
const MAX_DEPTH: usize = 8;
const PAGE_SIZE: usize = 200;
const MAX_PAGES: usize = 10_000;
const DURATION_WORKERS: usize = 3;
const DURATION_POLL_ATTEMPTS: usize = 5;

/// `item/list` 的业务错误码：签名无效（换 nonce/timestamp 重试即可）。
const CODE_INVALID_SIGN: i64 = 5000;
/// `item/list` 的业务错误码：登录态失效（需要重新登录）。
const CODE_UNAUTHENTICATED: i64 = -2;

// ---------------------------------------------------------------------------
// MD5 与 authx 签名（纯函数，可用固定向量回归）
// ---------------------------------------------------------------------------

/// 十六进制小写 MD5。
pub fn md5_hex(bytes: &[u8]) -> String {
    format!("{:x}", md5::compute(bytes))
}

/// 组装 `authx` 头值。
///
/// `body` 必须是**实际将要发送的请求体字节**——签名与发送共用同一份字节，
/// 避免序列化差异导致验签失败。登录等无 body 的请求传空切片。
pub fn gen_authx(path: &str, body: &[u8], nonce: &str, timestamp_ms: u128) -> String {
    let body_md5 = md5_hex(body);
    let sign_source = format!("{API_KEY}_{path}_{nonce}_{timestamp_ms}_{body_md5}_{API_SECRET}");
    format!(
        "nonce={nonce}&timestamp={timestamp_ms}&sign={}",
        md5_hex(sign_source.as_bytes())
    )
}

/// 6 位数字随机串（对应官方 `generateRandomDigits`）。
///
/// 不使用 `rand`：客户端 crate 未引入该依赖，这里沿用 `core.rs` 生成设备
/// 标识时的 `RandomState` 取熵方式，够用且零新增依赖。
fn random_nonce() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};

    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    format!("{:06}", hasher.finish() % 1_000_000)
}

/// 连接失败时追加的排障提示。
///
/// macOS 15+ 对局域网地址（192.168.x.x / 10.x.x.x / .local）有「本地网络」
/// 隐私管控：打包成 .app 后若未正确签名或未被授权，访问飞牛这类 NAS 会被
/// 静默拒绝，表现为「连不上网络」；而 `cargo run` 走的是终端已授权的身份，
/// 表现正常。这里给出明确指引，避免误判成服务端故障。
fn local_network_hint() -> &'static str {
    if cfg!(target_os = "macos") {
        "（若已打包为 .app：请确认 系统设置 → 隐私与安全性 → 本地网络 中已允许本应用，\
         且 .app 已做过 codesign 签名）"
    } else {
        ""
    }
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// serde 模型（宽松反序列化：未知字段忽略，数字兼容字符串/浮点）
// ---------------------------------------------------------------------------

/// 兼容 数字 / 浮点 / 字符串 / null 的可选整数字段。
fn de_opt_i64<'de, D>(deserializer: D) -> std::result::Result<Option<i64>, D::Error>
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
    let value: Option<Num> = Option::deserialize(deserializer)?;
    Ok(value.map(|n| match n {
        Num::I(i) => i,
        Num::F(f) => f as i64,
        Num::S(s) => s.trim().parse().unwrap_or(0),
    }))
}

/// `item/list` 返回的单条目。容器与叶子共用同一结构，靠 `kind` 区分。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct FnItem {
    #[serde(default)]
    pub guid: Option<String>,
    /// 原始文件 ID；网盘挂载视频需要用它探测实际时长。
    #[serde(default)]
    pub media_guid: Option<String>,
    /// 条目标题；容器（季/合集）与叶子（单集）都用它。
    #[serde(default)]
    pub title: Option<String>,
    /// 所属剧集名，仅叶子条目可能返回。
    #[serde(default)]
    pub tv_title: Option<String>,
    /// 父级标题（如「第 1 季」）。`item/list` 不返回父节点自身标题，
    /// 因此根标题由子项的该字段反推。
    #[serde(default)]
    pub parent_title: Option<String>,
    #[serde(default)]
    pub parent_guid: Option<String>,
    /// 条目类型：`Directory`/`Folder`/`TV`/`Season` 等为容器，
    /// `Movie`/`Episode`/`Video` 为可播放叶子。
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    /// 视频时长（**秒**）。
    #[serde(default, deserialize_with = "de_opt_i64")]
    pub duration: Option<i64>,
    /// 运行时长（**分钟**），`duration` 缺失时为兜底。
    #[serde(default, deserialize_with = "de_opt_i64")]
    pub runtime: Option<i64>,
    #[serde(default, deserialize_with = "de_opt_i64")]
    pub season_number: Option<i64>,
    #[serde(default, deserialize_with = "de_opt_i64")]
    pub episode_number: Option<i64>,
}

/// `/v/api/v1/item/list` 的 `data` 部分。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ItemListData {
    /// 媒体库名，作为根标题的最后兜底。
    #[serde(default)]
    pub mdb_name: Option<String>,
    #[serde(default, alias = "total_count", deserialize_with = "de_opt_i64")]
    pub total: Option<i64>,
    // 列表字段缺失属于协议异常，不能当成空目录静默跳过。
    pub list: Vec<FnItem>,
}

/// 统一响应包裹：`{ code, msg, data }`。
#[derive(Debug, Deserialize, Default)]
struct Envelope<T> {
    #[serde(default, deserialize_with = "de_opt_i64")]
    code: Option<i64>,
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    data: Option<T>,
}

/// `/v/api/v1/login` 的 `data` 部分。
#[derive(Debug, Clone, Deserialize, Default)]
struct LoginData {
    #[serde(default)]
    token: Option<String>,
}

// ---------------------------------------------------------------------------
// 纯逻辑：时长、类型判定、排序、标题
// ---------------------------------------------------------------------------

fn clean(value: Option<&str>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// 条目时长换算为秒：`duration`（秒）优先，`runtime`（分钟）兜底。
pub fn duration_of(item: &FnItem) -> i64 {
    if let Some(d) = item.duration {
        if d > 0 {
            return d;
        }
    }
    match item.runtime {
        Some(minutes) if minutes > 0 => minutes * 60,
        _ => 0,
    }
}

/// 单集标题：有 `episode_number`（>0）时拼成 `第N集 标题`，否则用 `title`。
pub fn episode_title_of(item: &FnItem) -> String {
    let name = clean(item.title.as_deref()).unwrap_or_default();
    match item.episode_number {
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

impl FnItem {
    /// 是否需要继续下钻。
    ///
    /// 已知容器类型直接判定；未知类型用「有时长即叶子，无时长当容器」兜底，
    /// 这样飞牛新增类型时不会漏抓内容，而递归本身另有深度上限兜底。
    pub fn is_container(&self) -> bool {
        let kind = self
            .kind
            .as_deref()
            .map(|k| k.trim().to_ascii_lowercase())
            .unwrap_or_default();
        match kind.as_str() {
            "directory" | "folder" | "tv" | "season" | "collectionfolder" | "boxset"
            | "playlist" | "mdb" => true,
            "movie" | "episode" | "video" => false,
            _ => duration_of(self) <= 0 && clean(self.media_guid.as_deref()).is_none(),
        }
    }

    /// 容器分组名：优先 `title`，回退 `tv_title`，再回退给定兜底值。
    pub fn display_title(&self, fallback: &str) -> String {
        clean(self.title.as_deref())
            .or_else(|| clean(self.tv_title.as_deref()))
            .unwrap_or_else(|| fallback.to_string())
    }
}

/// 叶子条目 → 观看单元；无有效时长时返回 `None`（无法纳入计划）。
///
/// 标题缺失时用占位名而非丢弃，避免静默少排内容。
pub fn episode_from(item: &FnItem) -> Option<EpisodeItem> {
    let duration = duration_of(item);
    if duration <= 0 {
        return None;
    }
    let title = episode_title_of(item);
    Some(EpisodeItem {
        title: if title.is_empty() {
            "未命名条目".to_string()
        } else {
            title
        },
        duration,
    })
}

/// 服务端排序不可控：仅当**全部**条目都带集号时才按 (季, 集) 重排，
/// 保证计划顺序与剧集顺序一致；否则保持服务端返回顺序不动。
pub fn order_items(mut items: Vec<FnItem>) -> Vec<FnItem> {
    let all_numbered = !items.is_empty()
        && items
            .iter()
            .all(|item| item.episode_number.unwrap_or(0) > 0);
    if all_numbered {
        items.sort_by_key(|item| {
            (
                item.season_number.unwrap_or(1),
                item.episode_number.unwrap_or(0),
            )
        });
    }
    items
}

/// 根标题反推：`item/list` 不返回父节点自身标题，只能借子项的
/// `parent_title`；依次回退 `tv_title` → 媒体库名 → 通用名。
pub fn derive_root_title(children: &[FnItem], mdb_name: Option<&str>) -> String {
    for item in children {
        if let Some(title) = clean(item.parent_title.as_deref()) {
            return title;
        }
    }
    for item in children {
        if let Some(title) = clean(item.tv_title.as_deref()) {
            return title;
        }
    }
    clean(mdb_name).unwrap_or_else(|| "飞牛影视合集".to_string())
}

// ---------------------------------------------------------------------------
// 分组（纯逻辑，`expand` 回调注入「如何展开一个容器」，便于无网络回归）
// ---------------------------------------------------------------------------

/// 把一层子项归类为科目分组，返回 `(根标题, groups, structure)`。
///
/// - 容器子项：调用 `expand(guid)` 取其下全部叶子，各自成为一门科目
/// - 叶子子项：与容器并列的散件归入根标题下的一组；没有容器时它即唯一一组
pub fn classify_children<F>(
    children: Vec<FnItem>,
    mdb_name: Option<&str>,
    mut expand: F,
) -> Result<(String, Vec<Group>, String)>
where
    F: FnMut(&str) -> Result<Vec<EpisodeItem>>,
{
    let root_title = derive_root_title(&children, mdb_name);
    let mut groups: Vec<Group> = Vec::new();
    let mut loose: Vec<EpisodeItem> = Vec::new();

    for item in children {
        if item.is_container() {
            let guid = match clean(item.guid.as_deref()) {
                Some(guid) => guid,
                None => continue,
            };
            let episodes = expand(&guid)?;
            if episodes.is_empty() {
                continue;
            }
            groups.push(Group {
                name: item.display_title(&root_title),
                episodes,
            });
        } else if let Some(episode) = episode_from(&item) {
            loose.push(episode);
        }
    }

    if !loose.is_empty() {
        let name = if groups.is_empty() {
            root_title.clone()
        } else {
            format!("{root_title}（散件）")
        };
        groups.insert(
            0,
            Group {
                name,
                episodes: loose,
            },
        );
    }

    if groups.is_empty() {
        return Err(Error::data(
            "飞牛影视该条目下未找到可规划的视频（缺少时长信息）。\
             请检查媒体元数据，或换一个合集/季重试。",
        ));
    }
    let total: i64 = groups
        .iter()
        .flat_map(|group| group.episodes.iter().map(|e| e.duration))
        .sum();
    if total <= 0 {
        return Err(Error::data(
            "飞牛影视统计范围内视频总时长为 0，无法生成计划。",
        ));
    }

    let structure = if groups.len() > 1 {
        "飞牛影视多层级合集（每个子项视为一门科目）"
    } else {
        "飞牛影视单合集（整个合集视为一门课程）"
    };
    Ok((root_title, groups, structure.to_string()))
}

// ---------------------------------------------------------------------------
// 输入解析
// ---------------------------------------------------------------------------

/// 查询参数里可承载条目 ID 的键。
fn is_id_key(key: &str) -> bool {
    matches!(
        key,
        "guid" | "id" | "item_guid" | "parent_guid" | "media_guid" | "season_guid" | "series_guid"
    )
}

/// 查询参数值：放宽到 4 字符（用户可能粘贴短 ID）。
fn is_query_id_like(value: &str) -> bool {
    value.len() >= 4
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// 路径末段：收紧到 16 字符，避免把 `details`、`web`、`video` 这类
/// 路由词误当成 guid。
fn is_path_guid_like(value: &str) -> bool {
    value.len() >= 16
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && value.chars().any(|c| c.is_ascii_digit())
}

/// 从用户输入中识别飞牛影视条目 guid。
///
/// 支持三类形态：
/// - 查询参数：`http://host:5666/v/video?guid=fv_3f2a…`
/// - 路径末段：`http://host:5666/v/video/fv_3f2a…`
/// - 裸 guid：`fv_3f2a…` 或 32 位十六进制
pub fn extract_guid(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }

    if let Some(qpos) = text.find('?') {
        let query = &text[qpos + 1..];
        let query = query.split_whitespace().next().unwrap_or(query);
        // hash 路由下 '#' 可能出现在 query 之后，先截断。
        let query = query.split('#').next().unwrap_or(query);
        for pair in query.split('&') {
            // 兼容 HTML 转义后的 `&amp;`。
            let pair = pair.strip_prefix("amp;").unwrap_or(pair);
            let Some(eq) = pair.find('=') else { continue };
            let key = pair[..eq].to_lowercase();
            if !is_id_key(&key) {
                continue;
            }
            let value = pair[eq + 1..].trim();
            if is_query_id_like(value) {
                return Some(value.to_string());
            }
        }
    }

    // 回退到路径末段；裸 guid 也走这一分支（无 '/' 时 rsplit 返回整串）。
    let no_query = text.split(['?', '#']).next().unwrap_or(text);
    let segment = no_query.trim_end_matches('/').rsplit('/').next()?;
    if is_path_guid_like(segment) {
        return Some(segment.to_string());
    }
    None
}

mod client;
mod duration;
mod pagination;
#[cfg(test)]
mod tests;

pub use client::FnOsClient;
use duration::{complete_durations, stream_duration, wait_for_duration};
pub use pagination::item_list_body;
use pagination::{collect_pages, truncate};

// ---------------------------------------------------------------------------
// 主入口
// ---------------------------------------------------------------------------

/// 递归展开一个容器，返回其下全部可规划的视频单元。
fn collect_leaves(
    client: &FnOsClient,
    guid: &str,
    depth: usize,
    progress: &mut dyn FnMut(String),
) -> Result<Vec<EpisodeItem>> {
    if depth > MAX_DEPTH {
        return Err(Error::data(
            "飞牛影视目录层级过深，已停止展开（疑似媒体库结构异常）。",
        ));
    }
    let mut out: Vec<EpisodeItem> = Vec::new();
    let mut items = client.item_list_with_progress(guid, progress)?.list;
    client.complete_durations(&mut items, progress)?;
    for item in order_items(items) {
        if item.is_container() {
            if let Some(child) = clean(item.guid.as_deref()) {
                out.extend(collect_leaves(client, &child, depth + 1, progress)?);
            }
        } else if let Some(episode) = episode_from(&item) {
            out.push(episode);
        }
    }
    Ok(out)
}

/// 主入口：解析输入 → 拉取一层子项 → 归类分组。
pub fn fetch_groups(client: &FnOsClient, input: &str) -> Result<(String, Vec<Group>, String)> {
    fetch_groups_with_progress(client, input, &mut |_| {})
}

/// 支持进度回调的获取入口，供桌面与命令行展示后台工作状态。
pub fn fetch_groups_with_progress(
    client: &FnOsClient,
    input: &str,
    progress: &mut dyn FnMut(String),
) -> Result<(String, Vec<Group>, String)> {
    if client.base_url.trim().is_empty() {
        return Err(Error::input("请填写飞牛影视服务器地址。"));
    }
    if client.username.trim().is_empty() || client.password.trim().is_empty() {
        return Err(Error::input("请填写飞牛影视账号与密码。"));
    }
    let guid = extract_guid(input).ok_or_else(|| {
        Error::input(
            "无法从输入中识别飞牛影视条目 ID。请粘贴形如 \
             http://host:5666/v/video?guid=fv_xxxx 的链接，或直接输入 guid。",
        )
    })?;

    progress("正在连接飞牛影视并读取目录…".to_string());
    let root = client.item_list_with_progress(&guid, progress)?;
    if root.list.is_empty() {
        return Err(Error::data(
            "飞牛影视未返回任何条目。请确认该 guid 指向影视库 / 合集 / 季，\
             且当前账号有访问权限。",
        ));
    }
    let mut children = order_items(root.list);
    client.complete_durations(&mut children, progress)?;
    classify_children(children, root.mdb_name.as_deref(), |child_guid| {
        collect_leaves(client, child_guid, 1, progress)
    })
}
