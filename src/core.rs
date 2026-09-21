//! 业务编排层（无 GUI 依赖）：状态数据、获取/生成/导出入口与凭证持久化。
//!
//! 数据来源接入 Bilibili 与 Jellyfin 两条适配器（`api`/`parse` 与
//! `jellyfin`），统一在 [`fetch_and_parse`] 按 [`FetchSource`] 分派；核心
//! 计划算法在 `plan`/`export` 模块，本模块只做纯函数编排，供任意 UI 层
//! （gpui-component）驱动。

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension, Transaction};

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

fn default_sync_server_url() -> String {
    "https://plan.pcsensor.cloud".to_string()
}

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
    #[serde(default = "default_sync_server_url")]
    pub sync_server_url: String,
    /// 云端设备令牌，由服务端注册接口签发后持久化在本机，作为所有云端请求的
    /// `Authorization: Bearer` 凭据。
    #[serde(default)]
    pub sync_device_token: Option<String>,
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
            sync_server_url: default_sync_server_url(),
            sync_device_token: None,
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
        SourceMode::Bilibili => "bilibili",
        SourceMode::Jellyfin => "jellyfin",
        SourceMode::FnOs => "fnos",
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

// ---------------------------------------------------------------------------
// 本地 SQLite 持久化
// ---------------------------------------------------------------------------

/// 本机数据库路径：用户家目录下 `.bili-planner.sqlite3`。
/// 旧版 JSON 路径仅用于一次性迁移，迁移后不再作为运行时数据源。
fn config_db_path() -> Option<PathBuf> {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let home = std::env::var(key).ok()?;
    Some(PathBuf::from(home).join(".bili-planner.sqlite3"))
}

fn legacy_config_path() -> Option<PathBuf> {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let home = std::env::var(key).ok()?;
    Some(PathBuf::from(home).join(".bili-planner.json"))
}

/// 本地数据库表。配置标量、历史、计划、备注分表保存，计划和备注保留其
/// serde JSON 数据契约，避免 UI 领域模型与存储模式耦合。
struct LocalConfigStore;

