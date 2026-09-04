//! 进度打卡与多科目学习管理核心逻辑（无 GUI 依赖，纯函数编排）。
//!
//! 提供计划实体、日历排期计算、多科目聚合今日任务、任务打卡与统计、一键顺延等功能。

use chrono::{Datelike, Duration, Local, NaiveDate};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;

use crate::plan::PlanOut;

/// 计划生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PlanStatus {
    /// 进行中（会在今日任务看板中展示）
    #[default]
    Active,
    /// 已暂停
    Paused,
    /// 已全部学完
    Completed,
    /// 已放弃/归档
    Archived,
}

impl PlanStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Active => "进行中",
            Self::Paused => "已暂停",
            Self::Completed => "已完成",
            Self::Archived => "已归档",
        }
    }
}

/// 单个学习任务条目（对应计划中一天的某个视频或切片）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskItem {
    /// 唯一 ID，如 "{plan_id}_{day_idx}_{item_idx}"
    pub id: String,
    /// 原始视频序号
    pub vid_no: i64,
    /// 视频/切片标题
    pub title: String,
    /// 当日需学习时长（秒）
    pub portion: i64,
    /// 是否接上一日
    pub from_prev: bool,
    /// 剩余顺延时长（秒）
    pub remainder: i64,
    /// 是否已完成打卡
    pub completed: bool,
    /// 打卡完成的时间戳（Unix 秒）
    pub completed_at: Option<i64>,
    /// 状态最后更新时间戳（Unix 秒），用于多端冲突解决 (Last-Write-Wins)
    #[serde(default)]
    pub updated_at: i64,
    /// 任务通过“一键提前”划归今天前所在的日期；取消打卡时用于自动归位。
    #[serde(default)]
    pub advanced_from_date: Option<String>,
}

/// 某一天的学习排期。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DailySchedule {
    /// 计划内的第几天序号（0-indexed）
    pub day_index: usize,
    /// 对应日历日期："YYYY-MM-DD"
    pub date: String,
    /// 当日学习任务列表
    pub tasks: Vec<TaskItem>,
    /// 是否为设定的休息日
    pub is_rest_day: bool,
}

/// 持久化的科目学习计划实体。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StudyPlan {
    /// 唯一标识 ID
    pub id: String,
    /// 科目/合集标题
    pub title: String,
    /// 来源类型："bilibili" 或 "jellyfin"
    pub source_type: String,
    /// 原始链接/BV号/ItemID
    pub source_url: String,
    /// 范围描述（如 "整个合集" 或 "科目1"）
    pub scope_desc: String,
    /// 总时长（秒）
    pub total_duration: i64,
    /// 规划总学习天数（不含纯休息日）
    pub planned_days: usize,
    /// 起始学习日期："YYYY-MM-DD"
    pub start_date: String,
    /// 预计结束日期："YYYY-MM-DD"
    pub end_date: String,
    /// 是否跳过周末（周六、周日不排任务）
    pub skip_weekends: bool,
    /// 计划状态
    pub status: PlanStatus,
    /// 创建时间戳（Unix 秒）
    pub created_at: i64,
    /// 每日日程表
    pub schedules: Vec<DailySchedule>,
    /// 是否为通过日历创建、可持续追加每日任务的系列计划。
    #[serde(default)]
    pub is_series: bool,
    /// 是否在“我的计划库”展示。一次性日历任务仍参与日历和打卡，但不占用计划库。
    #[serde(default = "default_show_in_library")]
    pub show_in_library: bool,
}

fn default_show_in_library() -> bool {
    true
}

/// 某天的一条学习备注。保留删除标记可让多端同步正确传播删除操作。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DailyNote {
    pub id: String,
    pub content: String,
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
    #[serde(default)]
    pub deleted: bool,
}

/// 日期 -> 多条备注。值内保留 tombstone，调用展示函数时会过滤已删除的条目。
pub type DailyNotes = HashMap<String, Vec<DailyNote>>;

/// 兼容旧版 `{"YYYY-MM-DD": "一条备注"}` 配置文件。
pub fn deserialize_daily_notes<'de, D>(deserializer: D) -> Result<DailyNotes, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum RawNotes {
        Modern(DailyNotes),
        Legacy(HashMap<String, String>),
    }

    match RawNotes::deserialize(deserializer)? {
        RawNotes::Modern(notes) => Ok(notes),
        RawNotes::Legacy(notes) => Ok(notes
            .into_iter()
            .filter_map(|(date, content)| {
                let content = content.trim().to_string();
                (!content.is_empty()).then(|| {
                    let id = format!("legacy_note_{date}");
                    (
                        date,
                        vec![DailyNote {
                            id,
                            content,
                            created_at: 0,
                            updated_at: 0,
                            deleted: false,
                        }],
                    )
                })
            })
            .collect()),
    }
}

/// 今日聚合任务视图项（用于今日打卡面板展示）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodayTaskView {
    pub plan_id: String,
    pub plan_title: String,
    pub source_type: String,
    pub source_url: String,
    pub day_display: String, // 如 "第 3 天"
    pub task: TaskItem,
}

/// 学习统计信息。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StudyStats {
    /// 活跃计划数
    pub active_plans: usize,
    /// 今日总任务数
    pub today_total_tasks: usize,
    /// 今日已完成任务数
    pub today_completed_tasks: usize,
    /// 今日总时长（秒）
    pub today_total_duration: i64,
    /// 今日已完成时长（秒）
    pub today_completed_duration: i64,
    /// 累计打卡天数
    pub total_days_checked_in: usize,
    /// 当前连续打卡天数 (Streak)
    pub current_streak: usize,
}

// ---------------------------------------------------------------------------
// 纯函数算法
// ---------------------------------------------------------------------------

/// 获取今天日期的标准字符串 "YYYY-MM-DD"。
pub fn today_date_str() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

/// 解析 "YYYY-MM-DD" 字符串为 NaiveDate，失败返回今天。
pub fn parse_date_or_today(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap_or_else(|_| Local::now().date_naive())
}

/// 格式化 NaiveDate 为 "YYYY-MM-DD"。
pub fn format_date(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

/// 判断某一天是否为休息日。
fn is_weekend(d: NaiveDate) -> bool {
    let weekday = d.weekday();
    weekday == chrono::Weekday::Sat || weekday == chrono::Weekday::Sun
}

/// 建立新学习计划并生成每日日历日程。
pub fn create_study_plan(
    title: &str,
    source_type: &str,
    source_url: &str,
    scope_desc: &str,
    plan_out: &PlanOut,
    start_date_str: &str,
    skip_weekends: bool,
) -> StudyPlan {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let plan_id = format!("plan_{}_{}", now, fast_rand_suffix());
    let start_date = parse_date_or_today(start_date_str);

    let mut schedules: Vec<DailySchedule> = Vec::new();
    let mut cur_date = start_date;
    for (plan_day_idx, entries) in plan_out.plan.iter().enumerate() {
        // 若开启跳过周末，且当前日期是周末，则插入休息日日程
        if skip_weekends {
            while is_weekend(cur_date) {
                schedules.push(DailySchedule {
                    day_index: plan_day_idx,
                    date: format_date(cur_date),
                    tasks: Vec::new(),
                    is_rest_day: true,
                });
                cur_date += Duration::days(1);
            }
        }

        let tasks: Vec<TaskItem> = entries
            .iter()
            .enumerate()
            .map(|(item_idx, entry)| TaskItem {
                id: format!("{}_{}_{}", plan_id, plan_day_idx, item_idx),
                vid_no: entry.vid_no,
                title: entry.title.clone(),
                portion: entry.portion,
                from_prev: entry.from_prev,
                remainder: entry.remainder,
                completed: false,
                completed_at: None,
                updated_at: 0,
                advanced_from_date: None,
            })
            .collect();

        schedules.push(DailySchedule {
            day_index: plan_day_idx,
            date: format_date(cur_date),
            tasks,
            is_rest_day: false,
        });

        cur_date += Duration::days(1);
    }

    let end_date = schedules
        .iter()
        .rfind(|s| !s.is_rest_day)
        .map(|s| s.date.clone())
        .unwrap_or_else(|| format_date(start_date));

    StudyPlan {
        id: plan_id,
        title: title.trim().to_string(),
        source_type: source_type.to_string(),
        source_url: source_url.trim().to_string(),
        scope_desc: scope_desc.to_string(),
        total_duration: plan_out.total,
        planned_days: plan_out.plan.len(),
        start_date: format_date(start_date),
        end_date,
        skip_weekends,
        status: PlanStatus::Active,
        created_at: now,
        schedules,
        is_series: false,
        show_in_library: true,
    }
}

/// 产生一个简短的随机后缀。
fn fast_rand_suffix() -> u32 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_i64(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0),
    );
    (hasher.finish() & 0xFFFF) as u32
}

