//! Stable persisted model shared by the desktop and server.
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;

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
    /// 机器人撤销提前后保留至客户端确认的归位信号。
    #[serde(default)]
    pub advance_restored: bool,
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
    /// 整日提前引起的补位历史；取消其中任一任务打卡时只撤销一次。
    #[serde(default)]
    pub advance_shifts: Vec<ScheduleShift>,
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