impl LocalConfigStore {
    fn open(path: &Path) -> rusqlite::Result<Connection> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(3))?;
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            CREATE TABLE IF NOT EXISTS app_meta (
                id INTEGER PRIMARY KEY CHECK(id = 1),
                payload_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS history (
                position INTEGER PRIMARY KEY,
                input TEXT NOT NULL,
                source TEXT NOT NULL,
                title TEXT NOT NULL,
                at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS plans (
                plan_id TEXT PRIMARY KEY,
                position INTEGER NOT NULL,
                payload_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS daily_notes (
                date TEXT NOT NULL,
                note_id TEXT NOT NULL,
                position INTEGER NOT NULL,
                payload_json TEXT NOT NULL,
                PRIMARY KEY(date, note_id)
            );
            CREATE INDEX IF NOT EXISTS idx_daily_notes_date_position
                ON daily_notes(date, position);
            ",
        )?;
        Ok(conn)
    }

    fn metadata(cfg: &AppConfig) -> AppConfig {
        let mut metadata = cfg.clone();
        metadata.history.clear();
        metadata.plans.clear();
        metadata.daily_notes.clear();
        metadata
    }

    fn write_history(tx: &Transaction<'_>, history: &[HistoryEntry]) -> rusqlite::Result<()> {
        tx.execute("DELETE FROM history", [])?;
        for (position, entry) in history.iter().enumerate() {
            tx.execute(
                "INSERT INTO history(position, input, source, title, at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![position as i64, entry.input, entry.source, entry.title, entry.at],
            )?;
        }
        Ok(())
    }

    fn write_plans(tx: &Transaction<'_>, plans: &[StudyPlan]) -> rusqlite::Result<()> {
        tx.execute("DELETE FROM plans", [])?;
        for (position, plan) in plans.iter().enumerate() {
            let payload = serde_json::to_string(plan).unwrap_or_else(|_| "{}".to_string());
            tx.execute(
                "INSERT INTO plans(plan_id, position, payload_json) VALUES (?1, ?2, ?3)",
                params![plan.id, position as i64, payload],
            )?;
        }
        Ok(())
    }

    fn write_notes(tx: &Transaction<'_>, notes: &DailyNotes) -> rusqlite::Result<()> {
        tx.execute("DELETE FROM daily_notes", [])?;
        for (date, entries) in notes {
            for (position, note) in entries.iter().enumerate() {
                let payload = serde_json::to_string(note).unwrap_or_else(|_| "{}".to_string());
                tx.execute(
                    "INSERT INTO daily_notes(date, note_id, position, payload_json) VALUES (?1, ?2, ?3, ?4)",
                    params![date, note.id, position as i64, payload],
                )?;
            }
        }
        Ok(())
    }

    fn save(path: &Path, cfg: &AppConfig) -> rusqlite::Result<()> {
        let mut conn = Self::open(path)?;
        let tx = conn.transaction()?;
        let metadata =
            serde_json::to_string(&Self::metadata(cfg)).unwrap_or_else(|_| "{}".to_string());
        tx.execute(
            "INSERT INTO app_meta(id, payload_json) VALUES (1, ?1)
             ON CONFLICT(id) DO UPDATE SET payload_json = excluded.payload_json",
            [metadata],
        )?;
        Self::write_history(&tx, &cfg.history)?;
        Self::write_plans(&tx, &cfg.plans)?;
        Self::write_notes(&tx, &cfg.daily_notes)?;
        tx.commit()
    }

    fn load(path: &Path) -> Option<AppConfig> {
        let conn = Self::open(path).ok()?;
        let metadata: String = conn
            .query_row(
                "SELECT payload_json FROM app_meta WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .ok()??;
        let mut cfg: AppConfig = serde_json::from_str(&metadata).ok()?;

        let mut history = Vec::new();
        let mut history_statement = conn
            .prepare("SELECT input, source, title, at FROM history ORDER BY position ASC")
            .ok()?;
        let rows = history_statement
            .query_map([], |row| {
                Ok(HistoryEntry {
                    input: row.get(0)?,
                    source: row.get(1)?,
                    title: row.get(2)?,
                    at: row.get(3)?,
                })
            })
            .ok()?;
        history.extend(rows.filter_map(Result::ok));
        cfg.history = history;

        let mut plans = Vec::new();
        let mut plans_statement = conn
            .prepare("SELECT payload_json FROM plans ORDER BY position ASC")
            .ok()?;
        let rows = plans_statement
            .query_map([], |row| row.get::<_, String>(0))
            .ok()?;
        plans.extend(
            rows.filter_map(Result::ok)
                .filter_map(|payload| serde_json::from_str(&payload).ok()),
        );
        cfg.plans = plans;

        let mut notes = DailyNotes::new();
        let mut notes_statement = conn
            .prepare("SELECT date, payload_json FROM daily_notes ORDER BY date ASC, position ASC")
            .ok()?;
        let rows = notes_statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .ok()?;
        for row in rows.filter_map(Result::ok) {
            if let Ok(note) = serde_json::from_str::<DailyNote>(&row.1) {
                notes.entry(row.0).or_default().push(note);
            }
        }
        cfg.daily_notes = notes;
        Some(cfg)
    }
}

/// 加载给定路径的 SQLite；数据库不存在或尚未初始化时导入旧 JSON。
fn load_config_at(db_path: &Path, legacy_path: &Path) -> Option<AppConfig> {
    if db_path.exists() {
        if let Some(config) = LocalConfigStore::load(db_path) {
            return Some(config);
        }
    }

    let legacy_data = std::fs::read_to_string(legacy_path).ok()?;
    let config: AppConfig = serde_json::from_str(&legacy_data).ok()?;
    LocalConfigStore::save(db_path, &config).ok()?;
    Some(config)
}

/// 启动时加载 SQLite。本地尚未迁移时，导入旧 JSON 并保留原文件为备份。
pub fn load_config() -> Option<AppConfig> {
    let db_path = config_db_path()?;
    let legacy_path = legacy_config_path()?;
    load_config_at(&db_path, &legacy_path)
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
    );

    // 插入或更新
    cfg.plans.retain(|p| p.id != study_plan.id);
    cfg.plans.insert(0, study_plan.clone());
    save_config(cfg);
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
    cfg.plans.insert(0, plan.clone());
    save_config(cfg);
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
    cfg.plans.insert(0, plan.clone());
    save_config(cfg);
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
    cfg.plans.insert(0, plan.clone());
    save_config(cfg);
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
    let plan = cfg
        .plans
        .iter_mut()
        .find(|plan| plan.id == plan_id)
        .ok_or_else(|| "未找到指定的系列计划。".to_string())?;
    study::append_calendar_series_task(plan, task_title, date, minutes)?;
    save_config(cfg);
    Ok(())
}