fn now_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 建立一个不依赖视频来源的自定义学习任务。
///
/// 每个学习日创建一项固定时长的任务，因此它会自然出现在已有的今日看板、
/// 月历、统计、机器人和顺延流程中。
pub fn create_custom_study_plan(
    title: &str,
    start_date_str: &str,
    days: i64,
    daily_minutes: i64,
    skip_weekends: bool,
) -> Result<StudyPlan, String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("请填写自定义任务名称。".to_string());
    }
    if days <= 0 {
        return Err("自定义任务天数必须是正整数。".to_string());
    }
    if daily_minutes <= 0 {
        return Err("每日时长必须是正整数分钟。".to_string());
    }

    let now = now_timestamp();
    let plan_id = format!("custom_{}_{}", now, fast_rand_suffix());
    let start_date = parse_date_or_today(start_date_str);
    let portion = daily_minutes
        .checked_mul(60)
        .ok_or_else(|| "每日时长过大。".to_string())?;

    let mut schedules = Vec::new();
    let mut cur_date = start_date;
    let mut day_index = 0usize;
    while day_index < days as usize {
        if skip_weekends {
            while is_weekend(cur_date) {
                schedules.push(DailySchedule {
                    day_index,
                    date: format_date(cur_date),
                    tasks: Vec::new(),
                    is_rest_day: true,
                });
                cur_date += Duration::days(1);
            }
        }

        schedules.push(DailySchedule {
            day_index,
            date: format_date(cur_date),
            tasks: vec![TaskItem {
                id: format!("{}_{}_0", plan_id, day_index),
                vid_no: day_index as i64 + 1,
                title: title.to_string(),
                portion,
                from_prev: false,
                remainder: 0,
                completed: false,
                completed_at: None,
                updated_at: 0,
                advanced_from_date: None,
            }],
            is_rest_day: false,
        });
        day_index += 1;
        cur_date += Duration::days(1);
    }

    let end_date = schedules
        .iter()
        .rfind(|schedule| !schedule.is_rest_day)
        .map(|schedule| schedule.date.clone())
        .unwrap_or_else(|| format_date(start_date));

    Ok(StudyPlan {
        id: plan_id,
        title: title.to_string(),
        source_type: "custom".to_string(),
        source_url: String::new(),
        scope_desc: format!("自定义任务 · 每日 {daily_minutes} 分钟"),
        total_duration: portion * days,
        planned_days: days as usize,
        start_date: format_date(start_date),
        end_date,
        skip_weekends,
        status: PlanStatus::Active,
        created_at: now,
        schedules,
        is_series: false,
        show_in_library: true,
    })
}

fn calendar_task_item(
    plan_id: &str,
    day_index: usize,
    item_index: usize,
    vid_no: i64,
    title: &str,
    portion: i64,
) -> TaskItem {
    TaskItem {
        id: format!("{plan_id}_{day_index}_{item_index}"),
        vid_no,
        title: title.to_string(),
        portion,
        from_prev: false,
        remainder: 0,
        completed: false,
        completed_at: None,
        updated_at: 0,
        advanced_from_date: None,
    }
}

fn validate_calendar_task(title: &str, minutes: i64) -> Result<(String, i64), String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("请填写当天的学习任务名称。".to_string());
    }
    if minutes <= 0 {
        return Err("任务时长必须是正整数分钟。".to_string());
    }
    let portion = minutes
        .checked_mul(60)
        .ok_or_else(|| "任务时长过大。".to_string())?;
    Ok((title.to_string(), portion))
}

fn refresh_calendar_series_summary(plan: &mut StudyPlan) {
    let task_schedules: Vec<_> = plan
        .schedules
        .iter()
        .filter(|schedule| !schedule.is_rest_day && !schedule.tasks.is_empty())
        .collect();
    plan.planned_days = task_schedules.len();
    plan.total_duration = task_schedules
        .iter()
        .flat_map(|schedule| schedule.tasks.iter())
        .map(|task| task.portion)
        .sum();
    if let Some(first) = task_schedules.iter().map(|schedule| &schedule.date).min() {
        plan.start_date = first.clone();
    }
    if let Some(last) = task_schedules.iter().map(|schedule| &schedule.date).max() {
        plan.end_date = last.clone();
    }
}

/// 从日历创建一个一次性任务。它会参与当日打卡和统计，但不显示在计划库中。
pub fn create_one_off_calendar_task(
    task_title: &str,
    date_str: &str,
    minutes: i64,
) -> Result<StudyPlan, String> {
    let (task_title, portion) = validate_calendar_task(task_title, minutes)?;
    let date = format_date(parse_date_or_today(date_str));
    let now = now_timestamp();
    let plan_id = format!("calendar_{}_{}", now, fast_rand_suffix());
    Ok(StudyPlan {
        id: plan_id.clone(),
        title: task_title.clone(),
        source_type: "calendar".to_string(),
        source_url: String::new(),
        scope_desc: "日历单日任务".to_string(),
        total_duration: portion,
        planned_days: 1,
        start_date: date.clone(),
        end_date: date.clone(),
        skip_weekends: false,
        status: PlanStatus::Active,
        created_at: now,
        schedules: vec![DailySchedule {
            day_index: 0,
            date,
            tasks: vec![calendar_task_item(&plan_id, 0, 0, 1, &task_title, portion)],
            is_rest_day: false,
        }],
        is_series: false,
        show_in_library: false,
    })
}

/// 从日历创建一个系列计划，并以当前日期的任务作为首个日程。
pub fn create_calendar_series(
    series_name: &str,
    task_title: &str,
    date_str: &str,
    minutes: i64,
) -> Result<StudyPlan, String> {
    let series_name = series_name.trim();
    if series_name.is_empty() {
        return Err("请填写系列计划名称。".to_string());
    }
    let (task_title, portion) = validate_calendar_task(task_title, minutes)?;
    let date = format_date(parse_date_or_today(date_str));
    let now = now_timestamp();
    let plan_id = format!("series_{}_{}", now, fast_rand_suffix());
    Ok(StudyPlan {
        id: plan_id.clone(),
        title: series_name.to_string(),
        source_type: "calendar".to_string(),
        source_url: String::new(),
        scope_desc: "日历系列计划（按实际添加日期自动延展）".to_string(),
        total_duration: portion,
        planned_days: 1,
        start_date: date.clone(),
        end_date: date.clone(),
        skip_weekends: false,
        status: PlanStatus::Active,
        created_at: now,
        schedules: vec![DailySchedule {
            day_index: 0,
            date,
            tasks: vec![calendar_task_item(&plan_id, 0, 0, 1, &task_title, portion)],
            is_rest_day: false,
        }],
        is_series: true,
        show_in_library: true,
    })
}

