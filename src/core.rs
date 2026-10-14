//! 业务编排层（无 GUI 依赖）：状态数据、获取/生成/导出入口与凭证持久化。
//!
//! 数据来源接入 Bilibili 与 Jellyfin 两条适配器（`api`/`parse` 与
//! `jellyfin`），统一在 [`fetch_and_parse`] 按 [`FetchSource`] 分派；核心
//! 计划算法在 `plan`/`export` 模块，本模块只做纯函数编排，供任意 UI 层
//! （gpui-component）驱动。

use crate::api;
use crate::export;
use crate::parse::{self, EpisodeItem, Group};
use crate::plan::{build_plan, Mode, PlanEntry};
use crate::{extract_sid, Error};
use planner_domain::source::SourceKind;

mod cloud;
mod storage;
pub use cloud::{
    check_cloud_bind_status, clear_cloud_binding_state, delete_cloud_binding,
    request_cloud_bind_code, sync_with_cloud, CloudUnbindReport,
};
pub use storage::{load_config, save_config, try_save_config};
#[cfg(test)]
use storage::{load_config_at, LocalConfigStore};

// ---------------------------------------------------------------------------
// 状态数据
// ---------------------------------------------------------------------------

/// 科目统计范围选择。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    All,
    Single(usize),
}

/// 视频来源：决定 `fetch_and_parse` 走哪条适配器。
///
/// UI 通过来源切换控件构造本枚举传给 `fetch_and_parse`，
/// 实现一处入口、三条适配器路径。
#[derive(Debug, Clone)]
pub enum FetchSource {
    /// B 站：可选 Cookie（SESSDATA），用于风控时匿名→登录升级。
    Bilibili { cookie: Option<String> },
    /// Jellyfin：服务器地址 + API Token（必填）。
    Jellyfin { server_url: String, token: String },
    /// 飞牛影视：服务器地址 + 账号密码（必填，用于换取并缓存登录 token）。
    FnOs {
        base_url: String,
        username: String,
        password: String,
    },
}

/// UI 来源切换的状态枚举（与 `plan::Mode` 同样模式：`from_index`/`index`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceMode {
    #[default]
    Bilibili,
    Jellyfin,
    FnOs,
}

impl SourceMode {
    pub fn from_index(i: usize) -> Self {
        match i {
            0 => Self::Bilibili,
            1 => Self::Jellyfin,
            _ => Self::FnOs,
        }
    }
    pub fn index(self) -> usize {
        match self {
            Self::Bilibili => 0,
            Self::Jellyfin => 1,
            Self::FnOs => 2,
        }
    }
}

/// 一条搜索历史（仅记录获取成功的输入）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HistoryEntry {
    /// 用户输入的链接 / BV 号 / item ID（trim 后）。
    pub input: String,
    /// 来源："bilibili"、"jellyfin" 或 "fnos"。
    pub source: String,
    /// 获取成功时的合集标题（用于列表展示，可为空）。
    pub title: String,
    /// 记录时间（unix 秒）。
    pub at: i64,
}

use crate::study::{self, DailyNote, DailyNotes, PlanStatus, StudyPlan};

fn default_auto_sync() -> bool {
    true
}

/// 持久化到本机 SQLite 的应用配置。
///
/// 字段保持兼容：旧版 JSON 文件只有 `server_url`/`token` 两个键时仍可导入，
/// 缺省的历史、计划和备注会以空集合恢复，避免升级丢数据。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub server_url: String,
    #[serde(default)]
    pub token: String,
    /// 飞牛影视服务器地址（形如 http://host:5666）。
    #[serde(default)]
    pub fnos_server_url: String,
    /// 飞牛影视登录账号。
    #[serde(default)]
    pub fnos_username: String,
    /// 飞牛影视登录密码。与 Jellyfin Token 一样明文存本机 SQLite，
    /// 不做加密（与既有取舍保持一致）。
    #[serde(default)]
    pub fnos_password: String,
    #[serde(default)]
    pub history: Vec<HistoryEntry>,
    #[serde(default)]
    pub plans: Vec<StudyPlan>,
    #[serde(default, deserialize_with = "study::deserialize_daily_notes")]
    pub daily_notes: DailyNotes, // "YYYY-MM-DD" -> 多条备注（含同步删除标记）
    /// 云端同步服务地址。默认空：用户必须在「云端设置」里显式填写后才能绑定或同步。
    #[serde(default)]
    pub sync_server_url: String,
    /// 云端设备令牌，由服务端注册接口签发后持久化在本机，作为所有云端请求的
    /// `Authorization: Bearer` 凭据。
    #[serde(default)]
    pub sync_device_token: Option<String>,
    /// Desktop snapshot revision acknowledged by the cloud service.
    #[serde(default)]
    pub sync_revision: i64,
    #[serde(default)]
    pub feishu_bound: bool,
    #[serde(default)]
    pub feishu_user_name: Option<String>,
    #[serde(default)]
    pub telegram_bound: bool,
    #[serde(default)]
    pub telegram_user_name: Option<String>,
    #[serde(default = "default_auto_sync")]
    pub auto_sync: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server_url: String::new(),
            token: String::new(),
            fnos_server_url: String::new(),
            fnos_username: String::new(),
            fnos_password: String::new(),
            history: Vec::new(),
            plans: Vec::new(),
            daily_notes: DailyNotes::new(),
            sync_server_url: String::new(),
            sync_device_token: None,
            sync_revision: 0,
            feishu_bound: false,
            feishu_user_name: None,
            telegram_bound: false,
            telegram_user_name: None,
            auto_sync: true,
        }
    }
}