/// 手动调整任意来源任务的日期，并沿用本地持久化流程。
pub fn move_study_task_to_date(
    cfg: &mut AppConfig,
    plan_id: &str,
    task_id: &str,
    date: &str,
) -> Result<(), String> {
    let plan = cfg
        .plans
        .iter_mut()
        .find(|plan| plan.id == plan_id)
        .ok_or_else(|| "未找到指定任务所属计划。".to_string())?;
    study::move_task_to_date(plan, task_id, date)?;
    save_config(cfg);
    Ok(())
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
    let plan = cfg
        .plans
        .iter_mut()
        .find(|plan| plan.id == plan_id)
        .ok_or_else(|| "未找到指定任务所属计划。".to_string())?;
    study::update_calendar_task(plan, task_id, task_title, date, minutes)?;
    save_config(cfg);
    Ok(())
}

/// 删除右键日历创建的任务；删除系列的最后一项时会同时移除空系列计划。
pub fn delete_calendar_task(
    cfg: &mut AppConfig,
    plan_id: &str,
    task_id: &str,
) -> Result<(), String> {
    let plan_index = cfg
        .plans
        .iter()
        .position(|plan| plan.id == plan_id)
        .ok_or_else(|| "未找到指定任务所属计划。".to_string())?;
    let remove_plan = study::delete_calendar_task(&mut cfg.plans[plan_index], task_id)?;
    if remove_plan {
        cfg.plans.remove(plan_index);
    }
    save_config(cfg);
    Ok(())
}

/// 删除指定计划。
pub fn remove_study_plan(cfg: &mut AppConfig, plan_id: &str) {
    cfg.plans.retain(|p| p.id != plan_id);
    save_config(cfg);
}

/// 切换计划状态（在 Active 和 Paused 之间切换）。
pub fn toggle_study_plan_status(cfg: &mut AppConfig, plan_id: &str) {
    if let Some(plan) = cfg.plans.iter_mut().find(|p| p.id == plan_id) {
        plan.status = match plan.status {
            PlanStatus::Active => PlanStatus::Paused,
            PlanStatus::Paused => PlanStatus::Active,
            other => other,
        };
        save_config(cfg);
    }
}

/// 切换某个任务的打卡状态并持久化。
pub fn checkin_study_task(
    cfg: &mut AppConfig,
    plan_id: &str,
    task_id: &str,
) -> Result<bool, String> {
    let res = study::toggle_task_checkin(&mut cfg.plans, plan_id, task_id)?;
    save_config(cfg);
    Ok(res)
}