/// 向已有日历系列追加某一天的任务。日期可任意指定；计划起止时间会自动覆盖实际范围。
pub fn append_calendar_series_task(
    plan: &mut StudyPlan,
    task_title: &str,
    date_str: &str,
    minutes: i64,
) -> Result<(), String> {
    if !plan.is_series {
        return Err("只能向日历系列计划追加任务。".to_string());
    }
    let (task_title, portion) = validate_calendar_task(task_title, minutes)?;
    let date = format_date(parse_date_or_today(date_str));
    let vid_no = plan
        .schedules
        .iter()
        .flat_map(|schedule| schedule.tasks.iter())
        .count() as i64
        + 1;

    if let Some(schedule) = plan
        .schedules
        .iter_mut()
        .find(|schedule| schedule.date == date && !schedule.is_rest_day)
    {
        let item_index = schedule.tasks.len();
        schedule.tasks.push(calendar_task_item(
            &plan.id,
            schedule.day_index,
            item_index,
            vid_no,
            &task_title,
            portion,
        ));
    } else {
        let day_index = plan
            .schedules
            .iter()
            .map(|schedule| schedule.day_index)
            .max()
            .map_or(0, |index| index + 1);
        plan.schedules.push(DailySchedule {
            day_index,
            date,
            tasks: vec![calendar_task_item(
                &plan.id,
                day_index,
                0,
                vid_no,
                &task_title,
                portion,
            )],
            is_rest_day: false,
        });
    }
    if plan.status == PlanStatus::Completed {
        plan.status = PlanStatus::Active;
    }
    refresh_calendar_series_summary(plan);
    Ok(())
}

/// 编辑日历创建的任务。可修改名称、时长与日期，编辑后自动刷新计划汇总日期。
pub fn update_calendar_task(
    plan: &mut StudyPlan,
    task_id: &str,
    task_title: &str,
    date_str: &str,
    minutes: i64,
) -> Result<(), String> {
    if plan.source_type != "calendar" {
        return Err("只能编辑通过日历创建的任务。".to_string());
    }
    let (task_title, portion) = validate_calendar_task(task_title, minutes)?;
    let one_off_title = task_title.clone();
    let target_date = format_date(parse_date_or_today(date_str));
    let source_index = plan
        .schedules
        .iter()
        .position(|schedule| schedule.tasks.iter().any(|task| task.id == task_id))
        .ok_or_else(|| "未找到指定任务。".to_string())?;
    let task_index = plan.schedules[source_index]
        .tasks
        .iter()
        .position(|task| task.id == task_id)
        .ok_or_else(|| "未找到指定任务。".to_string())?;
    let source_date = plan.schedules[source_index].date.clone();

    if source_date == target_date {
        let task = &mut plan.schedules[source_index].tasks[task_index];
        task.title = task_title;
        task.portion = portion;
        task.updated_at = now_timestamp();
        task.advanced_from_date = None;
    } else {
        let mut task = plan.schedules[source_index].tasks.remove(task_index);
        task.title = task_title;
        task.portion = portion;
        task.updated_at = now_timestamp();
        task.advanced_from_date = None;
        if plan.schedules[source_index].tasks.is_empty() {
            plan.schedules.remove(source_index);
        }

        if let Some(schedule) = plan
            .schedules
            .iter_mut()
            .find(|schedule| schedule.date == target_date && !schedule.is_rest_day)
        {
            schedule.tasks.push(task);
        } else {
            let day_index = plan
                .schedules
                .iter()
                .map(|schedule| schedule.day_index)
                .max()
                .map_or(0, |index| index + 1);
            plan.schedules.push(DailySchedule {
                day_index,
                date: target_date,
                tasks: vec![task],
                is_rest_day: false,
            });
        }
    }
    if !plan.is_series {
        plan.title = one_off_title;
    }
    refresh_calendar_series_summary(plan);
    Ok(())
}

/// 删除日历创建的一项任务。返回 `true` 表示计划已经没有任务，调用方应将计划移除。
pub fn delete_calendar_task(plan: &mut StudyPlan, task_id: &str) -> Result<bool, String> {
    if plan.source_type != "calendar" {
        return Err("只能删除通过日历创建的任务。".to_string());
    }
    let source_index = plan
        .schedules
        .iter()
        .position(|schedule| schedule.tasks.iter().any(|task| task.id == task_id))
        .ok_or_else(|| "未找到指定任务。".to_string())?;
    let task_index = plan.schedules[source_index]
        .tasks
        .iter()
        .position(|task| task.id == task_id)
        .ok_or_else(|| "未找到指定任务。".to_string())?;
    plan.schedules[source_index].tasks.remove(task_index);
    if plan.schedules[source_index].tasks.is_empty() {
        plan.schedules.remove(source_index);
    }
    if plan
        .schedules
        .iter()
        .all(|schedule| schedule.tasks.is_empty())
    {
        return Ok(true);
    }
    refresh_calendar_series_summary(plan);
    Ok(false)
}

/// 追加一条备注（同一天支持多条）。
pub fn add_daily_note(
    notes: &mut DailyNotes,
    date: &str,
    content: &str,
) -> Result<DailyNote, String> {
    let content = content.trim();
    if content.is_empty() {
        return Err("备注内容不能为空。".to_string());
    }
    let now = now_timestamp();
    let note = DailyNote {
        id: format!("note_{}_{}", now, fast_rand_suffix()),
        content: content.to_string(),
        created_at: now,
        updated_at: now,
        deleted: false,
    };
    notes
        .entry(date.to_string())
        .or_default()
        .push(note.clone());
    Ok(note)
}

/// 标记删除一条备注，以便下一次同步将删除同步到其他设备。
pub fn delete_daily_note(notes: &mut DailyNotes, date: &str, note_id: &str) -> bool {
    let Some(items) = notes.get_mut(date) else {
        return false;
    };
    let Some(note) = items
        .iter_mut()
        .find(|note| note.id == note_id && !note.deleted)
    else {
        return false;
    };
    note.deleted = true;
    note.updated_at = now_timestamp();
    true
}

/// 获取指定日期的可见备注，按添加顺序返回。
pub fn get_daily_notes<'a>(notes: &'a DailyNotes, date: &str) -> Vec<&'a DailyNote> {
    notes
        .get(date)
        .into_iter()
        .flatten()
        .filter(|note| !note.deleted)
        .collect()
}

/// 合并远端备注；同一备注 ID 以 `updated_at` 较新的版本为准。
pub fn merge_daily_notes(local: &mut DailyNotes, remote: DailyNotes) {
    for (date, remote_items) in remote {
        let local_items = local.entry(date).or_default();
        for remote_note in remote_items {
            match local_items
                .iter()
                .position(|note| note.id == remote_note.id)
            {
                Some(index) if remote_note.updated_at > local_items[index].updated_at => {
                    local_items[index] = remote_note;
                }
                Some(_) => {}
                None => local_items.push(remote_note),
            }
        }
    }
}

/// 获取指定日期下所有活跃计划的任务（多科目聚合）。
pub fn get_tasks_for_date(plans: &[StudyPlan], target_date: &str) -> Vec<TodayTaskView> {
    let mut list = Vec::new();
    for plan in plans {
        if matches!(plan.status, PlanStatus::Paused | PlanStatus::Archived) {
            continue;
        }
        for schedule in &plan.schedules {
            if schedule.date == target_date && !schedule.is_rest_day {
                for task in &schedule.tasks {
                    list.push(TodayTaskView {
                        plan_id: plan.id.clone(),
                        plan_title: plan.title.clone(),
                        source_type: plan.source_type.clone(),
                        source_url: plan.source_url.clone(),
                        day_display: format!("第 {} 天", schedule.day_index + 1),
                        task: task.clone(),
                    });
                }
            }
        }
    }
    list
}

