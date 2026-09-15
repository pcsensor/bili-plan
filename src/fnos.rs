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

// ---------------------------------------------------------------------------
// HTTP 客户端
// ---------------------------------------------------------------------------

/// 单次请求的判定结果，用于区分「该重试」和「该重新登录」。
enum Outcome {
    Data(serde_json::Value),
    /// 签名被拒：换一组 nonce/timestamp 重试即可。
    BadSign,
    /// 登录态失效：需要清缓存重新登录。
    Expired,
    /// 其它业务错误，不再重试。
    Failed(String),
}

/// 对外暴露的失败原因。
enum PostError {
    Expired,
    Fatal(String),
}

/// 飞牛影视 REST 客户端：服务器地址 + 账号密码，token 惰性登录并缓存。
pub struct FnOsClient {
    pub base_url: String,
    pub username: String,
    pub password: String,
    token: Mutex<Option<String>>,
}

impl std::fmt::Debug for FnOsClient {
    /// 手写 Debug，避免密码进入日志。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FnOsClient")
            .field("base_url", &self.base_url)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

impl FnOsClient {
    pub fn new(
        base_url: impl Into<String>,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        Self {
            base_url: base_url.into().trim().trim_end_matches('/').to_string(),
            username: username.into(),
            password: password.into(),
            token: Mutex::new(None),
        }
    }

    /// 当前 token（无则登录）。
    fn token(&self) -> Result<String> {
        if let Some(token) = self.token.lock().unwrap().as_ref() {
            return Ok(token.clone());
        }
        self.login()
    }

    fn login(&self) -> Result<String> {
        let body = serde_json::json!({
            "app_name": APP_NAME,
            "username": self.username,
            "password": self.password,
        });
        // 登录自身不能再触发「重新登录」，因此 allow_relogin = false。
        let value = self
            .send(API_LOGIN, body, false)
            .map_err(|e| Error::api(format!("飞牛影视登录失败：{e}")))?;
        let data: LoginData = serde_json::from_value(value)
            .map_err(|e| Error::network(format!("飞牛影视登录响应解析失败：{e}")))?;
        let token = data
            .token
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| Error::api("飞牛影视登录成功但未返回 token。"))?;
        *self.token.lock().unwrap() = Some(token.clone());
        Ok(token)
    }

    /// 带登录态重试的请求：token 失效时清缓存重登一次。
    fn send(
        &self,
        path: &str,
        body: serde_json::Value,
        allow_relogin: bool,
    ) -> std::result::Result<serde_json::Value, String> {
        let max_attempts = if allow_relogin { 2 } else { 1 };
        let mut last_err = String::from("未知错误");
        for attempt in 0..max_attempts {
            let token = if allow_relogin {
                self.token().map_err(|e| e.message().to_string())?
            } else {
                String::new()
            };
            match self.post(path, &body, &token) {
                Ok(value) => return Ok(value),
                Err(PostError::Expired) => {
                    last_err = "登录态已失效".to_string();
                    *self.token.lock().unwrap() = None;
                    if attempt + 1 >= max_attempts {
                        break;
                    }
                }
                Err(PostError::Fatal(message)) => return Err(message),
            }
        }
        Err(format!("{last_err}（已尝试 {max_attempts} 次）"))
    }

    /// 单次 POST：签名 → 发送 → 解析包裹 → 判定。
    fn post(
        &self,
        path: &str,
        body: &serde_json::Value,
        token: &str,
    ) -> std::result::Result<serde_json::Value, PostError> {
        let url = format!("{}{}", self.base_url, path);
        let mut last_err = String::from("未知错误");
        for attempt in 1..=DEFAULT_RETRIES {
            match self.try_post(&url, path, body, token) {
                Ok(Outcome::Data(value)) => return Ok(value),
                Ok(Outcome::Expired) => return Err(PostError::Expired),
                Ok(Outcome::Failed(message)) => return Err(PostError::Fatal(message)),
                Ok(Outcome::BadSign) => {
                    last_err = "服务端拒绝签名（invalid sign）".to_string();
                }
                Err(err) => last_err = err,
            }
            if attempt < DEFAULT_RETRIES {
                std::thread::sleep(Duration::from_secs_f64(1.2 * attempt as f64));
            }
        }
        Err(PostError::Fatal(format!(
            "飞牛影视请求失败（已重试 {DEFAULT_RETRIES} 次）：{last_err}"
        )))
    }