/// 一键顺延指定计划并持久化。
pub fn push_forward_study_plan(
    cfg: &mut AppConfig,
    plan_id: &str,
    destination_date: &str,
) -> Result<bool, String> {
    let plan = cfg
        .plans
        .iter_mut()
        .find(|p| p.id == plan_id)
        .ok_or_else(|| "未找到指定计划".to_string())?;
    let changed = study::push_forward_plan(plan, destination_date)?;
    if changed {
        save_config(cfg);
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
    let plan = cfg
        .plans
        .iter_mut()
        .find(|plan| plan.id == plan_id)
        .ok_or_else(|| "未找到指定计划".to_string())?;
    let moved = study::advance_completed_tasks(plan, future_date, today)?;
    if moved > 0 {
        save_config(cfg);
    }
    Ok(moved)
}

/// 云端普通请求的超时；同步载荷更大，单独放宽。
const CLOUD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const CLOUD_SYNC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// 云端接口只用到三种请求形态。
#[derive(Clone, Copy)]
enum CloudRequest<'a> {
    Get,
    PostEmpty,
    PostJson(&'a serde_json::Value),
}

enum CloudFailure {
    /// 服务端查无此设备令牌，需要重新注册。
    UnknownDevice,
    HttpStatus {
        status: u16,
        message: String,
    },
    Other(String),
}

impl CloudFailure {
    fn message(self) -> String {
        match self {
            CloudFailure::UnknownDevice => "云端不认识本机设备，且重新注册失败".to_string(),
            CloudFailure::HttpStatus { status, message } => {
                format!("云端请求失败（HTTP {status}）：{message}")
            }
            CloudFailure::Other(message) => message,
        }
    }

    fn permits_legacy_fallback(&self) -> bool {
        matches!(
            self,
            CloudFailure::HttpStatus {
                status: 400 | 404 | 405 | 415 | 422,
                ..
            }
        )
    }
}

/// 携带 `Authorization: Bearer <device_token>` 发起一次云端请求。
///
/// 设备令牌只走请求头：放在 query string 里会被反向代理访问日志与 TraceLayer 记录下来，
/// 等于把这把不记名密钥广播出去。
fn send_cloud(
    server: &str,
    path: &str,
    request: CloudRequest<'_>,
    token: &str,
    timeout: std::time::Duration,
) -> Result<serde_json::Value, CloudFailure> {
    let url = format!("{server}{path}");
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build()
        .new_agent();
    let authorization = format!("Bearer {token}");
    let sent = match request {
        CloudRequest::Get => agent
            .get(url.as_str())
            .header("Authorization", authorization.as_str())
            .call(),
        CloudRequest::PostEmpty => agent
            .post(url.as_str())
            .header("Authorization", authorization.as_str())
            .send_empty(),
        CloudRequest::PostJson(body) => {
            let payload = serde_json::to_vec(body)
                .map_err(|e| CloudFailure::Other(format!("序列化请求体失败: {}", e)))?;
            agent
                .post(url.as_str())
                .header("Authorization", authorization.as_str())
                .header("Content-Type", "application/json")
                .send(payload)
        }
    };
    let mut resp = sent.map_err(|e| CloudFailure::Other(format!("云端请求失败: {e}")))?;
    let status = resp.status().as_u16();
    let raw = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| CloudFailure::Other(format!("读取云端响应失败: {}", e)))?;
    let data = serde_json::from_str::<serde_json::Value>(&raw);
    if !(200..300).contains(&status) {
        if status == 404
            && data
                .as_ref()
                .ok()
                .and_then(|value| value.get("code"))
                .and_then(|value| value.as_str())
                == Some("unknown_device")
        {
            return Err(CloudFailure::UnknownDevice);
        }
        let message = data
            .as_ref()
            .ok()
            .and_then(|value| value.get("message"))
            .and_then(|value| value.as_str())
            .unwrap_or(raw.as_str())
            .to_string();
        return Err(CloudFailure::HttpStatus { status, message });
    }
    data.map_err(|e| CloudFailure::Other(format!("解析云端响应失败: {e}")))
}