/// 切换单个任务的打卡完成状态。
pub fn toggle_task_checkin(
    plans: &mut [StudyPlan],
    plan_id: &str,
    task_id: &str,
) -> Result<bool, String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    for plan in plans.iter_mut().filter(|plan| plan.id == plan_id) {
        let location = plan
            .schedules
            .iter()
            .enumerate()
            .find_map(|(schedule_index, schedule)| {
                schedule
                    .tasks
                    .iter()
                    .position(|task| task.id == task_id)
                    .map(|task_index| (schedule_index, task_index))
            });
        let Some((schedule_index, task_index)) = location else {
            continue;
        };

        let restore_date = plan.schedules[schedule_index].tasks[task_index]
            .completed
            .then(|| {
                plan.schedules[schedule_index].tasks[task_index]
                    .advanced_from_date
                    .clone()
            })
            .flatten();
        if let Some(restore_date) = restore_date {
            let mut task = plan.schedules[schedule_index].tasks.remove(task_index);
            task.completed = false;
            task.completed_at = None;
            task.updated_at = now;
            task.advanced_from_date = None;
            if plan.schedules[schedule_index].tasks.is_empty()
                && !plan.schedules[schedule_index].is_rest_day
            {
                plan.schedules.remove(schedule_index);
            }
            if let Some(schedule) = plan
                .schedules
                .iter_mut()
                .find(|schedule| schedule.date == restore_date && !schedule.is_rest_day)
            {
                schedule.tasks.push(task);
                schedule.tasks.sort_by_key(|task| task.vid_no);
            } else {
                plan.schedules.push(DailySchedule {
                    day_index: 0,
                    date: restore_date,
                    tasks: vec![task],
                    is_rest_day: false,
                });
            }
            rebuild_rest_days(plan);
            refresh_plan_schedule_summary(plan);
            check_update_plan_completion(plan);
            return Ok(false);
        }

        let task = &mut plan.schedules[schedule_index].tasks[task_index];
        task.completed = !task.completed;
        task.completed_at = if task.completed { Some(now) } else { None };
        task.updated_at = now;
        let new_state = task.completed;
        check_update_plan_completion(plan);
        return Ok(new_state);
    }
    Err("未找到对应的任务".to_string())
}

/// 应用云端取消打卡后的归位信号。任务若已在原日期，仅清除临时来源标记。
pub fn restore_cancelled_advanced_tasks(plans: &mut [StudyPlan]) {
    for plan in plans {
        let mut changed = false;
        loop {
            let location =
                plan.schedules
                    .iter()
                    .enumerate()
                    .find_map(|(schedule_index, schedule)| {
                        schedule
                            .tasks
                            .iter()
                            .enumerate()
                            .find(|(_, task)| !task.completed && task.advanced_from_date.is_some())
                            .map(|(task_index, task)| {
                                (
                                    schedule_index,
                                    task_index,
                                    schedule.date.clone(),
                                    task.advanced_from_date.clone().unwrap_or_default(),
                                )
                            })
                    });
            let Some((schedule_index, task_index, current_date, restore_date)) = location else {
                break;
            };
            if current_date == restore_date {
                plan.schedules[schedule_index].tasks[task_index].advanced_from_date = None;
                changed = true;
                continue;
            }
            let mut task = plan.schedules[schedule_index].tasks.remove(task_index);
            task.advanced_from_date = None;
            if plan.schedules[schedule_index].tasks.is_empty()
                && !plan.schedules[schedule_index].is_rest_day
            {
                plan.schedules.remove(schedule_index);
            }
            if let Some(schedule) = plan
                .schedules
                .iter_mut()
                .find(|schedule| schedule.date == restore_date && !schedule.is_rest_day)
            {
                schedule.tasks.push(task);
                schedule.tasks.sort_by_key(|task| task.vid_no);
            } else {
                plan.schedules.push(DailySchedule {
                    day_index: 0,
                    date: restore_date,
                    tasks: vec![task],
                    is_rest_day: false,
                });
            }
            changed = true;
        }
        if changed {
            rebuild_rest_days(plan);
            refresh_plan_schedule_summary(plan);
            check_update_plan_completion(plan);
        }
    }
}

/// 标记某个计划在某天的所有任务为已完成。
pub fn checkin_entire_day(
    plans: &mut [StudyPlan],
    plan_id: &str,
    target_date: &str,
) -> Result<(), String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    for plan in plans.iter_mut() {
        if plan.id == plan_id {
            for schedule in plan.schedules.iter_mut() {
                if schedule.date == target_date {
                    for task in schedule.tasks.iter_mut() {
                        task.completed = true;
                        task.completed_at = Some(now);
                        task.updated_at = now;
                    }
                }
            }
            check_update_plan_completion(plan);
            return Ok(());
        }
    }
    Err("未找到对应计划".to_string())
}

/// 计算计划的整体进度：(已完成数, 总任务数, 已完成秒数, 总秒数, 完成比例 0.0~1.0)。
pub fn compute_plan_progress(plan: &StudyPlan) -> (usize, usize, i64, i64, f64) {
    let mut total_tasks = 0;
    let mut completed_tasks = 0;
    let mut completed_duration = 0;
    let mut total_duration = 0;

    for s in &plan.schedules {
        for t in &s.tasks {
            total_tasks += 1;
            total_duration += t.portion;
            if t.completed {
                completed_tasks += 1;
                completed_duration += t.portion;
            }
        }
    }

    let ratio = if total_duration > 0 {
        completed_duration as f64 / total_duration as f64
    } else if total_tasks > 0 {
        completed_tasks as f64 / total_tasks as f64
    } else {
        0.0
    };

    (
        completed_tasks,
        total_tasks,
        completed_duration,
        total_duration,
        ratio,
    )
}

/// 检查并自动更新计划完成状态。
fn check_update_plan_completion(plan: &mut StudyPlan) {
    let (done, total, _, _, _) = compute_plan_progress(plan);
    if total > 0 && done == total {
        if plan.status == PlanStatus::Active {
            plan.status = PlanStatus::Completed;
        }
    } else if plan.status == PlanStatus::Completed {
        plan.status = PlanStatus::Active;
    }
}

fn next_learning_date(mut date: NaiveDate, skip_weekends: bool) -> NaiveDate {
    date += Duration::days(1);
    if skip_weekends {
        while is_weekend(date) {
            date += Duration::days(1);
        }
    }
    date
}

fn previous_learning_date(mut date: NaiveDate, skip_weekends: bool) -> NaiveDate {
    date -= Duration::days(1);
    if skip_weekends {
        while is_weekend(date) {
            date -= Duration::days(1);
        }
    }
    date
}

fn normalize_schedule_indices(plan: &mut StudyPlan) {
    plan.schedules
        .sort_by(|left, right| left.date.cmp(&right.date));
    let mut learning_day = 0usize;
    for schedule in &mut plan.schedules {
        schedule.day_index = learning_day;
        if !schedule.is_rest_day {
            learning_day += 1;
        }
    }
}

fn rebuild_rest_days(plan: &mut StudyPlan) {
    if !plan.skip_weekends {
        return;
    }
    let Some(first) = plan
        .schedules
        .iter()
        .filter(|schedule| !schedule.is_rest_day && !schedule.tasks.is_empty())
        .map(|schedule| parse_date_or_today(&schedule.date))
        .min()
    else {
        return;
    };
    let Some(last) = plan
        .schedules
        .iter()
        .filter(|schedule| !schedule.is_rest_day && !schedule.tasks.is_empty())
        .map(|schedule| parse_date_or_today(&schedule.date))
        .max()
    else {
        return;
    };
    plan.schedules.retain(|schedule| !schedule.is_rest_day);
    let mut date = first;
    while date <= last {
        if is_weekend(date) {
            let date_text = format_date(date);
            if !plan
                .schedules
                .iter()
                .any(|schedule| schedule.date == date_text)
            {
                plan.schedules.push(DailySchedule {
                    day_index: 0,
                    date: date_text,
                    tasks: Vec::new(),
                    is_rest_day: true,
                });
            }
        }
        date += Duration::days(1);
    }
}

