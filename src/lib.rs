//! Bilibili 合集观看计划生成器核心库（业务层无 GUI 依赖，可纯逻辑测试）。

pub mod api;
pub mod app;
pub mod assets;
pub mod core;
pub mod error;
pub mod export;
pub mod jellyfin;
pub mod model;
pub mod parse;
pub mod plan;
pub mod study;
pub mod theme;

pub use error::{Error, ErrorKind, Result};
pub use jellyfin::{
    classify_item, extract_base_url, extract_item_id, fetch_groups, group_episodes_by_season,
    group_episodes_by_series, ticks_to_secs, Item, JellyfinClient,
};
pub use parse::{extract_bvid, extract_sid, parse_groups, Group, ParseResult};
pub use plan::{build_plan, Mode, PlanEntry, PlanOut};
pub use study::{
    add_daily_note, advance_completed_tasks, append_calendar_series_task, compute_plan_progress,
    compute_study_stats, create_calendar_series, create_custom_study_plan,
    create_one_off_calendar_task, create_study_plan, delete_calendar_task, delete_daily_note,
    get_daily_notes, get_tasks_for_date, merge_daily_notes, push_forward_plan, today_date_str,
    toggle_task_checkin, update_calendar_task, DailyNote, DailyNotes, DailySchedule, PlanStatus,
    StudyPlan, StudyStats, TaskItem, TodayTaskView,
};
