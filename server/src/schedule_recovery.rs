//! 客户端和机器人共用的排期补位/归位逻辑；历史按任务 ID 记录，避免恢复整份旧计划。
use crate::schedule_model::{DailySchedule, PlanStatus, StudyPlan};
use chrono::{Datelike, Duration, NaiveDate};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskDateMove {
    pub task_id: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleShift {
    pub trigger_task_ids: Vec<String>,
    pub moves: Vec<TaskDateMove>,
    /// 固定的日期槽位映射；某项后来手动改期后仍保留，以便更新其他批次。
    #[serde(default)]
    pub date_slots: Vec<(String, String)>,
}

pub fn refresh(plan: &mut StudyPlan) {
    // 日期唯一；休息日不能遮住真实任务。
    let mut days: BTreeMap<String, DailySchedule> = BTreeMap::new();
    for schedule in std::mem::take(&mut plan.schedules) {
        days.entry(schedule.date.clone())
            .or_insert_with(|| DailySchedule {
                day_index: 0,
                date: schedule.date.clone(),
                tasks: Vec::new(),
                is_rest_day: true,
            })
            .tasks
            .extend(schedule.tasks);
    }
    days.retain(|_, day| !day.tasks.is_empty());
    if plan.skip_weekends {
        if let (Some(first), Some(last)) = (
            days.keys().next().cloned(),
            days.keys().next_back().cloned(),
        ) {
            if let (Ok(mut date), Ok(last)) = (
                NaiveDate::parse_from_str(&first, "%Y-%m-%d"),
                NaiveDate::parse_from_str(&last, "%Y-%m-%d"),
            ) {
                while date < last {
                    if date.weekday().number_from_monday() >= 6 {
                        let date_text = date.format("%Y-%m-%d").to_string();
                        days.entry(date_text.clone()).or_insert(DailySchedule {
                            day_index: 0,
                            date: date_text,
                            tasks: Vec::new(),
                            is_rest_day: true,
                        });
                    }
                    date += Duration::days(1);
                }
            }
        }
    }
    plan.schedules = days.into_values().collect();
    let mut day_index = 0;
    for day in &mut plan.schedules {
        day.is_rest_day = day.tasks.is_empty();
        day.day_index = day_index;
        if !day.is_rest_day {
            day_index += 1;
        }
    }
    plan.planned_days = day_index;
    plan.total_duration = plan
        .schedules
        .iter()
        .flat_map(|s| &s.tasks)
        .map(|t| t.portion)
        .sum();
    if let Some(day) = plan.schedules.iter().find(|s| !s.tasks.is_empty()) {
        plan.start_date = day.date.clone();
    }
    if let Some(day) = plan.schedules.iter().rfind(|s| !s.tasks.is_empty()) {
        plan.end_date = day.date.clone();
    }
    let mut tasks = plan.schedules.iter().flat_map(|s| &s.tasks).peekable();
    let complete = tasks.peek().is_some() && tasks.all(|t| t.completed);
    match plan.status {
        PlanStatus::Active if complete => plan.status = PlanStatus::Completed,
        PlanStatus::Completed if !complete => plan.status = PlanStatus::Active,
        _ => {}
    }
}

/// 从空出的日期开始，后续每批任务依次填入前一批的日期，支持稀疏日期和周末例外。
#[allow(dead_code)] // 由桌面端调用；服务端读取其历史并负责撤销。
pub fn compact_day(plan: &mut StudyPlan, source: &str, trigger_task_ids: Vec<String>) {
    let mut indices: Vec<_> = plan
        .schedules
        .iter()
        .enumerate()
        .filter(|(_, s)| s.date.as_str() > source && !s.tasks.is_empty())
        .map(|(i, _)| i)
        .collect();
    indices.sort_by_key(|&i| plan.schedules[i].date.clone());
    let mut destination = source.to_string();
    let mut moves = Vec::new();
    for i in indices {
        let schedule = &mut plan.schedules[i];
        let original = schedule.date.clone();
        for task in &schedule.tasks {
            moves.push(TaskDateMove {
                task_id: task.id.clone(),
                from: original.clone(),
                to: destination.clone(),
            });
        }
        schedule.date = destination;
        destination = original;
    }
    if !trigger_task_ids.is_empty() && !moves.is_empty() {
        let date_slots = moves
            .iter()
            .map(|m| (m.from.clone(), m.to.clone()))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        plan.advance_shifts.push(ScheduleShift {
            trigger_task_ids,
            moves,
            date_slots,
        });
    }
}

/// 显式改期优先于旧的补位历史；撤销旧操作不能覆盖后来指定的日期。
#[allow(dead_code)] // 手动编辑入口只在桌面端。
pub fn detach_task(plan: &mut StudyPlan, task_id: &str) {
    for shift in &mut plan.advance_shifts {
        shift.moves.retain(|m| m.task_id != task_id);
        shift.trigger_task_ids.retain(|id| id != task_id);
    }
    plan.advance_shifts
        .retain(|s| !s.trigger_task_ids.is_empty() && !s.moves.is_empty());
}

fn undo_compaction(plan: &mut StudyPlan, trigger: &str) {
    let Some(index) = plan
        .advance_shifts
        .iter()
        .position(|s| s.trigger_task_ids.iter().any(|id| id == trigger))
    else {
        return;
    };
    let shift = plan.advance_shifts.remove(index);
    let inverse: HashMap<_, _> = if shift.date_slots.is_empty() {
        shift
            .moves
            .iter()
            .map(|m| (m.to.clone(), m.from.clone()))
            .collect()
    } else {
        shift
            .date_slots
            .iter()
            .map(|(from, to)| (to.clone(), from.clone()))
            .collect()
    };
    let affected: HashSet<_> = shift.moves.iter().map(|m| m.task_id.as_str()).collect();
    // 后续提前操作的还原位置也要随之更新，支持不同顺序撤销多个提前批次。
    for later in plan.advance_shifts.iter_mut().skip(index) {
        for (from, to) in &mut later.date_slots {
            if let Some(date) = inverse.get(from) {
                *from = date.clone();
            }
            if let Some(date) = inverse.get(to) {
                *to = date.clone();
            }
        }
        for movement in &mut later.moves {
            if affected.contains(movement.task_id.as_str()) {
                if let Some(date) = inverse.get(&movement.from) {
                    movement.from = date.clone();
                }
                if let Some(date) = inverse.get(&movement.to) {
                    movement.to = date.clone();
                }
            }
        }
    }
    let mut relocations = HashMap::new();
    for day in &mut plan.schedules {
        for task in &mut day.tasks {
            if !affected.contains(task.id.as_str()) {
                continue;
            }
            if let Some(origin) = &mut task.advanced_from_date {
                if let Some(date) = inverse.get(origin) {
                    *origin = date.clone();
                }
            } else if let Some(date) = inverse.get(&day.date) {
                relocations.insert(task.id.clone(), date.clone());
            }
        }
    }
    relocate(plan, &relocations);
}

fn relocate(plan: &mut StudyPlan, dates: &HashMap<String, String>) {
    let mut moved = Vec::new();
    for schedule in &mut plan.schedules {
        for task in std::mem::take(&mut schedule.tasks) {
            if let Some(date) = dates.get(&task.id) {
                moved.push((date.clone(), task));
            } else {
                schedule.tasks.push(task);
            }
        }
    }
    for (date, task) in moved {
        if let Some(day) = plan.schedules.iter_mut().find(|s| s.date == date) {
            day.tasks.push(task);
            day.is_rest_day = false;
        } else {
            plan.schedules.push(DailySchedule {
                day_index: 0,
                date,
                tasks: vec![task],
                is_rest_day: false,
            });
        }
    }
}

/// 服务端保留归位信号，直到客户端采纳。重复应用同一信号不会重复推后日程。
pub fn restore(plan: &mut StudyPlan, keep_signal: bool) {
    let signals: Vec<_> = plan
        .schedules
        .iter()
        .flat_map(|s| &s.tasks)
        .filter(|t| t.advanced_from_date.is_some() && (!t.completed || t.advance_restored))
        .map(|t| t.id.clone())
        .collect();
    for id in signals {
        undo_compaction(plan, &id);
        let origin = plan
            .schedules
            .iter()
            .flat_map(|s| &s.tasks)
            .find(|t| t.id == id)
            .and_then(|t| t.advanced_from_date.clone());
        if let Some(origin) = origin {
            relocate(plan, &HashMap::from([(id.clone(), origin)]));
        }
        if let Some(task) = plan
            .schedules
            .iter_mut()
            .flat_map(|s| &mut s.tasks)
            .find(|t| t.id == id)
        {
            task.advance_restored = keep_signal;
            if !keep_signal {
                task.advanced_from_date = None;
            }
        }
    }
    refresh(plan);
}

pub fn toggle(plan: &mut StudyPlan, task_id: &str, now: i64, keep_signal: bool) -> Option<bool> {
    let task = plan
        .schedules
        .iter_mut()
        .flat_map(|s| &mut s.tasks)
        .find(|t| t.id == task_id)?;
    task.completed = !task.completed;
    task.completed_at = task.completed.then_some(now);
    task.updated_at = now.max(task.updated_at.saturating_add(1));
    let completed = task.completed;
    restore(plan, keep_signal);
    Some(completed)
}

/// 排期由客户端主控，云端仅合并新打卡和归位信号，避免覆盖同步期间的手动改期。
pub fn merge_checkins(local: &mut StudyPlan, remote: &StudyPlan, keep_signal: bool) {
    let remote_tasks: HashMap<_, _> = remote
        .schedules
        .iter()
        .flat_map(|s| &s.tasks)
        .map(|t| (&t.id, t))
        .collect();
    for task in local.schedules.iter_mut().flat_map(|s| &mut s.tasks) {
        if let Some(rt) = remote_tasks.get(&task.id) {
            if rt.updated_at > task.updated_at {
                task.completed = rt.completed;
                task.completed_at = rt.completed_at;
                task.updated_at = rt.updated_at;
                task.advanced_from_date = rt.advanced_from_date.clone();
                task.advance_restored = rt.advance_restored;
            }
        }
    }
    restore(local, keep_signal);
}