fn refresh_plan_schedule_summary(plan: &mut StudyPlan) {
    let task_schedules: Vec<_> = plan
        .schedules
        .iter()
        .filter(|schedule| !schedule.is_rest_day && !schedule.tasks.is_empty())
        .collect();
    plan.planned_days = task_schedules.len();
    plan.total_duration = task_schedules
        .iter()
        .flat_map(|schedule| schedule.tasks.iter())
        .map(|task| task.portion)
        .sum();
    if let Some(first) = task_schedules.iter().map(|schedule| &schedule.date).min() {
        plan.start_date = first.clone();
    }
    if let Some(last) = task_schedules.iter().map(|schedule| &schedule.date).max() {
        plan.end_date = last.clone();
    }
    normalize_schedule_indices(plan);
}

/// 在新的学习日开始时，顺延上一个学习日未完成的整个任务条目。
///
/// 已完成条目保留在原日期；未完成条目进入新日期；同一计划原本的所有
/// 后续日程整体后移一个学习日，因此新日期只新增该计划上一日的剩余条目。
/// 其他计划不会被修改。返回值表示是否实际发生顺延。
pub fn push_forward_plan(plan: &mut StudyPlan, destination_date_str: &str) -> Result<bool, String> {
    let destination_date = parse_date_or_today(destination_date_str);
    let target_date = previous_learning_date(destination_date, plan.skip_weekends);
    let target_text = format_date(target_date);
    let target_schedule = plan
        .schedules
        .iter()
        .find(|schedule| schedule.date == target_text && !schedule.is_rest_day)
        .cloned();
    let Some(target_schedule) = target_schedule else {
        return Ok(false);
    };
    let completed: Vec<TaskItem> = target_schedule
        .tasks
        .iter()
        .filter(|task| task.completed)
        .cloned()
        .collect();
    let unfinished: Vec<TaskItem> = target_schedule
        .tasks
        .iter()
        .filter(|task| !task.completed)
        .cloned()
        .collect();
    if unfinished.is_empty() {
        return Ok(false);
    }

    let mut rebuilt: Vec<DailySchedule> = plan
        .schedules
        .iter()
        .filter(|schedule| schedule.date < target_text && !schedule.is_rest_day)
        .cloned()
        .collect();
    if !completed.is_empty() {
        rebuilt.push(DailySchedule {
            day_index: target_schedule.day_index,
            date: target_text.clone(),
            tasks: completed,
            is_rest_day: false,
        });
    }
    rebuilt.push(DailySchedule {
        day_index: 0,
        date: format_date(next_learning_date(target_date, plan.skip_weekends)),
        tasks: unfinished,
        is_rest_day: false,
    });

    let mut future: Vec<DailySchedule> = plan
        .schedules
        .iter()
        .filter(|schedule| {
            schedule.date > target_text && !schedule.is_rest_day && !schedule.tasks.is_empty()
        })
        .cloned()
        .collect();
    future.sort_by(|left, right| left.date.cmp(&right.date));
    for mut schedule in future {
        let original = parse_date_or_today(&schedule.date);
        schedule.date = format_date(next_learning_date(original, plan.skip_weekends));
        rebuilt.push(schedule);
    }

    plan.schedules = rebuilt;
    rebuild_rest_days(plan);
    refresh_plan_schedule_summary(plan);
    if plan.status == PlanStatus::Completed {
        plan.status = PlanStatus::Active;
    }
    Ok(true)
}

/// 将指定未来日期中已打卡的任务条目移动到今天。
///
/// 若该计划在指定未来日期的全部任务均已完成，则移除该日并把更晚的日程
/// 整体提前一个学习日；若只完成部分，则指定日期保留未完成条目，后续日程不动。
pub fn advance_completed_tasks(
    plan: &mut StudyPlan,
    future_date_str: &str,
    today_str: &str,
) -> Result<usize, String> {
    let today = format_date(parse_date_or_today(today_str));
    let future_date = format_date(parse_date_or_today(future_date_str));
    if future_date <= today {
        return Ok(0);
    }
    let Some(source_index) = plan
        .schedules
        .iter()
        .position(|schedule| schedule.date == future_date && !schedule.is_rest_day)
    else {
        return Ok(0);
    };
    let original_tasks = std::mem::take(&mut plan.schedules[source_index].tasks);
    let full_day_completed =
        !original_tasks.is_empty() && original_tasks.iter().all(|task| task.completed);
    let mut moved = Vec::new();
    let mut retained = Vec::new();
    for mut task in original_tasks {
        if task.completed {
            task.advanced_from_date = Some(future_date.clone());
            moved.push(task);
        } else {
            retained.push(task);
        }
    }
    if moved.is_empty() {
        plan.schedules[source_index].tasks = retained;
        return Ok(0);
    }
    let moved_count = moved.len();
    plan.schedules[source_index].tasks = retained;

    if full_day_completed {
        // 该日整批完成：后续每个日程仅向前移动一个学习日。
        for schedule in plan.schedules.iter_mut().filter(|schedule| {
            schedule.date > future_date && !schedule.is_rest_day && !schedule.tasks.is_empty()
        }) {
            let original = parse_date_or_today(&schedule.date);
            schedule.date = format_date(previous_learning_date(original, plan.skip_weekends));
        }
    }

    plan.schedules.retain(|schedule| {
        !schedule.tasks.is_empty() || schedule.is_rest_day || schedule.date == today
    });
    if let Some(today_schedule) = plan
        .schedules
        .iter_mut()
        .find(|schedule| schedule.date == today && !schedule.is_rest_day)
    {
        today_schedule.tasks.extend(moved);
    } else {
        plan.schedules.retain(|schedule| schedule.date != today);
        plan.schedules.push(DailySchedule {
            day_index: 0,
            date: today,
            tasks: moved,
            is_rest_day: false,
        });
    }
    rebuild_rest_days(plan);
    refresh_plan_schedule_summary(plan);
    Ok(moved_count)
}

/// 计算综合学习统计（今日任务、打卡天数与连续打卡 Streak）。
pub fn compute_study_stats(plans: &[StudyPlan], today_str: &str) -> StudyStats {
    let today_tasks = get_tasks_for_date(plans, today_str);
    let today_total = today_tasks.len();
    let today_completed = today_tasks.iter().filter(|v| v.task.completed).count();
    let today_total_dur: i64 = today_tasks.iter().map(|v| v.task.portion).sum();
    let today_completed_dur: i64 = today_tasks
        .iter()
        .filter(|v| v.task.completed)
        .map(|v| v.task.portion)
        .sum();

    // 统计所有有打卡记录的唯一日期
    let mut checkin_dates = std::collections::BTreeSet::new();
    for plan in plans {
        for schedule in &plan.schedules {
            if schedule.tasks.iter().any(|t| t.completed) {
                checkin_dates.insert(schedule.date.clone());
            }
        }
    }
    let total_days_checked_in = checkin_dates.len();

    // 计算连续打卡 Streak（从今天或昨天往前推）
    let today = parse_date_or_today(today_str);
    let mut streak = 0;
    let mut check_date = today;

    let skip_weekends = !plans.is_empty()
        && plans
            .iter()
            .filter(|p| p.status == PlanStatus::Active)
            .all(|p| p.skip_weekends);

    // 如果今天还没打卡，允许从昨天算起（若昨天也是周末且跳过周末，则允许跳过周末追溯）
    if !checkin_dates.contains(&format_date(check_date)) {
        check_date -= Duration::days(1);
        if skip_weekends {
            while is_weekend(check_date) && !checkin_dates.contains(&format_date(check_date)) {
                check_date -= Duration::days(1);
            }
        }
    }

    while checkin_dates.contains(&format_date(check_date)) {
        streak += 1;
        check_date -= Duration::days(1);
        if skip_weekends {
            while is_weekend(check_date) && !checkin_dates.contains(&format_date(check_date)) {
                check_date -= Duration::days(1);
            }
        }
    }

    let active_plans = plans
        .iter()
        .filter(|p| p.status == PlanStatus::Active)
        .count();

    StudyStats {
        active_plans,
        today_total_tasks: today_total,
        today_completed_tasks: today_completed,
        today_total_duration: today_total_dur,
        today_completed_duration: today_completed_dur,
        total_days_checked_in,
        current_streak: streak,
    }
}

