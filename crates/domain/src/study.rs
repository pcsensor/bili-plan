//! 进度打卡与多科目学习管理核心逻辑（无 GUI 依赖，纯函数编排）。
//!
//! 提供计划实体、日历排期计算、多科目聚合今日任务、任务打卡与统计、一键顺延等功能。

use chrono::{Datelike, Duration, Local, NaiveDate};
use std::collections::HashMap;

use crate::plan::PlanOut;
use crate::source::SourceKind;

pub use crate::model::{
    deserialize_daily_notes, DailyNote, DailyNotes, DailySchedule, PlanStatus, StudyPlan, TaskItem,
};

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

fn validate_date(value: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d")
        .map_err(|_| "日期格式应为 YYYY-MM-DD。".to_string())
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

/// 根据“指定日期是计划第几天”反推出计划起始日期。
///
/// `day_number` 从 1 开始；开启跳过周末时，周六和周日不计入计划天数。
pub fn infer_plan_start_date(
    date_str: &str,
    day_number: usize,
    skip_weekends: bool,
) -> Result<String, String> {
    if day_number == 0 {
        return Err("今天是第几天必须是正整数。".to_string());
    }

    let mut date = NaiveDate::parse_from_str(date_str, "%Y-%m-%d")
        .map_err(|_| "日期格式应为 YYYY-MM-DD。".to_string())?;
    if skip_weekends && is_weekend(date) {
        return Err("今天是周末，开启“跳过周末”后不能作为计划学习日。".to_string());
    }

    let mut days_to_rewind = day_number - 1;
    while days_to_rewind > 0 {
        date -= Duration::days(1);
        if !skip_weekends || !is_weekend(date) {
            days_to_rewind -= 1;
        }
    }
    Ok(format_date(date))
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
) -> Result<StudyPlan, String> {
    let start_date = validate_date(start_date_str)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let plan_id = format!("plan_{}_{}", now, fast_rand_suffix());

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
                advance_restored: false,
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

    Ok(StudyPlan {
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
        advance_shifts: Vec::new(),
        is_series: false,
        show_in_library: true,
    })
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
    let start_date = validate_date(start_date_str)?;
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
                advance_restored: false,
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
        source_type: SourceKind::Custom.tag().to_string(),
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
        advance_shifts: Vec::new(),
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
        id: {
            static NEXT_TASK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let sequence = NEXT_TASK.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default();
            format!("{plan_id}_{day_index}_{item_index}_{nanos}_{sequence}")
        },
        vid_no,
        title: title.to_string(),
        portion,
        from_prev: false,
        remainder: 0,
        completed: false,
        completed_at: None,
        updated_at: 0,
        advanced_from_date: None,
        advance_restored: false,
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
    crate::schedule_recovery::refresh(plan);
}

/// 从日历创建一个一次性任务。它会参与当日打卡和统计，但不显示在计划库中。
pub fn create_one_off_calendar_task(
    task_title: &str,
    date_str: &str,
    minutes: i64,
) -> Result<StudyPlan, String> {
    let (task_title, portion) = validate_calendar_task(task_title, minutes)?;
    let date = format_date(validate_date(date_str)?);
    let now = now_timestamp();
    let plan_id = format!("calendar_{}_{}", now, fast_rand_suffix());
    Ok(StudyPlan {
        id: plan_id.clone(),
        title: task_title.clone(),
        source_type: SourceKind::Calendar.tag().to_string(),
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
        advance_shifts: Vec::new(),
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
    let date = format_date(validate_date(date_str)?);
    let now = now_timestamp();
    let plan_id = format!("series_{}_{}", now, fast_rand_suffix());
    Ok(StudyPlan {
        id: plan_id.clone(),
        title: series_name.to_string(),
        source_type: SourceKind::Calendar.tag().to_string(),
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
        advance_shifts: Vec::new(),
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
    let date = format_date(validate_date(date_str)?);
    let vid_no = plan
        .schedules
        .iter()
        .flat_map(|schedule| schedule.tasks.iter())
        .map(|task| task.vid_no)
        .max()
        .unwrap_or(0)
        .saturating_add(1);

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

/// 手动向前改期清空原日后，后续任务依次补位；允许周末作为手动例外。
/// 新日期取代提前归位日期，打卡状态和视频切片信息保持不变。
pub fn move_task_to_date(
    plan: &mut StudyPlan,
    task_id: &str,
    date_str: &str,
) -> Result<(), String> {
    let date = NaiveDate::parse_from_str(date_str.trim(), "%Y-%m-%d")
        .map_err(|_| "日期格式应为 YYYY-MM-DD。".to_string())?;
    let target_date = format_date(date);
    let (source_index, task_index) = plan
        .schedules
        .iter()
        .enumerate()
        .find_map(|(index, schedule)| {
            schedule
                .tasks
                .iter()
                .position(|task| task.id == task_id)
                .map(|task_index| (index, task_index))
        })
        .ok_or_else(|| "未找到指定任务。".to_string())?;
    if plan.schedules[source_index].date == target_date {
        return Ok(());
    }
    let source_date = plan.schedules[source_index].date.clone();
    crate::schedule_recovery::detach_task(plan, task_id);
    let mut task = plan.schedules[source_index].tasks.remove(task_index);
    task.advanced_from_date = None;
    task.advance_restored = false;
    if target_date < source_date && plan.schedules[source_index].tasks.is_empty() {
        crate::schedule_recovery::compact_day(plan, &source_date, Vec::new());
    }
    task.updated_at = now_timestamp().max(task.updated_at.saturating_add(1));
    if let Some(schedule) = plan.schedules.iter_mut().find(|s| s.date == target_date) {
        schedule.is_rest_day = false;
        schedule.tasks.push(task);
    } else {
        plan.schedules.push(DailySchedule {
            day_index: 0,
            date: target_date,
            tasks: vec![task],
            is_rest_day: false,
        });
    }
    plan.schedules
        .retain(|s| !s.tasks.is_empty() || s.is_rest_day);
    refresh_plan_schedule_summary(plan);
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
    if SourceKind::from_tag(&plan.source_type) != SourceKind::Calendar {
        return Err("只能编辑通过日历创建的任务。".to_string());
    }
    let (task_title, portion) = validate_calendar_task(task_title, minutes)?;
    let one_off_title = task_title.clone();
    move_task_to_date(plan, task_id, date_str)?;
    let task = plan
        .schedules
        .iter_mut()
        .flat_map(|s| &mut s.tasks)
        .find(|t| t.id == task_id)
        .expect("validated task");
    task.title = task_title;
    task.portion = portion;
    task.updated_at = now_timestamp().max(task.updated_at.saturating_add(1));
    if !plan.is_series {
        plan.title = one_off_title;
    }
    refresh_calendar_series_summary(plan);
    Ok(())
}

/// 删除日历创建的一项任务。返回 `true` 表示计划已经没有任务，调用方应将计划移除。
pub fn delete_calendar_task(plan: &mut StudyPlan, task_id: &str) -> Result<bool, String> {
    if SourceKind::from_tag(&plan.source_type) != SourceKind::Calendar {
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
    crate::schedule_recovery::detach_task(plan, task_id);
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
    let plan = plans
        .iter_mut()
        .find(|p| p.id == plan_id)
        .ok_or_else(|| "未找到对应计划".to_string())?;
    crate::schedule_recovery::toggle(plan, task_id, now_timestamp(), false)
        .ok_or_else(|| "未找到对应的任务".to_string())
}

/// 应用云端归位信号，同时恢复该次整日提前引起的后续补位。
pub fn restore_cancelled_advanced_tasks(plans: &mut [StudyPlan]) {
    for plan in plans {
        crate::schedule_recovery::restore(plan, false);
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
                        if !task.completed {
                            task.completed = true;
                            task.completed_at = Some(now);
                            task.updated_at = now.max(task.updated_at.saturating_add(1));
                        }
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

fn refresh_plan_schedule_summary(plan: &mut StudyPlan) {
    crate::schedule_recovery::refresh(plan);
}

/// 所有过去未完成任务按原日期分批排到今天起的学习日，已完成记录留在原处。
/// 后续日程为积压批次让位；周末手动任务也会被收集，重复执行不会再次移动。
pub fn push_forward_plan(plan: &mut StudyPlan, destination_date_str: &str) -> Result<bool, String> {
    let destination = validate_date(destination_date_str)?;
    let target = format_date(destination);
    let mut backlog: Vec<_> = plan
        .schedules
        .iter()
        .filter(|s| s.date < target && s.tasks.iter().any(|t| !t.completed))
        .cloned()
        .collect();
    if backlog.is_empty() {
        return Ok(false);
    }
    backlog.sort_by(|a, b| a.date.cmp(&b.date));
    let count = backlog.len();
    let skip_weekends = plan.skip_weekends;
    // 将历史中的空位也纳入日期映射，防止周末例外在顺延后映射到同一天。
    let mut future_dates = std::collections::BTreeSet::new();
    for schedule in &plan.schedules {
        if !schedule.tasks.is_empty() {
            future_dates.insert(schedule.date.clone());
        }
        for task in &schedule.tasks {
            if let Some(origin) = &task.advanced_from_date {
                future_dates.insert(origin.clone());
            }
        }
    }
    for movement in plan.advance_shifts.iter().flat_map(|s| &s.moves) {
        future_dates.insert(movement.from.clone());
        future_dates.insert(movement.to.clone());
    }
    for (from, to) in plan.advance_shifts.iter().flat_map(|s| &s.date_slots) {
        future_dates.insert(from.clone());
        future_dates.insert(to.clone());
    }
    let mut date_map = HashMap::new();
    let mut previous = None;
    for value in future_dates.into_iter().filter(|d| d >= &target) {
        let mut d = validate_date(&value)?;
        for _ in 0..count {
            d = next_learning_date(d, skip_weekends);
        }
        if let Some(previous) = previous {
            d = d.max(next_learning_date(previous, skip_weekends));
        }
        previous = Some(d);
        date_map.insert(value, format_date(d));
    }
    let shift_date = |value: &str| {
        date_map
            .get(value)
            .cloned()
            .unwrap_or_else(|| value.to_string())
    };
    // 顺延是新操作，旧提前记录的归位位置也必须相应后移。
    for shift in &mut plan.advance_shifts {
        for (from, to) in &mut shift.date_slots {
            *from = shift_date(from);
            *to = shift_date(to);
        }
        for movement in &mut shift.moves {
            movement.from = shift_date(&movement.from);
            movement.to = shift_date(&movement.to);
        }
    }
    for task in plan.schedules.iter_mut().flat_map(|s| &mut s.tasks) {
        if let Some(origin) = &mut task.advanced_from_date {
            *origin = shift_date(origin);
        }
    }
    let mut rebuilt: Vec<_> = plan
        .schedules
        .iter()
        .filter(|s| s.date < target)
        .cloned()
        .map(|mut s| {
            s.tasks.retain(|t| t.completed);
            s
        })
        .collect();
    let mut cursor = destination;
    if skip_weekends {
        while is_weekend(cursor) {
            cursor += Duration::days(1);
        }
    }
    for mut schedule in backlog {
        schedule.tasks.retain(|t| !t.completed);
        schedule.date = format_date(cursor);
        schedule.is_rest_day = false;
        rebuilt.push(schedule);
        cursor = next_learning_date(cursor, skip_weekends);
    }
    let mut future: Vec<_> = plan
        .schedules
        .iter()
        .filter(|s| s.date >= target && !s.tasks.is_empty())
        .cloned()
        .collect();
    future.sort_by(|a, b| a.date.cmp(&b.date));
    for mut schedule in future {
        let shifted = parse_date_or_today(&shift_date(&schedule.date));
        let actual = shifted.max(cursor);
        schedule.date = format_date(actual);
        rebuilt.push(schedule);
        cursor = next_learning_date(actual, skip_weekends);
    }
    plan.schedules = rebuilt;
    refresh_plan_schedule_summary(plan);
    Ok(true)
}

/// 将计划中所有未完成任务整体平移，使最早的未完成任务从 `target_start_date` 开始。
///
/// 每个未完成日期批次使用相同的自然日偏移量，因此原有批次间隔、手动周末安排与任务顺序
/// 都会保留；已完成任务始终留在原日期。整体改期属于新的显式安排，会清除这些未完成任务
/// 之前由“一键提前”产生的归位信号，避免以后撤销旧操作时覆盖新排期。
pub fn reschedule_unfinished_plan(
    plan: &mut StudyPlan,
    target_start_date: &str,
) -> Result<bool, String> {
    let target_start = validate_date(target_start_date)?;
    let Some(current_start_text) = plan
        .schedules
        .iter()
        .filter(|schedule| schedule.tasks.iter().any(|task| !task.completed))
        .map(|schedule| schedule.date.as_str())
        .min()
    else {
        return Ok(false);
    };
    let current_start = validate_date(current_start_text)?;
    let offset_days = target_start.signed_duration_since(current_start).num_days();
    if offset_days == 0 {
        return Ok(false);
    }

    let unfinished_ids: Vec<String> = plan
        .schedules
        .iter()
        .flat_map(|schedule| &schedule.tasks)
        .filter(|task| !task.completed)
        .map(|task| task.id.clone())
        .collect();
    for task_id in &unfinished_ids {
        crate::schedule_recovery::detach_task(plan, task_id);
    }

    let mut rebuilt = Vec::new();
    for schedule in std::mem::take(&mut plan.schedules) {
        let source_date = validate_date(&schedule.date)?;
        let shifted_date = source_date
            .checked_add_signed(Duration::days(offset_days))
            .ok_or_else(|| "目标日期超出支持范围。".to_string())?;
        let shifted_date = format_date(shifted_date);
        let mut completed_tasks = Vec::new();
        let mut unfinished_tasks = Vec::new();
        for mut task in schedule.tasks {
            if task.completed {
                completed_tasks.push(task);
            } else {
                task.advanced_from_date = None;
                task.advance_restored = false;
                unfinished_tasks.push(task);
            }
        }
        if !completed_tasks.is_empty() {
            rebuilt.push(DailySchedule {
                day_index: schedule.day_index,
                date: schedule.date,
                tasks: completed_tasks,
                is_rest_day: false,
            });
        }
        if !unfinished_tasks.is_empty() {
            rebuilt.push(DailySchedule {
                day_index: schedule.day_index,
                date: shifted_date,
                tasks: unfinished_tasks,
                is_rest_day: false,
            });
        }
    }

    plan.schedules = rebuilt;
    refresh_plan_schedule_summary(plan);
    Ok(true)
}

/// 将指定未来日期中已打卡的任务条目移动到今天。
///
/// 若该计划在指定未来日期的全部任务均已完成，则移除该日并把更晚的日程
/// 按实际日期依次补位；若只完成部分，则指定日期保留未完成条目，后续日程不动。
pub fn advance_completed_tasks(
    plan: &mut StudyPlan,
    future_date_str: &str,
    today_str: &str,
) -> Result<usize, String> {
    let today = format_date(validate_date(today_str)?);
    let future_date = format_date(validate_date(future_date_str)?);
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
            task.advance_restored = false;
            task.updated_at = now_timestamp().max(task.updated_at.saturating_add(1));
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
        let triggers = plan
            .schedules
            .iter()
            .flat_map(|s| &s.tasks)
            .filter(|t| t.advanced_from_date.as_deref() == Some(&future_date))
            .map(|t| t.id.clone())
            .chain(moved.iter().map(|t| t.id.clone()))
            .collect();
        crate::schedule_recovery::compact_day(plan, &future_date, triggers);
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
mod tests;