    fn try_post(
        &self,
        url: &str,
        path: &str,
        body: &serde_json::Value,
        token: &str,
    ) -> std::result::Result<Outcome, String> {
        // 1. 先注入防重放 nonce，再序列化——之后签名与发送都用这份字节。
        let mut payload = body.clone();
        if let Some(object) = payload.as_object_mut() {
            object.insert(
                "nonce".to_string(),
                serde_json::Value::String(random_nonce()),
            );
        }
        let bytes = serde_json::to_vec(&payload).map_err(|e| format!("构造请求体失败：{e}"))?;

        // 2. authx 用的 nonce 与 body 里的防重放 nonce 是两个独立随机值。
        let authx = gen_authx(path, &bytes, &random_nonce(), now_ms());

        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(DEFAULT_TIMEOUT_SECS)))
            .build()
            .new_agent();
        let mut request = agent
            .post(url)
            .header("User-Agent", crate::api::DEFAULT_UA)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .header("Authx", &authx);
        if !token.is_empty() {
            request = request.header("Authorization", token);
        }

        let mut response = request
            .send(bytes)
            .map_err(|e| format!("网络请求失败：{e}"))?;
        let raw = response
            .body_mut()
            .read_to_string()
            .map_err(|e| format!("读取响应失败：{e}"))?;

        let envelope: Envelope<serde_json::Value> = serde_json::from_str(&raw)
            .map_err(|e| format!("响应不是合法 JSON（{e}）：{}", truncate(&raw, 200)))?;

        let code = envelope.code.unwrap_or(0);
        let message = envelope.msg.clone().unwrap_or_default();
        match code {
            0 => Ok(Outcome::Data(
                envelope.data.unwrap_or(serde_json::Value::Null),
            )),
            CODE_INVALID_SIGN => Ok(Outcome::BadSign),
            CODE_UNAUTHENTICATED => Ok(Outcome::Expired),
            _ => Ok(Outcome::Failed(format!("接口返回 code={code} {message}"))),
        }
    }

    /// 分页拉取某容器（媒体库/合集/季/文件夹）的全部直属子项。
    pub fn item_list(&self, parent_guid: &str) -> Result<ItemListData> {
        collect_pages(parent_guid, |page| {
            let mut body = item_list_body(parent_guid);
            body["page"] = serde_json::json!(page);
            let value = self.item_list_with_body(body)?;
            serde_json::from_value(value)
                .map_err(|e| Error::network(format!("飞牛影视 item/list 响应解析失败：{e}")))
        })
    }

    /// 用自定义请求体调用 `item/list`，原样返回 `data`。
    ///
    /// 仅供诊断使用：飞牛没有公开参数语义，当结构识别异常或飞牛改版时，
    /// 用它做对照实验（换 `exclude_folder`、试分页参数），比猜更快。
    pub fn item_list_with_body(&self, body: serde_json::Value) -> Result<serde_json::Value> {
        self.send(API_ITEM_LIST, body, true).map_err(Error::api)
    }

    /// 拉取某容器子项的**原始**响应，不做任何字段映射。
    pub fn item_list_raw(&self, parent_guid: &str) -> Result<serde_json::Value> {
        self.item_list_with_body(item_list_body(parent_guid))
    }

    /// 列表中的网盘视频可能尚未探测，duration 为 0。
    /// 使用官方前端 /stream 的 Required(0) 模式获取媒体信息，不启动播放。
    fn resolve_duration(&self, item: &FnItem) -> Result<i64> {
        let media_guid = match clean(item.media_guid.as_deref()) {
            Some(guid) => guid,
            None => {
                let guid = clean(item.guid.as_deref())
                    .ok_or_else(|| Error::data("视频缺少条目 ID 和文件 ID，无法探测时长。"))?;
                let info = self
                    .send(API_PLAY_INFO, serde_json::json!({"item_guid": guid}), true)
                    .map_err(Error::api)?;
                clean(info.get("media_guid").and_then(|v| v.as_str()))
                    .ok_or_else(|| Error::data("视频信息未返回文件 ID，无法探测时长。"))?
            }
        };
        wait_for_duration(
            || {
                let data = self
                    .send(
                        API_STREAM,
                        serde_json::json!({
                            "media_guid": media_guid,
                            "level": 0,
                            "ip": "bili-planner",
                            "header": {"User-Agent": [crate::api::DEFAULT_UA]},
                        }),
                        true,
                    )
                    .map_err(Error::api)?;
                stream_duration(data)
            },
            || std::thread::sleep(Duration::from_secs(1)),
        )
    }

    fn complete_durations(
        &self,
        items: &mut [FnItem],
        progress: &mut dyn FnMut(String),
    ) -> Result<()> {
        complete_durations(items, |item| self.resolve_duration(item), progress)
    }
}