/// 单日日历视图聚合模型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonthCalendarDay {
    pub date: String,
    pub day_num: u32,
    pub is_current_month: bool,
    pub is_today: bool,
    pub is_weekend: bool,
    pub is_rest_day: bool,
    pub total_duration: i64,
    pub completed_duration: i64,
    pub total_tasks: usize,
    pub completed_tasks: usize,
    pub plan_titles: Vec<String>,
}

/// 月度学习统计。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MonthStudyStats {
    pub total_duration: i64,
    pub completed_duration: i64,
    pub total_tasks: usize,
    pub completed_tasks: usize,
    pub active_study_days: usize,
}

/// 获取某年某月的第一天和最后一天。
pub fn get_month_range(year: i32, month: u32) -> (NaiveDate, NaiveDate) {
    let first_day =
        NaiveDate::from_ymd_opt(year, month, 1).unwrap_or_else(|| Local::now().date_naive());
    let next_month_first = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    }
    .unwrap_or_else(|| first_day + Duration::days(31));
    let last_day = next_month_first - Duration::days(1);
    (first_day, last_day)
}

/// 生成某月份完整的对齐日历网格（以周一为每周起始，7列，通常 5~6 行，共 35 或 42 格）。
pub fn generate_month_calendar_matrix(
    year: i32,
    month: u32,
    plans: &[StudyPlan],
) -> Vec<MonthCalendarDay> {
    let (first_day, last_day) = get_month_range(year, month);
    let today_str = today_date_str();

    // 确定网格开始日期（前移至当周周一）
    let start_weekday = first_day.weekday().num_days_from_monday(); // 0=Mon, 6=Sun
    let grid_start = first_day - Duration::days(start_weekday as i64);

    // 确定网格结束日期（后移至当周周日）
    let end_weekday = last_day.weekday().num_days_from_monday();
    let days_to_sunday = 6 - end_weekday;
    let grid_end = last_day + Duration::days(days_to_sunday as i64);

    let mut matrix = Vec::new();
    let mut cur = grid_start;

    while cur <= grid_end {
        let date_str = format_date(cur);
        let is_current_month = cur.month() == month;
        let is_today = date_str == today_str;
        let is_wkend = is_weekend(cur);

        let mut total_duration = 0;
        let mut completed_duration = 0;
        let mut total_tasks = 0;
        let mut completed_tasks = 0;
        let mut is_rest_day = false;
        let mut plan_titles = Vec::new();

        for plan in plans {
            if matches!(plan.status, PlanStatus::Paused | PlanStatus::Archived) {
                continue;
            }
            if let Some(sch) = plan.schedules.iter().find(|s| s.date == date_str) {
                if sch.is_rest_day {
                    is_rest_day = true;
                }
                if !sch.tasks.is_empty() {
                    if !plan_titles.contains(&plan.title) {
                        plan_titles.push(plan.title.clone());
                    }
                    for task in &sch.tasks {
                        total_tasks += 1;
                        total_duration += task.portion;
                        if task.completed {
                            completed_tasks += 1;
                            completed_duration += task.portion;
                        }
                    }
                }
            }
        }

        matrix.push(MonthCalendarDay {
            date: date_str,
            day_num: cur.day(),
            is_current_month,
            is_today,
            is_weekend: is_wkend,
            is_rest_day,
            total_duration,
            completed_duration,
            total_tasks,
            completed_tasks,
            plan_titles,
        });

        cur += Duration::days(1);
    }

    matrix
}

