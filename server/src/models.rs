#[cfg(test)]
pub use planner_domain::study::{DailyNote, DailySchedule};
pub use planner_domain::study::{DailyNotes, PlanStatus, StudyPlan, TaskItem};
use serde::{Deserialize, Serialize};

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

/// 同步请求载荷。设备身份走 `Authorization: Bearer` 头，不在 body 里重复携带。
#[derive(Debug, Serialize, Deserialize)]
pub struct SyncPayload {
    /// 仅供旧版客户端滚动升级。新客户端必须使用 Authorization 头。
    #[serde(default)]
    pub device_token: Option<String>,
    /// Omitted only by a legacy client. New clients must send their last revision.
    #[serde(default)]
    pub base_revision: Option<i64>,
    #[serde(default)]
    pub plans: Vec<StudyPlan>,
    #[serde(default)]
    pub daily_notes: DailyNotes,
}

/// 设备注册响应：令牌由服务端签发，客户端持久化后用于后续所有请求。
#[derive(Debug, Serialize, Deserialize)]
pub struct RegisterResponse {
    pub device_token: String,
}

/// 同步失败原因。区分"令牌未知"与"存储故障"，前者要让客户端重新注册而不是重试。
#[derive(Debug)]
pub enum SyncError {
    UnknownDevice,
    StaleSnapshot,
    Storage,
}

/// 一次同步合并后的服务端状态。
#[derive(Debug)]
pub struct SyncOutcome {
    pub revision: i64,
    pub plans: Vec<StudyPlan>,
    pub daily_notes: DailyNotes,
    pub feishu_bound: bool,
    pub feishu_user_name: Option<String>,
    pub telegram_bound: bool,
    pub telegram_user_name: Option<String>,
}

/// 同步响应载荷。
#[derive(Debug, Serialize, Deserialize)]
pub struct SyncResponse {
    pub success: bool,
    pub revision: i64,
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