/// 历史记录上限（超出后丢弃最旧的）。
pub const HISTORY_LIMIT: usize = 20;

/// 来源枚举 → 历史记录里的字符串标记。
///
/// 同时作为 `StudyPlan.source_type` 落库，供 UI 还原来源图标与链接，
/// 因此对外公开，避免各调用点各写一份 match 漏掉新增来源。
pub fn source_tag(source: SourceMode) -> &'static str {
    match source {
        SourceMode::Bilibili => SourceKind::Bilibili.tag(),
        SourceMode::Jellyfin => SourceKind::Jellyfin.tag(),
        SourceMode::FnOs => SourceKind::FnOs.tag(),
    }
}

/// 记录一条搜索历史：按（来源, 输入）去重、最新在前、超限截断。
/// 纯内存操作，调用方决定何时 [`try_save_config`] / [`commit_config`]。
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
/// - `FetchSource::FnOs`：`FnOsClient`（登录换 token）→ `fnos::fetch_groups`
///
/// 三条路径共用 `ReadyState` 构造与默认选择策略（多科目→All，单科目→Single(0)），
/// 后续计划生成、表格、导出与来源无关。
pub fn fetch_and_parse(input: &str, source: &FetchSource) -> Result<ReadyState, String> {
    fetch_and_parse_with_progress(input, source, &mut |_| {})
}

