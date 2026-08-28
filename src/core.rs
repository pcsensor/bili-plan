//! 业务编排层（无 GUI 依赖）：状态数据、获取/生成/导出入口与凭证持久化。
//!
//! 数据来源接入 Bilibili 与 Jellyfin 两条适配器（`api`/`parse` 与
//! `jellyfin`），统一在 [`fetch_and_parse`] 按 [`FetchSource`] 分派；核心
//! 计划算法在 `plan`/`export` 模块，本模块只做纯函数编排，供任意 UI 层
//! （gpui-component）驱动。

use std::path::PathBuf;

use crate::api;
use crate::export;
use crate::parse::{self, EpisodeItem, Group};
use crate::plan::{build_plan, Mode, PlanEntry};
use crate::{extract_sid, Error};

// ---------------------------------------------------------------------------
// 状态数据
// ---------------------------------------------------------------------------

/// 科目统计范围选择。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    All,
    Single(usize),
}

/// 视频来源：决定 `fetch_and_parse` 走 B 站还是 Jellyfin 适配器。
///
/// UI 通过来源切换控件构造本枚举传给 `fetch_and_parse`，
/// 实现一处入口、两条适配器路径。
#[derive(Debug, Clone)]
pub enum FetchSource {
    /// B 站：可选 Cookie（SESSDATA），用于风控时匿名→登录升级。
    Bilibili { cookie: Option<String> },
    /// Jellyfin：服务器地址 + API Token（必填）。
    Jellyfin { server_url: String, token: String },
}

/// UI 来源切换的状态枚举（与 `plan::Mode` 同样模式：`from_index`/`index`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceMode {
    #[default]
    Bilibili,
    Jellyfin,
}

impl SourceMode {
    pub fn from_index(i: usize) -> Self {
        if i == 0 {
            Self::Bilibili
        } else {
            Self::Jellyfin
        }
    }
    pub fn index(self) -> usize {
        match self {
            Self::Bilibili => 0,
            Self::Jellyfin => 1,
        }
    }
}

/// 一条搜索历史（仅记录获取成功的输入）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HistoryEntry {
    /// 用户输入的链接 / BV 号 / item ID（trim 后）。
    pub input: String,
    /// 来源："bilibili" 或 "jellyfin"。
    pub source: String,
    /// 获取成功时的合集标题（用于列表展示，可为空）。
    pub title: String,
    /// 记录时间（unix 秒）。
    pub at: i64,
}

/// 持久化到本机的应用配置（JSON 文件，家目录下）。
///
/// 字段保持扁平：旧版本文件只有 `server_url`/`token` 两个键，
/// `history` 缺省为空即可读入，避免升级丢凭证。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct AppConfig {
    pub server_url: String,
    pub token: String,
    #[serde(default)]
    pub history: Vec<HistoryEntry>,
}

/// 历史记录上限（超出后丢弃最旧的）。
pub const HISTORY_LIMIT: usize = 20;

/// 来源枚举 → 历史记录里的字符串标记。
fn source_tag(source: SourceMode) -> &'static str {
    match source {
        SourceMode::Bilibili => "bilibili",
        SourceMode::Jellyfin => "jellyfin",
    }
}

/// 记录一条搜索历史：按（来源, 输入）去重、最新在前、超限截断。
/// 纯内存操作，调用方决定何时 [`save_config`]。
pub fn record_history(cfg: &mut AppConfig, source: SourceMode, input: &str, title: &str) {
    let input = input.trim();
    if input.is_empty() {
        return;
    }
    let tag = source_tag(source);
    cfg.history
        .retain(|h| !(h.source == tag && h.input == input));
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    cfg.history.insert(
        0,
        HistoryEntry {
            input: input.to_string(),
            source: tag.to_string(),
            title: title.trim().to_string(),
            at,
        },
    );
    cfg.history.truncate(HISTORY_LIMIT);
}

/// 删除第 `index` 条历史；越界时静默忽略。
pub fn remove_history(cfg: &mut AppConfig, index: usize) {
    if index < cfg.history.len() {
        cfg.history.remove(index);
    }
}

/// 清空全部历史。
pub fn clear_history(cfg: &mut AppConfig) {
    cfg.history.clear();
}

/// 已获取的合集/视频数据。
#[derive(Debug, Clone)]
pub struct ReadyState {
    pub season_title: String,
    pub structure: String,
    pub groups: Vec<Group>,
    pub selection: Selection,
    pub plan: Option<PlanData>,
}

/// 已生成的观看计划。
#[derive(Debug, Clone)]
pub struct PlanData {
    pub plan: Vec<Vec<PlanEntry>>,
    pub capacities: Vec<i64>,
    pub total: i64,
    pub days: i64,
    pub avg: f64,
    pub scope_desc: String,
}

// ---------------------------------------------------------------------------
// 纯函数编排
// ---------------------------------------------------------------------------

