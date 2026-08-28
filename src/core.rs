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

/// 持久化到本机的 Jellyfin 凭证（JSON 文件，工作目录旁）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct JellyfinConfig {
    pub server_url: String,
    pub token: String,
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

/// 启动时尝试加载本机 Jellyfin 凭证；文件不存在或损坏时静默返回 `None`。
pub fn load_config() -> Option<JellyfinConfig> {
    let path = config_path()?;
    let data = std::fs::read_to_string(&path).ok()?;
    let cfg: JellyfinConfig = serde_json::from_str(&data).ok()?;
    if cfg.server_url.trim().is_empty() || cfg.token.trim().is_empty() {
        return None;
    }
    Some(cfg)
}

/// 把 Jellyfin 凭证写到本机（pretty JSON）。失败静默：不阻塞主流程。
pub fn save_config(cfg: &JellyfinConfig) {
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
}