fn stream_duration(data: serde_json::Value) -> Result<i64> {
    #[derive(Deserialize)]
    struct StreamData {
        video_stream: FnItem,
    }
    let data: StreamData = serde_json::from_value(data)
        .map_err(|e| Error::data(format!("飞牛影视媒体信息解析失败：{e}")))?;
    // 成功响应中的 0 可能表示网盘媒体仍在探测，交给调用方有限重查。
    Ok(duration_of(&data.video_stream))
}

fn wait_for_duration<F, W>(mut fetch: F, mut wait: W) -> Result<i64>
where
    F: FnMut() -> Result<i64>,
    W: FnMut(),
{
    for attempt in 1..=DURATION_POLL_ATTEMPTS {
        let duration = fetch()?;
        if duration > 0 {
            return Ok(duration);
        }
        if attempt < DURATION_POLL_ATTEMPTS {
            wait();
        }
    }
    Err(Error::data(format!(
        "飞牛影视媒体信息仍未准备就绪（已查询 {DURATION_POLL_ATTEMPTS} 次），请稍后重试。"
    )))
}

/// 有限并发探测，按原索引写回，避免完成先后顺序影响课程顺序。
/// progress 始终在调用线程执行；失败时不返回部分课程。
fn complete_durations<F>(
    items: &mut [FnItem],
    resolve: F,
    progress: &mut dyn FnMut(String),
) -> Result<()>
where
    F: Fn(&FnItem) -> Result<i64> + Sync,
{
    let mut pending = Vec::new();
    let mut total = 0;
    for (index, item) in items.iter().enumerate() {
        if item.is_container() {
            if clean(item.guid.as_deref()).is_none() {
                return Err(Error::data(format!(
                    "飞牛影视目录「{}」缺少 ID，无法展开，当前结果不完整。",
                    item.display_title("未命名目录")
                )));
            }
        } else {
            total += 1;
            if duration_of(item) <= 0 {
                pending.push(index);
            }
        }
    }
    let mut ready = total - pending.len();
    progress(format!("当前目录：{ready} / {total} 个视频时长已就绪"));
    if pending.is_empty() {
        return Ok(());
    }
    progress(format!(
        "当前目录：{ready} / {total} 个视频时长已就绪，正在读取网盘媒体信息…"
    ));
    let next = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let (tx, rx) = mpsc::channel();
    let mut resolved = Vec::new();
    let mut first_error = None;
    std::thread::scope(|scope| {
        let items = &*items;
        for _ in 0..DURATION_WORKERS.min(pending.len()) {
            let (tx, pending, next, failed, resolve) =
                (tx.clone(), &pending, &next, &failed, &resolve);
            scope.spawn(move || {
                while !failed.load(Ordering::Relaxed) {
                    let Some(&index) = pending.get(next.fetch_add(1, Ordering::Relaxed)) else {
                        break;
                    };
                    let item = &items[index];
                    let result = resolve(item)
                        .and_then(|duration| {
                            if duration > 0 {
                                Ok(duration)
                            } else {
                                Err(Error::data("媒体探测未返回有效时长。"))
                            }
                        })
                        .map_err(|e| {
                            Error::data(format!(
                        "飞牛影视视频「{}」时长补全失败：{} 当前结果不完整，请稍后重新获取。",
                        item.display_title("未命名视频"), e.message()
                    ))
                        });
                    if result.is_err() {
                        failed.store(true, Ordering::Relaxed);
                    }
                    if tx.send((index, result)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        for (index, result) in rx {
            match result {
                Ok(duration) => {
                    resolved.push((index, duration));
                    ready += 1;
                    if first_error.is_none() {
                        progress(format!("当前目录：{ready} / {total} 个视频时长已就绪"));
                    }
                }
                Err(error) => {
                    if first_error.is_none() {
                        progress("视频时长探测失败，正在结束剩余请求…".to_string());
                        first_error = Some(error);
                    }
                }
            }
        }
    });
    if let Some(error) = first_error {
        return Err(error);
    }
    for (index, duration) in resolved {
        items[index].duration = Some(duration);
    }
    Ok(())
}

/// `item/list` 的默认请求体。
///
/// 单独提出，便于诊断工具在它的基础上做变体实验。
pub fn item_list_body(parent_guid: &str) -> serde_json::Value {
    serde_json::json!({
        "parent_guid": parent_guid,
        // 0 = 保留文件夹子项，遍历时需要下钻。
        "exclude_folder": 0,
        "sort_column": "sort_title",
        "sort_type": "ASC",
        "page": 1,
        "page_size": PAGE_SIZE,
    })
}

/// 飞牛的页码从 1 开始。参考客户端：
/// https://github.com/jxxghp/MoviePilot/blob/v3/app/modules/trimemedia/api.py
/// 以 total 为准，不能根据请求的 page_size 判断末页（服务端可能限制页长）。
/// 无 total 时继续到空页；分页不前进、提前空页均报错，避免返回残缺结果。
fn collect_pages<F>(parent_guid: &str, mut fetch: F) -> Result<ItemListData>
where
    F: FnMut(usize) -> Result<ItemListData>,
{
    let mut result = ItemListData::default();
    let mut seen = HashSet::new();
    let mut seen_pages = HashSet::new();
    for page in 1..=MAX_PAGES {
        let data = fetch(page)?;
        if result.mdb_name.is_none() {
            result.mdb_name = data.mdb_name;
        }
        if let Some(total) = data.total.filter(|n| *n >= 0) {
            if result.total.is_some_and(|previous| previous != total) {
                return Err(Error::data(
                    "飞牛影视目录总数在读取期间发生变化，请重新获取。",
                ));
            }
            result.total = Some(total);
        }
        if data.list.is_empty() {
            if result
                .total
                .is_some_and(|total| result.list.len() < total as usize)
            {
                return Err(Error::data(format!(
                    "飞牛影视目录 {parent_guid} 返回不完整：应有 {} 项，仅获取 {} 项，第 {page} 页为空。",
                    result.total.unwrap(), result.list.len()
                )));
            }
            return Ok(result);
        }
        if !seen_pages.insert(format!("{:?}", data.list)) {
            return Err(Error::data(format!(
                "飞牛影视目录 {parent_guid} 第 {page} 页重复，分页未前进。"
            )));
        }
        let before = result.list.len();
        for item in data.list {
            // 不按标题去重：同名视频是合法的不同文件。
            // 缺 guid 时保留条目，整页指纹另行检测重复页。
            if clean(item.guid.as_deref()).is_none_or(|guid| seen.insert(guid)) {
                result.list.push(item);
            }
        }
        if result.list.len() == before {
            return Err(Error::data(format!(
                "飞牛影视目录 {parent_guid} 第 {page} 页重复，分页未前进（已获取 {} 项）。请检查服务端分页接口。",
                result.list.len()
            )));
        }
        if let Some(total) = result.total {
            if result.list.len() > total as usize {
                return Err(Error::data("飞牛影视返回条数超过目录总数，请重新获取。"));
            }
            if result.list.len() == total as usize {
                return Ok(result);
            }
        }
    }
    Err(Error::data(
        "飞牛影视目录分页超过安全上限，已停止读取，未返回部分结果。",
    ))
}

fn truncate(text: &str, max_chars: usize) -> String {
    let mut out: String = text.chars().take(max_chars).collect();
    if text.chars().count() > max_chars {
        out.push('…');
    }
    out
}

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
    let mut items = client.item_list(guid)?.list;
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
    let root = client.item_list(&guid)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn page_items(ids: &[&str], total: Option<i64>) -> ItemListData {
        ItemListData {
            total,
            list: ids
                .iter()
                .map(|id| FnItem {
                    guid: Some((*id).into()),
                    title: Some("同名视频".into()),
                    kind: Some("Video".into()),
                    duration: Some(60),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn pagination_reads_short_pages_until_total_and_preserves_same_titles() {
        let mut calls = Vec::new();
        let result = collect_pages("root", |page| {
            calls.push(page);
            Ok(match page {
                1 => page_items(&["a", "b"], Some(4)),
                2 => page_items(&["b", "c"], Some(4)),
                3 => page_items(&["d"], Some(4)),
                _ => panic!("不应再请求"),
            })
        })
        .unwrap();
        assert_eq!(result.list.len(), 4);
        assert_eq!(calls, [1, 2, 3]);
        assert_eq!(result.list[3].guid.as_deref(), Some("d"));
    }

    #[test]
    fn pagination_without_total_reads_until_empty() {
        let result = collect_pages("root", |page| {
            Ok(match page {
                1 => page_items(&["a"], None),
                2 => page_items(&["b"], None),
                3 => page_items(&[], None),
                _ => panic!("不应再请求"),
            })
        })
        .unwrap();
        assert_eq!(result.list.len(), 2);
    }

    #[test]
    fn pagination_rejects_repeated_pages_and_premature_end() {
        for total in [None, Some(3)] {
            let err = collect_pages("root", |_| Ok(page_items(&["a"], total))).unwrap_err();
            assert!(err.message().contains("分页未前进"));
        }
        let err = collect_pages("root", |page| {
            Ok(page_items(if page == 1 { &["a"] } else { &[] }, Some(3)))
        })
        .unwrap_err();
        assert!(err.message().contains("应有 3 项，仅获取 1 项"));
    }

    #[test]
    fn pagination_propagates_later_failure_and_detects_changed_total() {
        let err = collect_pages("root", |page| {
            if page == 1 {
                Ok(page_items(&["a"], Some(2)))
            } else {
                Err(Error::network("断线"))
            }
        })
        .unwrap_err();
        assert_eq!(err.message(), "断线");
        let err = collect_pages("root", |page| {
            Ok(match page {
                1 => page_items(&["a"], Some(2)),
                _ => page_items(&["b"], Some(3)),
            })
        })
        .unwrap_err();
        assert!(err.message().contains("总数在读取期间发生变化"));
    }

    #[test]
    fn missing_list_is_an_error_and_total_count_is_supported() {
        assert!(serde_json::from_str::<ItemListData>(r#"{"total":32}"#).is_err());
        let data: ItemListData = serde_json::from_str(r#"{"total_count":"32","list":[]}"#).unwrap();
        assert_eq!(data.total, Some(32));
    }

    #[test]
    fn cloud_folder_resolves_22_zero_durations_and_keeps_all_32_videos() {
        // 真实故障的脱敏契约：32 个 Video，22 个 duration=0，均有 media_guid。
        let mut items: Vec<FnItem> = (1..=32)
            .map(|n| FnItem {
                guid: Some(format!("item-{n}")),
                media_guid: Some(format!("file-{n}")),
                kind: Some("Video".into()),
                title: Some(format!("课程 {n}")),
                duration: Some(if (20..=29).contains(&n) { 600 } else { 0 }),
                ..Default::default()
            })
            .collect();
        let probes = AtomicUsize::new(0);
        complete_durations(
            &mut items,
            |item| {
                assert_eq!(duration_of(item), 0);
                probes.fetch_add(1, Ordering::Relaxed);
                stream_duration(serde_json::json!({"video_stream":{"duration":1692}}))
            },
            &mut |_| {},
        )
        .unwrap();
        let (_, groups, _) =
            classify_children(items, Some("课程"), |_| panic!("视频不应下钻")).unwrap();
        assert_eq!(probes.load(Ordering::Relaxed), 22);
        assert_eq!(groups[0].episodes.len(), 32);
        assert_eq!(groups[0].episodes[0].duration, 1692);
        assert_eq!(groups[0].episodes[19].duration, 600);
    }

    #[test]
    fn pending_stream_duration_is_retried_but_failure_is_bounded() {
        let mut values = [0, 0, 2338].into_iter();
        let mut waits = 0;
        assert_eq!(
            wait_for_duration(|| Ok(values.next().unwrap()), || waits += 1).unwrap(),
            2338
        );
        assert_eq!(waits, 2);
        let mut calls = 0;
        let err = wait_for_duration(
            || {
                calls += 1;
                Ok(0)
            },
            || {},
        )
        .unwrap_err();
        assert_eq!(calls, DURATION_POLL_ATTEMPTS);
        assert!(err.message().contains("仍未准备就绪"));
        let err = wait_for_duration(|| Err(Error::api("无权限")), || panic!("不应重试业务错误"))
            .unwrap_err();
        assert_eq!(err.message(), "无权限");
    }

    #[test]
    fn concurrent_duration_completion_preserves_order_and_reports_progress() {
        let mut items: Vec<_> = (0..9)
            .map(|n| FnItem {
                title: Some(n.to_string()),
                kind: Some("Video".into()),
                duration: Some(0),
                ..Default::default()
            })
            .collect();
        let active = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let mut messages = Vec::new();
        complete_durations(
            &mut items,
            |item| {
                let n: u64 = item.title.as_ref().unwrap().parse().unwrap();
                let running = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(running, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(10 * (3 - n % 3)));
                active.fetch_sub(1, Ordering::SeqCst);
                Ok(100 + n as i64)
            },
            &mut |message| messages.push(message),
        )
        .unwrap();
        assert!(peak.load(Ordering::SeqCst) > 1);
        assert!(peak.load(Ordering::SeqCst) <= DURATION_WORKERS);
        assert_eq!(
            items.iter().map(duration_of).collect::<Vec<_>>(),
            (100..109).collect::<Vec<_>>()
        );
        for ready in 0..=9 {
            assert!(messages
                .iter()
                .any(|s| s == &format!("当前目录：{ready} / 9 个视频时长已就绪")));
        }
    }

    #[test]
    fn concurrent_duration_failure_does_not_commit_partial_results() {
        let mut items = vec![
            FnItem {
                title: Some("正常".into()),
                kind: Some("Video".into()),
                duration: Some(0),
                ..Default::default()
            },
            FnItem {
                title: Some("故障".into()),
                kind: Some("Video".into()),
                duration: Some(0),
                ..Default::default()
            },
        ];
        let err = complete_durations(
            &mut items,
            |item| {
                if item.title.as_deref() == Some("故障") {
                    Err(Error::network("超时"))
                } else {
                    Ok(100)
                }
            },
            &mut |_| {},
        )
        .unwrap_err();
        assert!(err.message().contains("故障"));
        assert!(err.message().contains("超时"));
        assert!(items.iter().all(|item| duration_of(item) == 0));
    }

    #[test]
    fn duration_completion_skips_containers_and_runtime_and_fails_explicitly() {
        let mut items = vec![
            FnItem {
                kind: Some("Directory".into()),
                guid: Some("folder".into()),
                ..Default::default()
            },
            FnItem {
                kind: Some("Video".into()),
                runtime: Some(10),
                ..Default::default()
            },
        ];
        complete_durations(&mut items, |_| panic!("不需要探测"), &mut |_| {}).unwrap();
        let mut items = vec![FnItem {
            kind: Some("Video".into()),
            title: Some("缺失课时".into()),
            ..Default::default()
        }];
        let err = complete_durations(&mut items, |_| Err(Error::api("探测失败")), &mut |_| {})
            .unwrap_err();
        assert!(err.message().contains("缺失课时"));
        assert!(err.message().contains("当前结果不完整"));
        assert_eq!(
            stream_duration(serde_json::json!({"video_stream":{"duration":0}})).unwrap(),
            0
        );
        assert!(stream_duration(serde_json::json!({})).is_err());
        assert_eq!(
            stream_duration(serde_json::json!({"video_stream":{"duration":"1692"}})).unwrap(),
            1692
        );
    }

    #[test]
    fn md5_matches_rfc1321_vectors() {
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            md5_hex(b"The quick brown fox jumps over the lazy dog"),
            "9e107d9d372bb6826bd81d3542a419d6"
        );
    }

    #[test]
    fn authx_uses_exactly_the_signed_bytes() {
        // 签名与发送共用同一份字节：同一字节 → 同一签名。
        let a = gen_authx(
            "/v/api/v1/item/list",
            b"{\"a\":1}",
            "123456",
            1_700_000_000_000,
        );
        let b = gen_authx(
            "/v/api/v1/item/list",
            b"{\"a\":1}",
            "123456",
            1_700_000_000_000,
        );
        assert_eq!(a, b);
        // 键序不同 → 字节不同 → 签名不同（因此必须发送被签名的那份字节）。
        let c = gen_authx(
            "/v/api/v1/item/list",
            b"{\"a\":1,\"b\":2}",
            "123456",
            1_700_000_000_000,
        );
        let d = gen_authx(
            "/v/api/v1/item/list",
            b"{\"b\":2,\"a\":1}",
            "123456",
            1_700_000_000_000,
        );
        assert_ne!(c, d);
        assert!(a.starts_with("nonce=123456&timestamp=1700000000000&sign="));
    }

    #[test]
    fn extract_guid_handles_urls_and_bare_ids() {
        assert_eq!(
            extract_guid("http://10.0.0.6:5666/v/video?guid=fv_30006e2fdaa44c7aac2c3cb25c10121d"),
            Some("fv_30006e2fdaa44c7aac2c3cb25c10121d".to_string())
        );
        assert_eq!(
            extract_guid("http://host:5666/v/index/#!/details?id=abcdef0123456789abcdef0123456789"),
            Some("abcdef0123456789abcdef0123456789".to_string())
        );
        assert_eq!(
            extract_guid("http://host:5666/v/video/fv_30006e2fdaa44c7aac2c3cb25c10121d"),
            Some("fv_30006e2fdaa44c7aac2c3cb25c10121d".to_string())
        );
        assert_eq!(
            extract_guid("fv_30006e2fdaa44c7aac2c3cb25c10121d"),
            Some("fv_30006e2fdaa44c7aac2c3cb25c10121d".to_string())
        );
        // 路由词不能被当成 guid。
        assert_eq!(extract_guid("http://host:5666/v/index/#!/details"), None);
        assert_eq!(extract_guid("http://host:5666/v/video"), None);
        assert_eq!(extract_guid(""), None);
        assert_eq!(extract_guid("   "), None);
    }

    #[test]
    fn duration_prefers_seconds_and_falls_back_to_runtime() {
        let mut item = FnItem {
            duration: Some(1500),
            runtime: Some(99),
            ..Default::default()
        };
        assert_eq!(duration_of(&item), 1500);
        // duration 为 0 时用 runtime（分钟）换算。
        item.duration = Some(0);
        assert_eq!(duration_of(&item), 99 * 60);
        // 两者都缺 → 0。
        item.runtime = None;
        assert_eq!(duration_of(&item), 0);
        // 数字以字符串返回时也能解析。
        let lenient: FnItem =
            serde_json::from_str(r#"{"duration":"600","episode_number":"3"}"#).unwrap();
        assert_eq!(duration_of(&lenient), 600);
        assert_eq!(lenient.episode_number, Some(3));
    }

    #[test]
    fn container_detection_follows_type_then_duration() {
        let container = FnItem {
            kind: Some("Season".to_string()),
            duration: Some(9999),
            ..Default::default()
        };
        assert!(container.is_container());

        let leaf = FnItem {
            kind: Some("Episode".to_string()),
            duration: Some(0),
            ..Default::default()
        };
        assert!(!leaf.is_container());

        // 未知类型：有时长当叶子，无时长当容器继续下钻。
        let unknown_with_duration = FnItem {
            kind: Some("NewKind".to_string()),
            duration: Some(120),
            ..Default::default()
        };
        assert!(!unknown_with_duration.is_container());
        let unknown_without_duration = FnItem {
            kind: Some("NewKind".to_string()),
            ..Default::default()
        };
        assert!(unknown_without_duration.is_container());
    }

    #[test]
    fn order_items_sorts_only_when_numbering_is_complete() {
        let numbered = vec![
            FnItem {
                episode_number: Some(2),
                season_number: Some(1),
                title: Some("二".into()),
                ..Default::default()
            },
            FnItem {
                episode_number: Some(1),
                season_number: Some(1),
                title: Some("一".into()),
                ..Default::default()
            },
        ];
        let ordered = order_items(numbered);
        assert_eq!(ordered[0].title.as_deref(), Some("一"));

        // 缺集号时保持原顺序，避免把服务端有意排好的顺序打乱。
        let partial = vec![
            FnItem {
                episode_number: Some(2),
                title: Some("keep-a".into()),
                ..Default::default()
            },
            FnItem {
                episode_number: None,
                title: Some("keep-b".into()),
                ..Default::default()
            },
        ];
        let ordered = order_items(partial);
        assert_eq!(ordered[0].title.as_deref(), Some("keep-a"));
    }

    #[test]
    fn classify_groups_containers_and_loose_leaves() {
        let children = vec![
            FnItem {
                guid: Some("g-season-1".into()),
                title: Some("第 1 季".into()),
                kind: Some("Season".into()),
                parent_title: Some("示例剧集".into()),
                ..Default::default()
            },
            FnItem {
                guid: Some("g-season-2".into()),
                title: Some("第 2 季".into()),
                kind: Some("Season".into()),
                ..Default::default()
            },
            FnItem {
                title: Some("花絮".into()),
                kind: Some("Video".into()),
                duration: Some(300),
                ..Default::default()
            },
        ];

        let (title, groups, structure) = classify_children(children, Some("测试库"), |guid| {
            Ok(vec![EpisodeItem {
                title: format!("{guid}-ep"),
                duration: 600,
            }])
        })
        .unwrap();

        // 根标题由子项的 parent_title 反推。
        assert_eq!(title, "示例剧集");
        assert_eq!(groups.len(), 3);
        // 散件组排在容器组之前。
        assert_eq!(groups[0].name, "示例剧集（散件）");
        assert_eq!(groups[0].episodes[0].duration, 300);
        assert_eq!(groups[1].name, "第 1 季");
        assert_eq!(groups[1].episodes[0].title, "g-season-1-ep");
        assert_eq!(groups[2].name, "第 2 季");
        assert!(structure.contains("多层级合集"));
    }

    #[test]
    fn classify_single_flat_season_becomes_one_course() {
        let children = vec![
            FnItem {
                title: Some("第一课".into()),
                kind: Some("Episode".into()),
                episode_number: Some(1),
                duration: Some(600),
                parent_title: Some("示例课程".into()),
                ..Default::default()
            },
            FnItem {
                title: Some("第二课".into()),
                kind: Some("Episode".into()),
                episode_number: Some(2),
                duration: Some(900),
                ..Default::default()
            },
        ];
        let (title, groups, structure) =
            classify_children(children, Some("测试库"), |_| unreachable!()).unwrap();
        assert_eq!(title, "示例课程");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "示例课程");
        assert_eq!(groups[0].episodes[0].title, "第1集 第一课");
        assert_eq!(groups[0].episodes[1].duration, 900);
        assert!(structure.contains("单合集"));
    }

    #[test]
    fn classify_rejects_content_without_duration() {
        let children = vec![FnItem {
            title: Some("无时长".into()),
            kind: Some("Episode".into()),
            duration: Some(0),
            ..Default::default()
        }];
        assert!(classify_children(children, None, |_| Ok(Vec::new())).is_err());
    }

    #[test]
    fn root_title_degrades_gracefully() {
        let empty: Vec<FnItem> = Vec::new();
        assert_eq!(derive_root_title(&empty, Some("媒体库")), "媒体库");
        assert_eq!(derive_root_title(&empty, None), "飞牛影视合集");
    }
}