/// 按来源分派获取视频/合集信息并识别结构。
///
/// - `FetchSource::Bilibili`：sid → 归档接口；否则 BV → view API → parse_groups
/// - `FetchSource::Jellyfin`：`JellyfinClient` → `jellyfin::fetch_groups`
///
/// 两条路径共用 `ReadyState` 构造与默认选择策略（多科目→All，单科目→Single(0)），
/// 后续计划生成、表格、导出与来源无关。
pub fn fetch_and_parse(input: &str, source: &FetchSource) -> Result<ReadyState, String> {
    let r = (|| -> crate::Result<ReadyState> {
        let (season_title, groups, structure) = match source {
            FetchSource::Bilibili { cookie } => {
                let cookie = cookie.as_deref();
                if let Some(sid) = extract_sid(input) {
                    let flat = api::fetch_season_archives(sid, cookie)?;
                    if flat.is_empty() {
                        return Err(Error::data("未获取到任何视频，请检查链接是否正确。"));
                    }
                    let groups = vec![Group {
                        name: format!("合集{sid}"),
                        episodes: flat
                            .into_iter()
                            .map(|it| EpisodeItem {
                                title: it.title,
                                duration: it.duration,
                            })
                            .collect(),
                    }];
                    (
                        format!("合集{sid}"),
                        groups,
                        "合集归档接口（sid 链接）".to_string(),
                    )
                } else {
                    let bvid = parse::extract_bvid(input)?;
                    let view = api::fetch_view(&bvid, cookie)?;
                    let r = parse::parse_groups(&view, cookie, &parse::default_fallback)?;
                    (r.season_title, r.groups, r.structure)
                }
            }
            FetchSource::Jellyfin { server_url, token } => {
                if server_url.trim().is_empty() {
                    return Err(Error::input("请填写 Jellyfin 服务器地址。"));
                }
                if token.trim().is_empty() {
                    return Err(Error::input("请填写 Jellyfin API Token。"));
                }
                let client = crate::jellyfin::JellyfinClient::new(
                    server_url.trim().to_string(),
                    token.trim().to_string(),
                );
                crate::jellyfin::fetch_groups(&client, input)?
            }
        };
        let selection = if groups.len() > 1 {
            Selection::All
        } else {
            Selection::Single(0)
        };
        Ok(ReadyState {
            season_title,
            structure,
            groups,
            selection,
            plan: None,
        })
    })();
    r.map_err(|e| e.message().to_string())
}

/// 用当前科目选择与天数生成计划。
pub fn generate_plan(rd: &mut ReadyState, days: i64, mode: Mode) -> Result<(), String> {
    let (items, scope_desc) = match &rd.selection {
        Selection::All => {
            let mut items: Vec<EpisodeItem> = Vec::new();
            for (i, g) in rd.groups.iter().enumerate() {
                for ep in &g.episodes {
                    items.push(EpisodeItem {
                        title: format!("[科目{}] {}", i + 1, ep.title),
                        duration: ep.duration,
                    });
                }
            }
            (items, "整个合集（全部科目）".to_string())
        }
        Selection::Single(gi) => {
            let g = rd
                .groups
                .get(*gi)
                .ok_or_else(|| "科目编号超出范围".to_string())?;
            (
                g.episodes.clone(),
                format!("{}（{} 个视频）", g.name, g.episodes.len()),
            )
        }
    };
    let total: i64 = items.iter().map(|i| i.duration).sum();
    if total <= 0 {
        return Err("统计范围内视频总时长为 0。".to_string());
    }
    let out = build_plan(&items, days, mode)?;
    let avg = total as f64 / days as f64;
    rd.plan = Some(PlanData {
        plan: out.plan,
        capacities: out.capacities,
        total,
        days,
        avg,
        scope_desc,
    });
    Ok(())
}

/// 天数输入校验：非空、可解析、正整数。
pub fn parse_days(text: &str) -> std::result::Result<i64, String> {
    let t = text.trim();
    if t.is_empty() {
        return Err("请先填写目标观看天数（正整数）。".to_string());
    }
    let days: i64 = t.parse().map_err(|_| "天数必须是正整数。".to_string())?;
    if days <= 0 {
        return Err("天数必须是正整数。".to_string());
    }
    Ok(days)
}

/// 导出载荷：完整计划文本 + 建议文件名；未生成计划时返回 `None`。
pub fn export_payload(rd: &ReadyState, mode: Mode) -> Option<(String, String)> {
    let p = rd.plan.as_ref()?;
    let text = export::full_text(
        &rd.season_title,
        &rd.structure,
        &p.scope_desc,
        p.total,
        p.days,
        p.avg,
        &rd.groups,
        &p.plan,
        &p.capacities,
        p.total,
        mode,
    );
    let file = format!("观看计划_{}.txt", sanitize(&rd.season_title));
    Some((text, file))
}

/// 文件名安全化。
pub fn sanitize(s: &str) -> String {
    s.chars()
        .filter(|c| {
            !matches!(
                c,
                '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\n' | '\r'
            )
        })
        .take(40)
        .collect()
}