/// 向云端注册本设备。调用方只有在后续目标请求也成功后才替换已有令牌，避免代理或
/// 路由误报 404 时丢失仍然有效的机器人绑定。
///
/// 令牌一律由服务端发牌：客户端自造令牌会让 `/api/sync` 退化成一个匿名可写的注册入口，
/// 任何人都能凭空往云端灌设备记录。
fn register_cloud_device(server: &str) -> Result<String, CloudFailure> {
    let url = format!("{}/api/device/register", server.trim_end_matches('/'));
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(CLOUD_TIMEOUT))
        .http_status_as_error(false)
        .build()
        .new_agent();
    let mut resp = agent
        .post(url.as_str())
        .send_empty()
        .map_err(|e| CloudFailure::Other(format!("注册云端设备失败: {e}")))?;
    let status = resp.status().as_u16();
    let raw = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| CloudFailure::Other(format!("读取注册响应失败: {e}")))?;
    let data: serde_json::Value = serde_json::from_str(&raw).map_err(|e| {
        if (200..300).contains(&status) {
            CloudFailure::Other(format!("解析注册响应失败: {e}"))
        } else {
            CloudFailure::HttpStatus {
                status,
                message: raw.clone(),
            }
        }
    })?;
    if !(200..300).contains(&status) {
        return Err(CloudFailure::HttpStatus {
            status,
            message: data["message"].as_str().unwrap_or(raw.as_str()).to_string(),
        });
    }
    let token = data["device_token"]
        .as_str()
        .filter(|token| !token.trim().is_empty())
        .ok_or_else(|| CloudFailure::Other("云端未返回 device_token".to_string()))?
        .to_string();
    Ok(token)
}

fn persist_cloud_token(cfg: &mut AppConfig, token: String) {
    cfg.sync_device_token = Some(token);
    save_config(cfg);
}

/// 带设备令牌访问云端接口，返回解析后的 JSON。
///
/// 只有服务端回带 `unknown_device` 机器错误码的 404 才说明本机令牌已不被承认。
/// 普通代理/路由 404 不会触发轮换；替换令牌也只在目标请求重试成功后持久化。
fn cloud_request(
    cfg: &mut AppConfig,
    path: &str,
    request: CloudRequest<'_>,
    timeout: std::time::Duration,
) -> Result<serde_json::Value, CloudFailure> {
    let server = cfg.sync_server_url.trim_end_matches('/').to_string();
    let existing_token = cfg
        .sync_device_token
        .clone()
        .filter(|token| !token.trim().is_empty());
    let token = match existing_token.as_ref() {
        Some(token) => token.clone(),
        None => register_cloud_device(&server)?,
    };
    match send_cloud(&server, path, request, &token, timeout) {
        Ok(data) => {
            if existing_token.is_none() {
                persist_cloud_token(cfg, token);
            }
            Ok(data)
        }
        Err(CloudFailure::UnknownDevice) => {
            let replacement = register_cloud_device(&server)?;
            let data = send_cloud(&server, path, request, &replacement, timeout)?;
            persist_cloud_token(cfg, replacement);
            Ok(data)
        }
        Err(failure) => Err(failure),
    }
}

/// 旧服务兼容仅在新版请求明确返回“不支持该协议”的状态码后启用。新服务仍只允许旧版
/// 传输访问数据库中已经存在的设备，不会恢复匿名建号漏洞。
fn legacy_device_token(cfg: &mut AppConfig) -> String {
    if let Some(token) = cfg
        .sync_device_token
        .clone()
        .filter(|token| !token.trim().is_empty())
    {
        return token;
    }
    let token = format!(
        "desktop_{:016x}{:016x}",
        rand::random::<u64>(),
        rand::random::<u64>()
    );
    persist_cloud_token(cfg, token.clone());
    token
}