/// 计算当月学习汇总统计。
pub fn compute_month_study_stats(year: i32, month: u32, plans: &[StudyPlan]) -> MonthStudyStats {
    let (first_day, last_day) = get_month_range(year, month);
    let mut total_dur = 0;
    let mut done_dur = 0;
    let mut total_t = 0;
    let mut done_t = 0;
    let mut active_days_set = std::collections::HashSet::new();

    let mut cur = first_day;
    while cur <= last_day {
        let date_str = format_date(cur);
        for plan in plans {
            if matches!(plan.status, PlanStatus::Paused | PlanStatus::Archived) {
                continue;
            }
            if let Some(sch) = plan.schedules.iter().find(|s| s.date == date_str) {
                if !sch.tasks.is_empty() {
                    active_days_set.insert(date_str.clone());
                    for t in &sch.tasks {
                        total_t += 1;
                        total_dur += t.portion;
                        if t.completed {
                            done_t += 1;
                            done_dur += t.portion;
                        }
                    }
                }
            }
        }
        cur += Duration::days(1);
    }

    MonthStudyStats {
        total_duration: total_dur,
        completed_duration: done_dur,
        total_tasks: total_t,
        completed_tasks: done_t,
        active_study_days: active_days_set.len(),
    }
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::PlanEntry;

    fn mock_plan_out() -> PlanOut {
        PlanOut {
            plan: vec![
                vec![
                    PlanEntry {
                        vid_no: 1,
                        title: "P1 第一节".to_string(),
                        portion: 600,
                        from_prev: false,
                        remainder: 0,
                        cont_day: None,
                    },
                    PlanEntry {
                        vid_no: 2,
                        title: "P2 第二节".to_string(),
                        portion: 400,
                        from_prev: false,
                        remainder: 200,
                        cont_day: Some(2),
                    },
                ],
                vec![
                    PlanEntry {
                        vid_no: 2,
                        title: "P2 第二节".to_string(),
                        portion: 200,
                        from_prev: true,
                        remainder: 0,
                        cont_day: None,
                    },
                    PlanEntry {
                        vid_no: 3,
                        title: "P3 第三节".to_string(),
                        portion: 800,
                        from_prev: false,
                        remainder: 0,
                        cont_day: None,
                    },
                ],
            ],
            capacities: vec![1000, 1000],
            total: 2000,
        }
    }

    #[test]
    fn create_plan_continuous_days() {
        let plan_out = mock_plan_out();
        let plan = create_study_plan(
            "高数",
            "bilibili",
            "BV123",
            "全集",
            &plan_out,
            "2026-09-01",
            false,
        );

        assert_eq!(plan.title, "高数");
        assert_eq!(plan.planned_days, 2);
        assert_eq!(plan.start_date, "2026-09-01");
        assert_eq!(plan.end_date, "2026-09-02");
        assert_eq!(plan.schedules.len(), 2);
        assert_eq!(plan.schedules[0].tasks.len(), 2);
        assert_eq!(plan.schedules[1].tasks.len(), 2);
    }

    #[test]
    fn custom_plan_uses_existing_daily_schedule_model() {
        let plan = create_custom_study_plan("背单词", "2026-08-28", 2, 30, true).unwrap();
        assert_eq!(plan.source_type, "custom");
        assert_eq!(plan.total_duration, 3_600);
        assert_eq!(plan.schedules.len(), 4); // 周六、周日作为休息日保留
        assert_eq!(plan.schedules[0].date, "2026-08-28");
        assert_eq!(plan.schedules[3].date, "2026-08-31");
        assert_eq!(plan.schedules[3].tasks[0].portion, 1_800);
    }

    #[test]
    fn daily_notes_support_multiple_items_and_tombstones() {
        let mut notes = DailyNotes::new();
        let first = add_daily_note(&mut notes, "2026-09-03", "完成练习").unwrap();
        let _second = add_daily_note(&mut notes, "2026-09-03", "整理错题").unwrap();
        assert_eq!(get_daily_notes(&notes, "2026-09-03").len(), 2);
        assert!(delete_daily_note(&mut notes, "2026-09-03", &first.id));
        assert_eq!(get_daily_notes(&notes, "2026-09-03").len(), 1);
    }

    #[test]
    fn calendar_series_expands_from_manually_added_dates() {
        let mut plan =
            create_calendar_series("英语冲刺", "背 50 个单词", "2026-09-10", 30).unwrap();
        append_calendar_series_task(&mut plan, "完成阅读", "2026-09-08", 45).unwrap();
        append_calendar_series_task(&mut plan, "整理错题", "2026-09-10", 20).unwrap();

        assert!(plan.is_series);
        assert!(plan.show_in_library);
        assert_eq!(plan.start_date, "2026-09-08");
        assert_eq!(plan.end_date, "2026-09-10");
        assert_eq!(plan.planned_days, 2);
        assert_eq!(plan.total_duration, (30 + 45 + 20) * 60);
        let tenth = plan
            .schedules
            .iter()
            .find(|schedule| schedule.date == "2026-09-10")
            .unwrap();
        assert_eq!(tenth.tasks.len(), 2);
    }

    #[test]
    fn one_off_calendar_task_is_not_a_library_plan() {
        let plan = create_one_off_calendar_task("预约体检", "2026-09-12", 20).unwrap();
        assert!(!plan.is_series);
        assert!(!plan.show_in_library);
        assert_eq!(plan.schedules[0].date, "2026-09-12");
    }

    #[test]
    fn calendar_task_can_be_edited_moved_and_deleted() {
        let mut plan = create_calendar_series("英语冲刺", "背单词", "2026-09-10", 30).unwrap();
        let task_id = plan.schedules[0].tasks[0].id.clone();
        update_calendar_task(&mut plan, &task_id, "精读文章", "2026-09-12", 50).unwrap();

        assert_eq!(plan.start_date, "2026-09-12");
        assert_eq!(plan.end_date, "2026-09-12");
        assert_eq!(plan.total_duration, 3_000);
        assert_eq!(plan.schedules[0].tasks[0].title, "精读文章");
        assert!(delete_calendar_task(&mut plan, &task_id).unwrap());
    }

    #[test]
    fn create_plan_skip_weekends() {
        let plan_out = mock_plan_out();
        // 2026-08-28 是周五，下两天是周六、周日
        let plan = create_study_plan(
            "高数",
            "bilibili",
            "BV123",
            "全集",
            &plan_out,
            "2026-08-28",
            true,
        );

        assert_eq!(plan.start_date, "2026-08-28");
        // 28(五任务), 29(六休息), 30(日休息), 31(一任务)
        assert_eq!(plan.end_date, "2026-08-31");
        assert_eq!(plan.schedules.len(), 4);
        assert_eq!(plan.schedules[0].date, "2026-08-28");
        assert!(!plan.schedules[0].is_rest_day);
        assert_eq!(plan.schedules[1].date, "2026-08-29");
        assert!(plan.schedules[1].is_rest_day);
        assert_eq!(plan.schedules[2].date, "2026-08-30");
        assert!(plan.schedules[2].is_rest_day);
        assert_eq!(plan.schedules[3].date, "2026-08-31");
        assert!(!plan.schedules[3].is_rest_day);
    }

    #[test]
    fn task_checkin_and_progress() {
        let plan_out = mock_plan_out();
        let plan = create_study_plan(
            "高数",
            "bilibili",
            "BV123",
            "全集",
            &plan_out,
            "2026-09-01",
            false,
        );

        let task_id = plan.schedules[0].tasks[0].id.clone();
        let plan_id = plan.id.clone();
        let mut plans = vec![plan];

        let state = toggle_task_checkin(&mut plans, &plan_id, &task_id).unwrap();
        assert!(state);
        assert!(plans[0].schedules[0].tasks[0].completed);

        let (done, total, done_dur, total_dur, ratio) = compute_plan_progress(&plans[0]);
        assert_eq!(done, 1);
        assert_eq!(total, 4);
        assert_eq!(done_dur, 600);
        assert_eq!(total_dur, 2000);
        assert_eq!(ratio, 0.3);
    }

    #[test]
    fn push_forward_plan_test() {
        let plan_out = mock_plan_out();
        let mut plan = create_study_plan(
            "高数",
            "bilibili",
            "BV123",
            "全集",
            &plan_out,
            "2026-09-01",
            false,
        );

        // 打卡第 1 天任务 0
        plan.schedules[0].tasks[0].completed = true;

        assert!(push_forward_plan(&mut plan, "2026-09-02").unwrap());

        let first = plan
            .schedules
            .iter()
            .find(|schedule| schedule.date == "2026-09-01")
            .unwrap();
        let second = plan
            .schedules
            .iter()
            .find(|schedule| schedule.date == "2026-09-02")
            .unwrap();
        let third = plan
            .schedules
            .iter()
            .find(|schedule| schedule.date == "2026-09-03")
            .unwrap();
        assert_eq!(first.tasks.len(), 1);
        assert!(first.tasks[0].completed);
        assert_eq!(second.tasks.len(), 1); // 只保留 9/1 未完成部分
        assert!(!second.tasks[0].completed);
        assert_eq!(third.tasks.len(), 2); // 原 9/2 日程整体后移
        assert_eq!(plan.end_date, "2026-09-03");
    }

    #[test]
    fn multi_plan_superposition_and_stats_test() {
        let plan_out = mock_plan_out();
        let plan1 = create_study_plan(
            "高数",
            "bilibili",
            "BV1",
            "全集",
            &plan_out,
            "2026-09-01",
            false,
        );
        let plan2 = create_study_plan(
            "计网",
            "jellyfin",
            "item_123",
            "全集",
            &plan_out,
            "2026-09-01",
            false,
        );

        let mut plans = vec![plan1, plan2];

        // 9月1日应聚合两个科目的任务（每个科目2项，共4项）
        let today_tasks = get_tasks_for_date(&plans, "2026-09-01");
        assert_eq!(today_tasks.len(), 4);
        assert_eq!(today_tasks[0].plan_title, "高数");
        assert_eq!(today_tasks[2].plan_title, "计网");

        // 一键打卡高数第1天全部任务
        let pid0 = plans[0].id.clone();
        checkin_entire_day(&mut plans, &pid0, "2026-09-01").unwrap();
        assert!(plans[0].schedules[0].tasks[0].completed);
        assert!(plans[0].schedules[0].tasks[1].completed);

        // 统计信息
        let stats = compute_study_stats(&plans, "2026-09-01");
        assert_eq!(stats.active_plans, 2);
        assert_eq!(stats.today_total_tasks, 4);
        assert_eq!(stats.today_completed_tasks, 2);
        assert_eq!(stats.total_days_checked_in, 1);
        assert_eq!(stats.current_streak, 1);
    }

    #[test]
    fn push_forward_to_today_with_partial_checkin() {
        let plan_out = mock_plan_out();
        let mut plan = create_study_plan(
            "高数",
            "bilibili",
            "BV123",
            "全集",
            &plan_out,
            "2026-08-30",
            false,
        );

        // 8月30日当天完成任务0，但任务1未完成
        plan.schedules[0].tasks[0].completed = true;

        // 8月31日开始时，将 8月30日未完成条目顺延到当天
        push_forward_plan(&mut plan, "2026-08-31").unwrap();

        // 8月30日仅保留已完成任务；未完成任务独占下一日。
        let sch_today = plan
            .schedules
            .iter()
            .find(|s| s.date == "2026-08-30")
            .unwrap();
        assert_eq!(sch_today.tasks.len(), 1);
        assert!(sch_today.tasks[0].completed);
        let next = plan
            .schedules
            .iter()
            .find(|schedule| schedule.date == "2026-08-31")
            .unwrap();
        assert_eq!(next.tasks.len(), 1);
        assert!(!next.tasks[0].completed);
        assert!(plan.schedules.iter().any(|s| s.date == "2026-09-01"));
    }

    #[test]
    fn advance_partial_future_day_then_postpone_today_composes_correctly() {
        let plan_out = mock_plan_out();
        let mut plan = create_study_plan(
            "高数",
            "bilibili",
            "BV123",
            "全集",
            &plan_out,
            "2026-09-01",
            false,
        );
        // 今天完成 A、未完成 B；未来 9/2 提前完成 C、未完成 D。
        plan.schedules[0].tasks[0].completed = true;
        plan.schedules[1].tasks[0].completed = true;

        assert_eq!(
            advance_completed_tasks(&mut plan, "2026-09-02", "2026-09-01").unwrap(),
            1
        );
        let today = plan
            .schedules
            .iter()
            .find(|schedule| schedule.date == "2026-09-01")
            .unwrap();
        assert_eq!(today.tasks.len(), 3);
        assert_eq!(today.tasks.iter().filter(|task| task.completed).count(), 2);
        let future = plan
            .schedules
            .iter()
            .find(|schedule| schedule.date == "2026-09-02")
            .unwrap();
        assert_eq!(future.tasks.len(), 1); // 部分提前，后续日期不移动

        assert!(push_forward_plan(&mut plan, "2026-09-02").unwrap());
        let today = plan
            .schedules
            .iter()
            .find(|schedule| schedule.date == "2026-09-01")
            .unwrap();
        assert_eq!(today.tasks.len(), 2); // A、C 的完成记录都留在今天
        let next = plan
            .schedules
            .iter()
            .find(|schedule| schedule.date == "2026-09-02")
            .unwrap();
        assert_eq!(next.tasks.len(), 1); // 只放今天剩余的 B
        let day_after = plan
            .schedules
            .iter()
            .find(|schedule| schedule.date == "2026-09-03")
            .unwrap();
        assert_eq!(day_after.tasks.len(), 1); // 原 9/2 剩余 D 后移
    }

    #[test]
    fn advance_full_future_day_shifts_later_schedule_one_day_earlier() {
        let mut plan = create_custom_study_plan("背单词", "2026-09-01", 3, 30, false).unwrap();
        plan.schedules[1].tasks[0].completed = true;
        assert_eq!(
            advance_completed_tasks(&mut plan, "2026-09-02", "2026-09-01").unwrap(),
            1
        );
        let today = plan
            .schedules
            .iter()
            .find(|schedule| schedule.date == "2026-09-01")
            .unwrap();
        assert_eq!(today.tasks.len(), 2);
        let shifted = plan
            .schedules
            .iter()
            .find(|schedule| schedule.date == "2026-09-02")
            .unwrap();
        assert_eq!(shifted.tasks.len(), 1); // 原 9/3 日程提前到 9/2
        assert_eq!(plan.end_date, "2026-09-02");
    }

    #[test]
    fn cancelling_an_advanced_checkin_restores_its_original_date() {
        let plan_out = mock_plan_out();
        let mut plan = create_study_plan(
            "高数",
            "bilibili",
            "BV123",
            "全集",
            &plan_out,
            "2026-09-01",
            false,
        );
        plan.schedules[1].tasks[0].completed = true;
        let advanced_task_id = plan.schedules[1].tasks[0].id.clone();
        assert_eq!(
            advance_completed_tasks(&mut plan, "2026-09-02", "2026-09-01").unwrap(),
            1
        );
        let today = plan
            .schedules
            .iter()
            .find(|schedule| schedule.date == "2026-09-01")
            .unwrap();
        assert!(today
            .tasks
            .iter()
            .any(|task| task.id == advanced_task_id && task.completed));

        let plan_id = plan.id.clone();
        let mut plans = vec![plan];
        assert!(!toggle_task_checkin(&mut plans, &plan_id, &advanced_task_id).unwrap());
        let today = plans[0]
            .schedules
            .iter()
            .find(|schedule| schedule.date == "2026-09-01")
            .unwrap();
        assert!(!today.tasks.iter().any(|task| task.id == advanced_task_id));
        let original = plans[0]
            .schedules
            .iter()
            .find(|schedule| schedule.date == "2026-09-02")
            .unwrap();
        let restored = original
            .tasks
            .iter()
            .find(|task| task.id == advanced_task_id)
            .unwrap();
        assert!(!restored.completed);
        assert_eq!(restored.advanced_from_date, None);
    }

    #[test]
    fn advance_and_postpone_leave_other_plans_unchanged() {
        let mut missed = create_custom_study_plan("计划B", "2026-09-01", 2, 30, false).unwrap();
        let mut other = create_custom_study_plan("计划D", "2026-09-02", 1, 45, false).unwrap();
        let other_before = other.clone();

        assert!(push_forward_plan(&mut missed, "2026-09-02").unwrap());
        assert!(!push_forward_plan(&mut other, "2026-09-02").unwrap());
        assert_eq!(other, other_before);

        let mut early = create_custom_study_plan("计划C", "2026-09-01", 3, 20, false).unwrap();
        early.schedules[1].tasks[0].completed = true;
        assert_eq!(
            advance_completed_tasks(&mut early, "2026-09-02", "2026-09-01").unwrap(),
            1
        );
        assert_eq!(
            advance_completed_tasks(&mut other, "2026-09-02", "2026-09-01").unwrap(),
            0
        );
        assert_eq!(other, other_before);
    }

    #[test]
    fn streak_with_skip_weekends() {
        let plan_out = mock_plan_out();
        // 2026-08-28 是周五，08-29 周六，08-30 周日，08-31 周一
        let mut plan = create_study_plan(
            "高数",
            "bilibili",
            "BV123",
            "全集",
            &plan_out,
            "2026-08-28",
            true, // 跳过周末
        );

        // 周五 (08-28) 打卡
        plan.schedules[0].tasks[0].completed = true;

        let plans = vec![plan];

        // 在周日 08-30 查询 streak（周五已打卡，周末休息），连续打卡应当为 1
        let stats_sunday = compute_study_stats(&plans, "2026-08-30");
        assert_eq!(stats_sunday.current_streak, 1);

        // 在周一 08-31（尚未打卡）查询 streak，应追溯到周五，连续打卡应当仍为 1
        let stats_monday = compute_study_stats(&plans, "2026-08-31");
        assert_eq!(stats_monday.current_streak, 1);
    }

    #[test]
    fn calendar_matrix_and_month_stats_test() {
        let plan_out = mock_plan_out();
        let plan = create_study_plan(
            "高数",
            "bilibili",
            "BV123",
            "全集",
            &plan_out,
            "2026-08-15",
            false,
        );

        let plans = vec![plan];
        let matrix = generate_month_calendar_matrix(2026, 8, &plans);

        // 2026年8月网格行数应为 5 或 6 周（35 或 42 格）
        assert!(matrix.len() == 35 || matrix.len() == 42);

        // 验证 8月15日 当天包含高数的任务
        let day_15 = matrix.iter().find(|d| d.date == "2026-08-15").unwrap();
        assert_eq!(day_15.total_tasks, 2);
        assert_eq!(day_15.total_duration, 1000);
        assert_eq!(day_15.plan_titles, vec!["高数"]);

        // 月度统计
        let month_stats = compute_month_study_stats(2026, 8, &plans);
        assert_eq!(month_stats.total_tasks, 4);
        assert_eq!(month_stats.total_duration, 2000);
        assert_eq!(month_stats.active_study_days, 2);
    }
}