// ---------------------------------------------------------------------------
// Jellyfin 凭证持久化（0 新依赖：家目录 + serde_json）
// ---------------------------------------------------------------------------

/// 配置文件路径：用户家目录下 `.bili-planner.json`（比工作目录稳定，
/// 不同启动目录都能读到）。Windows 取 `%USERPROFILE%`，Unix 取 `$HOME`。
fn config_path() -> Option<PathBuf> {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let home = std::env::var(key).ok()?;
    Some(PathBuf::from(home).join(".bili-planner.json"))
}

/// 启动时尝试加载本机配置（Jellyfin 凭证 + 搜索历史）；
/// 文件不存在或损坏时静默返回 `None`。
pub fn load_config() -> Option<AppConfig> {
    let path = config_path()?;
    let data = std::fs::read_to_string(&path).ok()?;
    let cfg: AppConfig = serde_json::from_str(&data).ok()?;
    Some(cfg)
}

/// 把应用配置写到本机（pretty JSON）。失败静默：不阻塞主流程。
pub fn save_config(cfg: &AppConfig) {
    let Some(path) = config_path() else { return };
    if let Ok(json) = serde_json::to_string_pretty(cfg) {
        let _ = std::fs::write(&path, json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_days_rejects_empty_and_nonpositive() {
        assert!(parse_days("").is_err());
        assert!(parse_days("  ").is_err());
        assert!(parse_days("abc").is_err());
        assert!(parse_days("0").is_err());
        assert!(parse_days("-3").is_err());
        assert_eq!(parse_days(" 30 ").unwrap(), 30);
    }

    #[test]
    fn sanitize_strips_path_characters() {
        assert_eq!(sanitize("a/b:c*d?e\"f<g>h|i"), "abcdefghi");
        assert_eq!(sanitize("正常标题").len(), 12);
        assert_eq!(sanitize(&"x".repeat(100)).len(), 40);
    }

    #[test]
    fn source_mode_roundtrip() {
        assert_eq!(SourceMode::from_index(0), SourceMode::Bilibili);
        assert_eq!(SourceMode::from_index(1), SourceMode::Jellyfin);
        assert_eq!(SourceMode::Bilibili.index(), 0);
        assert_eq!(SourceMode::Jellyfin.index(), 1);
    }

    #[test]
    fn record_history_dedupes_and_prepends() {
        let mut cfg = AppConfig::default();
        record_history(&mut cfg, SourceMode::Bilibili, " BV1abc ", "合集A");
        record_history(&mut cfg, SourceMode::Jellyfin, "http://jf/?id=1", "课程B");
        assert_eq!(cfg.history.len(), 2);
        assert_eq!(cfg.history[0].source, "jellyfin");

        // 相同（来源, 输入）去重并置顶，不产生重复项。
        record_history(&mut cfg, SourceMode::Bilibili, "BV1abc", "合集A·新标题");
        assert_eq!(cfg.history.len(), 2);
        assert_eq!(cfg.history[0].title, "合集A·新标题");
        // 相同输入、不同来源视为两条。
        record_history(&mut cfg, SourceMode::Jellyfin, "BV1abc", "");
        assert_eq!(cfg.history.len(), 3);
        // 空白输入不记录。
        record_history(&mut cfg, SourceMode::Bilibili, "   ", "x");
        assert_eq!(cfg.history.len(), 3);
    }

    #[test]
    fn record_history_caps_at_limit() {
        let total = HISTORY_LIMIT as i64 + 5;
        let mut cfg = AppConfig::default();
        for i in 0..total {
            record_history(&mut cfg, SourceMode::Bilibili, &format!("BV{i:04}"), "");
        }
        assert_eq!(cfg.history.len(), HISTORY_LIMIT);
        // 最新的留在最前，最旧的被截断。
        assert_eq!(cfg.history[0].input, format!("BV{:04}", total - 1));
        assert_eq!(
            cfg.history.last().unwrap().input,
            format!("BV{:04}", total - HISTORY_LIMIT as i64)
        );
    }

    #[test]
    fn remove_and_clear_history() {
        let mut cfg = AppConfig::default();
        record_history(&mut cfg, SourceMode::Bilibili, "BV1", "A");
        record_history(&mut cfg, SourceMode::Bilibili, "BV2", "B");
        remove_history(&mut cfg, 0);
        assert_eq!(cfg.history.len(), 1);
        assert_eq!(cfg.history[0].input, "BV1");
        remove_history(&mut cfg, 99); // 越界静默
        assert_eq!(cfg.history.len(), 1);
        clear_history(&mut cfg);
        assert!(cfg.history.is_empty());
    }

    /// 旧版配置文件（只有 Jellyfin 两键）必须能无损读入。
    #[test]
    fn legacy_config_parses_with_empty_history() {
        let cfg: AppConfig =
            serde_json::from_str(r#"{"server_url": "http://jf:8096", "token": "t"}"#).unwrap();
        assert_eq!(cfg.server_url, "http://jf:8096");
        assert!(cfg.history.is_empty());
    }
}