fn send_legacy_cloud(
    cfg: &mut AppConfig,
    path: &str,
    request: CloudRequest<'_>,
    timeout: std::time::Duration,
) -> Result<serde_json::Value, String> {
    let token = legacy_device_token(cfg);
    let server = cfg.sync_server_url.trim_end_matches('/');
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .build()
        .new_agent();
    let sent = match request {
        CloudRequest::Get => agent
            .get(format!("{server}{path}?device_token={token}"))
            .call(),
        CloudRequest::PostEmpty => {
            let body = serde_json::json!({ "device_token": token });
            let payload = serde_json::to_vec(&body).map_err(|e| e.to_string())?;
            agent
                .post(format!("{server}{path}"))
                .header("Content-Type", "application/json")
                .send(payload)
        }
        CloudRequest::PostJson(body) => {
            let mut legacy_body = body.clone();
            legacy_body["device_token"] = serde_json::Value::String(token);
            let payload = serde_json::to_vec(&legacy_body).map_err(|e| e.to_string())?;
            agent
                .post(format!("{server}{path}"))
                .header("Content-Type", "application/json")
                .send(payload)
        }
    };
    let mut response = sent.map_err(|e| format!("旧版云端请求失败: {e}"))?;
    let raw = response
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("读取旧版云端响应失败: {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("解析旧版云端响应失败: {e}"))
}

fn cloud_request_with_legacy_fallback(
    cfg: &mut AppConfig,
    path: &str,
    request: CloudRequest<'_>,
    timeout: std::time::Duration,
) -> Result<serde_json::Value, String> {
    match cloud_request(cfg, path, request, timeout) {
        Ok(data) => Ok(data),
        Err(failure) if failure.permits_legacy_fallback() => {
            send_legacy_cloud(cfg, path, request, timeout)
        }
        Err(failure) => Err(failure.message()),
    }
}

/// 请求云端 6 位绑定验证码。
pub fn request_cloud_bind_code(cfg: &mut AppConfig) -> Result<(String, u64), String> {
    let data = cloud_request_with_legacy_fallback(
        cfg,
        "/api/bind/request",
        CloudRequest::PostEmpty,
        CLOUD_TIMEOUT,
    )?;
    let code = data["bind_code"]
        .as_str()
        .ok_or_else(|| "缺少 bind_code".to_string())?
        .to_string();
    let expires = data["expires_in_secs"].as_u64().unwrap_or(600);
    Ok((code, expires))
}

/// 查询云端飞书绑定状态。
pub fn check_cloud_bind_status(cfg: &mut AppConfig) -> Result<bool, String> {
    // 尚未注册过设备时不发请求：一次状态查询不该顺带在云端建号。
    if cfg.sync_device_token.is_none() {
        return Ok(false);
    }
    let data = cloud_request_with_legacy_fallback(
        cfg,
        "/api/bind/status",
        CloudRequest::Get,
        CLOUD_TIMEOUT,
    )?;
    let bound = data["bound"].as_bool().unwrap_or(false);
    cfg.feishu_bound = bound;
    cfg.feishu_user_name = data["feishu_user_name"].as_str().map(|s| s.to_string());
    cfg.telegram_bound = data["telegram_bound"].as_bool().unwrap_or(false);
    cfg.telegram_user_name = data["telegram_user_name"].as_str().map(|s| s.to_string());
    save_config(cfg);

    Ok(bound)
}

/// 执行双向增量同步。
pub fn sync_with_cloud(cfg: &mut AppConfig) -> Result<String, String> {
    let body = serde_json::json!({
        "plans": cfg.plans,
        "daily_notes": cfg.daily_notes
    });
    let data = cloud_request_with_legacy_fallback(
        cfg,
        "/api/sync",
        CloudRequest::PostJson(&body),
        CLOUD_SYNC_TIMEOUT,
    )?;

    if let Some(plans_val) = data.get("plans") {
        if let Ok(merged_plans) = serde_json::from_value::<Vec<StudyPlan>>(plans_val.clone()) {
            if cfg.plans.is_empty() {
                cfg.plans = merged_plans;
            } else {
                let mut remote_map: std::collections::HashMap<String, StudyPlan> =
                    std::collections::HashMap::new();
                for rp in merged_plans {
                    remote_map.insert(rp.id.clone(), rp);
                }

                for plan in &mut cfg.plans {
                    if let Some(rp) = remote_map.get(&plan.id) {
                        crate::schedule_recovery::merge_checkins(plan, rp, true);
                    }
                }
            }
        }
    }

    if let Some(notes_val) = data.get("daily_notes") {
        if let Ok(remote_notes) = serde_json::from_value::<DailyNotes>(notes_val.clone()) {
            study::merge_daily_notes(&mut cfg.daily_notes, remote_notes);
        }
    }

    // 保留归位信号给 GUI 合并：后台副本的排期不会直接覆盖正在操作的界面。
    for plan in &mut cfg.plans {
        crate::schedule_recovery::restore(plan, true);
    }

    if let Some(bound) = data.get("feishu_bound").and_then(|b| b.as_bool()) {
        cfg.feishu_bound = bound;
    }
    if let Some(name) = data.get("feishu_user_name").and_then(|n| n.as_str()) {
        cfg.feishu_user_name = Some(name.to_string());
    }
    if let Some(bound) = data.get("telegram_bound").and_then(|b| b.as_bool()) {
        cfg.telegram_bound = bound;
    }
    if let Some(name) = data.get("telegram_user_name").and_then(|n| n.as_str()) {
        cfg.telegram_user_name = Some(name.to_string());
    }

    save_config(cfg);
    Ok("云端同步完成！".to_string())
}

/// 追加并持久化某日期的一条学习备注。
pub fn add_daily_note(
    cfg: &mut AppConfig,
    date_str: &str,
    note: &str,
) -> Result<DailyNote, String> {
    let item = study::add_daily_note(&mut cfg.daily_notes, date_str, note)?;
    save_config(cfg);
    Ok(item)
}

/// 删除一条学习备注并保留同步删除标记。
pub fn delete_daily_note(cfg: &mut AppConfig, date_str: &str, note_id: &str) -> bool {
    let deleted = study::delete_daily_note(&mut cfg.daily_notes, date_str, note_id);
    if deleted {
        save_config(cfg);
    }
    deleted
}

/// 读取某日期可见的全部学习备注。
pub fn get_daily_notes<'a>(cfg: &'a AppConfig, date_str: &str) -> Vec<&'a DailyNote> {
    study::get_daily_notes(&cfg.daily_notes, date_str)
}