/// 后台取数进度。回调运行于调用线程，UI 通过消息传递接收。
pub fn fetch_and_parse_with_progress(
    input: &str,
    source: &FetchSource,
    progress: &mut dyn FnMut(String),
) -> Result<ReadyState, String> {
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
            FetchSource::FnOs {
                base_url,
                username,
                password,
            } => {
                if base_url.trim().is_empty() {
                    return Err(Error::input("请填写飞牛影视服务器地址。"));
                }
                if username.trim().is_empty() {
                    return Err(Error::input("请填写飞牛影视账号。"));
                }
                if password.is_empty() {
                    return Err(Error::input("请填写飞牛影视密码。"));
                }
                let client = crate::fnos::FnOsClient::new(
                    base_url.trim(),
                    username.trim(),
                    password.clone(),
                );
                crate::fnos::fetch_groups_with_progress(&client, input, progress)?
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

/// 落盘成功后才把 `next` 写回实时配置，避免“内存已改、磁盘失败”。
fn commit_config(cfg: &mut AppConfig, next: AppConfig) -> Result<(), String> {
    try_save_config(&next)?;
    *cfg = next;
    Ok(())
}

/// 把已生成的计划保存到打卡学习计划库。
pub fn enroll_study_plan(
    cfg: &mut AppConfig,
    rd: &ReadyState,
    source_url: &str,
    source_type: &str,
    start_date_str: &str,
    skip_weekends: bool,
) -> Result<StudyPlan, String> {
    let plan_data = rd.plan.as_ref().ok_or_else(|| "尚未生成计划".to_string())?;
    let plan_out = crate::plan::PlanOut {
        plan: plan_data.plan.clone(),
        capacities: plan_data.capacities.clone(),
        total: plan_data.total,
    };

    let study_plan = study::create_study_plan(
        &rd.season_title,
        source_type,
        source_url,
        &plan_data.scope_desc,
        &plan_out,
        start_date_str,
        skip_weekends,
    )?;

    // 插入或更新
    let mut next = cfg.clone();
    next.plans.retain(|p| p.id != study_plan.id);
    next.plans.insert(0, study_plan.clone());
    commit_config(cfg, next)?;
    Ok(study_plan)
}

/// 新建一个自定义的每日学习任务并写入计划库。
pub fn add_custom_study_plan(
    cfg: &mut AppConfig,
    title: &str,
    start_date: &str,
    days: i64,
    daily_minutes: i64,
    skip_weekends: bool,
) -> Result<StudyPlan, String> {
    let plan =
        study::create_custom_study_plan(title, start_date, days, daily_minutes, skip_weekends)?;
    let mut next = cfg.clone();
    next.plans.insert(0, plan.clone());
    commit_config(cfg, next)?;
    Ok(plan)
}

/// 从日历添加一次性任务：参与当天打卡和统计，但不展示在“我的计划库”。
pub fn add_one_off_calendar_task(
    cfg: &mut AppConfig,
    task_title: &str,
    date: &str,
    minutes: i64,
) -> Result<StudyPlan, String> {
    let plan = study::create_one_off_calendar_task(task_title, date, minutes)?;
    let mut next = cfg.clone();
    next.plans.insert(0, plan.clone());
    commit_config(cfg, next)?;
    Ok(plan)
}

/// 从日历创建可持续追加每日任务的系列计划。系列计划显示在“我的计划库”。
pub fn create_calendar_series_plan(
    cfg: &mut AppConfig,
    series_name: &str,
    task_title: &str,
    date: &str,
    minutes: i64,
) -> Result<StudyPlan, String> {
    let plan = study::create_calendar_series(series_name, task_title, date, minutes)?;
    let mut next = cfg.clone();
    next.plans.insert(0, plan.clone());
    commit_config(cfg, next)?;
    Ok(plan)
}

/// 向已有系列计划追加一个指定日期的日任务，并自动更新其起止日期与汇总。
pub fn append_calendar_series_task(
    cfg: &mut AppConfig,
    plan_id: &str,
    task_title: &str,
    date: &str,
    minutes: i64,
) -> Result<(), String> {
    let mut next = cfg.clone();
    let plan = next
        .plans
        .iter_mut()
        .find(|plan| plan.id == plan_id)
        .ok_or_else(|| "未找到指定的系列计划。".to_string())?;
    study::append_calendar_series_task(plan, task_title, date, minutes)?;
    commit_config(cfg, next)
}

/// 手动调整任意来源任务的日期，并沿用本地持久化流程。
pub fn move_study_task_to_date(
    cfg: &mut AppConfig,
    plan_id: &str,
    task_id: &str,
    date: &str,
) -> Result<(), String> {
    let mut next = cfg.clone();
    let plan = next
        .plans
        .iter_mut()
        .find(|plan| plan.id == plan_id)
        .ok_or_else(|| "未找到指定任务所属计划。".to_string())?;
    study::move_task_to_date(plan, task_id, date)?;
    commit_config(cfg, next)
}

/// 编辑右键日历创建的任务。
pub fn update_calendar_task(
    cfg: &mut AppConfig,
    plan_id: &str,
    task_id: &str,
    task_title: &str,
    date: &str,
    minutes: i64,
) -> Result<(), String> {
    let mut next = cfg.clone();
    let plan = next
        .plans
        .iter_mut()
        .find(|plan| plan.id == plan_id)
        .ok_or_else(|| "未找到指定任务所属计划。".to_string())?;
    study::update_calendar_task(plan, task_id, task_title, date, minutes)?;
    commit_config(cfg, next)
}

/// 删除右键日历创建的任务；删除系列的最后一项时会同时移除空系列计划。
pub fn delete_calendar_task(
    cfg: &mut AppConfig,
    plan_id: &str,
    task_id: &str,
) -> Result<(), String> {
    let mut next = cfg.clone();
    let plan_index = next
        .plans
        .iter()
        .position(|plan| plan.id == plan_id)
        .ok_or_else(|| "未找到指定任务所属计划。".to_string())?;
    let remove_plan = study::delete_calendar_task(&mut next.plans[plan_index], task_id)?;
    if remove_plan {
        next.plans.remove(plan_index);
    }
    commit_config(cfg, next)
}

/// 删除指定计划。
pub fn remove_study_plan(cfg: &mut AppConfig, plan_id: &str) -> Result<(), String> {
    let mut next = cfg.clone();
    next.plans.retain(|p| p.id != plan_id);
    commit_config(cfg, next)
}

/// 切换计划状态（在 Active 和 Paused 之间切换）。
pub fn toggle_study_plan_status(cfg: &mut AppConfig, plan_id: &str) -> Result<(), String> {
    let mut next = cfg.clone();
    let Some(plan) = next.plans.iter_mut().find(|p| p.id == plan_id) else {
        return Ok(());
    };
    plan.status = match plan.status {
        PlanStatus::Active => PlanStatus::Paused,
        PlanStatus::Paused => PlanStatus::Active,
        other => other,
    };
    commit_config(cfg, next)
}

/// 切换某个任务的打卡状态并持久化。
pub fn checkin_study_task(
    cfg: &mut AppConfig,
    plan_id: &str,
    task_id: &str,
) -> Result<bool, String> {
    let mut next = cfg.clone();
    let res = study::toggle_task_checkin(&mut next.plans, plan_id, task_id)?;
    commit_config(cfg, next)?;
    Ok(res)
}

/// 一键顺延指定计划并持久化。
pub fn push_forward_study_plan(
    cfg: &mut AppConfig,
    plan_id: &str,
    destination_date: &str,
) -> Result<bool, String> {
    let mut next = cfg.clone();
    let plan = next
        .plans
        .iter_mut()
        .find(|p| p.id == plan_id)
        .ok_or_else(|| "未找到指定计划".to_string())?;
    let changed = study::push_forward_plan(plan, destination_date)?;
    if changed {
        commit_config(cfg, next)?;
    }
    Ok(changed)
}

/// 固定计划结束日期，从新起点重新分配未完成时长。
pub fn reschedule_unfinished_study_plan(
    cfg: &mut AppConfig,
    plan_id: &str,
    target_start_date: &str,
) -> Result<bool, String> {
    save_plan_change(cfg, plan_id, |plan| {
        study::reschedule_unfinished_plan(plan, target_start_date)
    })
}

/// 按新的结束日期重排剩余任务；本地写入成功后才更新实时配置。
pub fn redistribute_unfinished_study_plan(
    cfg: &mut AppConfig,
    plan_id: &str,
    current_date: &str,
    target_end_date: &str,
) -> Result<bool, String> {
    save_plan_change(cfg, plan_id, |plan| {
        study::redistribute_unfinished_plan(plan, current_date, target_end_date)
    })
}

fn save_plan_change(
    cfg: &mut AppConfig,
    plan_id: &str,
    change: impl FnOnce(&mut StudyPlan) -> Result<bool, String>,
) -> Result<bool, String> {
    let mut next = cfg.clone();
    let plan = next
        .plans
        .iter_mut()
        .find(|plan| plan.id == plan_id)
        .ok_or_else(|| "未找到指定计划".to_string())?;
    let changed = change(plan)?;
    if changed {
        commit_config(cfg, next)?;
    }
    Ok(changed)
}

/// 将某个未来日期已经打卡的任务划归今天，并按整日/部分完成规则更新后续排期。
pub fn advance_completed_study_tasks(
    cfg: &mut AppConfig,
    plan_id: &str,
    future_date: &str,
    today: &str,
) -> Result<usize, String> {
    let mut next = cfg.clone();
    let plan = next
        .plans
        .iter_mut()
        .find(|plan| plan.id == plan_id)
        .ok_or_else(|| "未找到指定计划".to_string())?;
    let moved = study::advance_completed_tasks(plan, future_date, today)?;
    if moved > 0 {
        commit_config(cfg, next)?;
    }
    Ok(moved)
}

/// 追加并持久化某日期的一条学习备注。
pub fn add_daily_note(
    cfg: &mut AppConfig,
    date_str: &str,
    note: &str,
) -> Result<DailyNote, String> {
    let mut next = cfg.clone();
    let item = study::add_daily_note(&mut next.daily_notes, date_str, note)?;
    commit_config(cfg, next)?;
    Ok(item)
}

/// 删除一条学习备注并保留同步删除标记。
pub fn delete_daily_note(
    cfg: &mut AppConfig,
    date_str: &str,
    note_id: &str,
) -> Result<bool, String> {
    let mut next = cfg.clone();
    let deleted = study::delete_daily_note(&mut next.daily_notes, date_str, note_id);
    if deleted {
        commit_config(cfg, next)?;
    }
    Ok(deleted)
}

/// 读取某日期可见的全部学习备注。
pub fn get_daily_notes<'a>(cfg: &'a AppConfig, date_str: &str) -> Vec<&'a DailyNote> {
    study::get_daily_notes(&cfg.daily_notes, date_str)
}

#[cfg(test)]
mod tests;
