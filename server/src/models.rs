use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 计划状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PlanStatus {
    #[default]
    Active,
    Paused,
    Completed,
    Archived,
}

#[allow(dead_code)]
impl PlanStatus {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Active => "进行中",
            Self::Paused => "已暂停",
            Self::Completed => "已完成",
            Self::Archived => "已归档",
        }
    }
}

/// 单项打卡任务。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskItem {
    pub id: String,
    pub vid_no: i64,
    pub title: String,
    pub portion: i64,
    pub remainder: i64,
    pub from_prev: bool,
    #[serde(default)]
    pub completed: bool,
    #[serde(default)]
    pub completed_at: Option<i64>,
    #[serde(default)]
    pub updated_at: i64,
    #[serde(default)]
    pub advanced_from_date: Option<String>,
}

/// 每日排期。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DailySchedule {
    pub day_index: usize,
    pub date: String,
    pub tasks: Vec<TaskItem>,
    pub is_rest_day: bool,
}

/// 科目学习计划。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StudyPlan {
    pub id: String,
    pub title: String,
    pub source_type: String,
    pub source_url: String,
    pub scope_desc: String,
    pub total_duration: i64,
    pub planned_days: usize,
    pub start_date: String,
    pub end_date: String,
    #[serde(default)]
    pub skip_weekends: bool,
    #[serde(default)]
    pub status: PlanStatus,
    #[serde(default)]
    pub created_at: i64,
    pub schedules: Vec<DailySchedule>,
    #[serde(default)]
    pub is_series: bool,
    #[serde(default = "default_show_in_library")]
    pub show_in_library: bool,
}

fn default_show_in_library() -> bool {
    true
}

/// 可跨端同步的单条日历备注。`deleted` 是删除同步用的 tombstone。
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

pub type DailyNotes = HashMap<String, Vec<DailyNote>>;

/// 绑定的设备用户。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DeviceUser {
    pub device_token: String,
    pub feishu_open_id: Option<String>,
    pub feishu_user_name: Option<String>,
    #[serde(default)]
    pub telegram_chat_id: Option<i64>,
    #[serde(default)]
    pub telegram_user_name: Option<String>,
    pub bind_code: Option<String>,
    pub bind_code_expires_at: i64,
    pub created_at: String,
}

/// 客户端请求绑定码。
#[derive(Debug, Serialize, Deserialize)]
pub struct BindRequestResponse {
    pub bind_code: String,
    pub device_token: String,
    pub expires_in_secs: u64,
}

/// 客户端查询绑定状态。
#[derive(Debug, Serialize, Deserialize)]
pub struct BindStatusResponse {
    pub bound: bool,
    pub feishu_bound: bool,
    pub feishu_user_name: Option<String>,
    #[serde(default)]
    pub telegram_bound: bool,
    #[serde(default)]
    pub telegram_user_name: Option<String>,
}

/// 同步请求载荷。
#[derive(Debug, Serialize, Deserialize)]
pub struct SyncPayload {
    pub device_token: String,
    pub plans: Vec<StudyPlan>,
    #[serde(default)]
    pub daily_notes: DailyNotes,
}

/// 同步响应载荷。
#[derive(Debug, Serialize, Deserialize)]
pub struct SyncResponse {
    pub success: bool,
    pub plans: Vec<StudyPlan>,
    #[serde(default)]
    pub daily_notes: DailyNotes,
    pub feishu_bound: bool,
    pub feishu_user_name: Option<String>,
    #[serde(default)]
    pub telegram_bound: bool,
    #[serde(default)]
    pub telegram_user_name: Option<String>,
    pub message: String,
}

/// 飞书事件通用包裹。
#[derive(Debug, Serialize, Deserialize)]
pub struct FeishuCallbackRequest {
    #[serde(default)]
    pub r#type: Option<String>,
    #[serde(default)]
    pub challenge: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub schema: Option<String>,
    #[serde(default)]
    pub header: Option<FeishuEventHeader>,
    #[serde(default)]
    pub event: Option<serde_json::Value>,
    #[serde(default)]
    pub action: Option<FeishuCardAction>,
    #[serde(default)]
    pub open_id: Option<String>,
    #[serde(default)]
    pub open_message_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FeishuEventHeader {
    pub event_id: Option<String>,
    pub event_type: Option<String>,
    pub create_time: Option<String>,
    pub token: Option<String>,
    pub app_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FeishuCardAction {
    pub value: serde_json::Value,
    pub tag: Option<String>,
    pub option: Option<String>,
}

/// 飞书卡片操作数据。
#[derive(Debug, Serialize, Deserialize)]
pub struct CardActionData {
    pub action: String, // "checkin", "push_forward", "refresh"
    pub plan_id: Option<String>,
    pub task_id: Option<String>,
    pub date: Option<String>,
}