/// 把应用配置原子写入本地 SQLite。失败静默：不阻塞主流程。
pub fn save_config(cfg: &AppConfig) {
    let Some(path) = config_db_path() else { return };
    let _ = LocalConfigStore::save(&path, cfg);
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
        assert_eq!(SourceMode::from_index(2), SourceMode::FnOs);
        // 越界索引回退到最后一个来源，不 panic。
        assert_eq!(SourceMode::from_index(9), SourceMode::FnOs);
        assert_eq!(SourceMode::Bilibili.index(), 0);
        assert_eq!(SourceMode::Jellyfin.index(), 1);
        assert_eq!(SourceMode::FnOs.index(), 2);
    }

    #[test]
    fn fnos_credentials_roundtrip_and_legacy_defaults() {
        let cfg = AppConfig {
            fnos_server_url: "http://nas:5666".to_string(),
            fnos_username: "admin".to_string(),
            fnos_password: "secret".to_string(),
            ..Default::default()
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let parsed: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.fnos_server_url, "http://nas:5666");
        assert_eq!(parsed.fnos_username, "admin");
        assert_eq!(parsed.fnos_password, "secret");

        // 旧配置没有这三个键时必须能无损读入。
        let legacy: AppConfig = serde_json::from_str(r#"{"server_url":"http://jf"}"#).unwrap();
        assert!(legacy.fnos_server_url.is_empty());
        assert!(legacy.fnos_username.is_empty());
        assert!(legacy.fnos_password.is_empty());
    }

    #[test]
    fn history_distinguishes_fnos_source_tag() {
        let mut cfg = AppConfig::default();
        record_history(&mut cfg, SourceMode::FnOs, "fv_abc", "飞牛剧集");
        assert_eq!(cfg.history[0].source, "fnos");
        // 同一输入在不同来源下视为两条独立记录。
        record_history(&mut cfg, SourceMode::Jellyfin, "fv_abc", "");
        assert_eq!(cfg.history.len(), 2);
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

    /// 旧版配置文件（只有 Jellyfin 两键）必须能无损读入，且 auto_sync 缺省为 true。
    #[test]
    fn legacy_config_parses_with_empty_history() {
        let cfg: AppConfig =
            serde_json::from_str(r#"{"server_url": "http://jf:8096", "token": "t"}"#).unwrap();
        assert_eq!(cfg.server_url, "http://jf:8096");
        assert!(cfg.history.is_empty());
        assert!(cfg.auto_sync);
    }

    #[test]
    fn auto_sync_config_roundtrip() {
        let cfg_default = AppConfig::default();
        assert!(cfg_default.auto_sync);

        let json = serde_json::to_string(&cfg_default).unwrap();
        let parsed: AppConfig = serde_json::from_str(&json).unwrap();
        assert!(parsed.auto_sync);

        let parsed_false: AppConfig = serde_json::from_str(r#"{"auto_sync": false}"#).unwrap();
        assert!(!parsed_false.auto_sync);
    }

    #[test]
    fn daily_notes_migrate_from_legacy_single_string_format() {
        let cfg: AppConfig =
            serde_json::from_str(r#"{"daily_notes":{"2026-09-03":"旧版单条备注"}}"#).unwrap();
        let notes = get_daily_notes(&cfg, "2026-09-03");
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].content, "旧版单条备注");
    }

    #[test]
    fn local_sqlite_roundtrip_keeps_all_desktop_data() {
        let dir = std::env::temp_dir().join(format!("bili_planner_local_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let db_path = dir.join("config.sqlite3");
        let mut cfg = AppConfig {
            server_url: "https://media.example.com".to_string(),
            token: "jf-token".to_string(),
            ..Default::default()
        };
        record_history(&mut cfg, SourceMode::Bilibili, "BV1test", "测试合集");
        cfg.plans
            .push(study::create_custom_study_plan("背单词", "2026-09-03", 3, 30, false).unwrap());
        let plan_id = cfg.plans[0].id.clone();
        study::checkin_entire_day(&mut cfg.plans, &plan_id, "2026-09-04").unwrap();
        study::advance_completed_tasks(&mut cfg.plans[0], "2026-09-04", "2026-09-03").unwrap();
        assert!(!cfg.plans[0].advance_shifts.is_empty());
        study::add_daily_note(&mut cfg.daily_notes, "2026-09-03", "完成第一组单词").unwrap();

        LocalConfigStore::save(&db_path, &cfg).unwrap();
        let loaded = LocalConfigStore::load(&db_path).unwrap();
        assert_eq!(loaded.server_url, cfg.server_url);
        assert_eq!(loaded.history, cfg.history);
        assert_eq!(loaded.plans, cfg.plans);
        assert_eq!(
            study::get_daily_notes(&loaded.daily_notes, "2026-09-03").len(),
            1
        );
    }

    #[test]
    fn local_json_is_imported_to_sqlite_and_kept_as_backup() {
        let dir =
            std::env::temp_dir().join(format!("bili_planner_migration_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let db_path = dir.join("config.sqlite3");
        let json_path = dir.join("legacy.json");
        let cfg = AppConfig {
            sync_device_token: Some("device-token".to_string()),
            ..Default::default()
        };
        std::fs::write(&json_path, serde_json::to_string(&cfg).unwrap()).unwrap();

        let imported = load_config_at(&db_path, &json_path).unwrap();
        assert_eq!(imported.sync_device_token, cfg.sync_device_token);
        assert!(db_path.exists());
        assert!(json_path.exists());
        assert_eq!(
            LocalConfigStore::load(&db_path).unwrap().sync_device_token,
            cfg.sync_device_token
        );
    }
}
