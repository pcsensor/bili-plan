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
    pub event: Option<FeishuEvent>,
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
    pub value: FeishuActionValue,
    pub tag: Option<String>,
    pub option: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FeishuActionValue {
    Data(CardActionData),
    Encoded(String),
}

impl FeishuActionValue {
    pub fn decode(&self) -> Option<CardActionData> {
        match self {
            Self::Data(data) => Some(data.clone()),
            Self::Encoded(text) => serde_json::from_str(text).ok(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FeishuEventAction {
    Wrapped(FeishuCardAction),
    Direct(CardActionData),
}

impl FeishuEventAction {
    pub fn decode(&self) -> Option<CardActionData> {
        match self {
            Self::Wrapped(action) => action.value.decode(),
            Self::Direct(data) => Some(data.clone()),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FeishuEvent {
    pub action: Option<FeishuEventAction>,
    pub operator: Option<FeishuOperator>,
    pub sender: Option<FeishuSender>,
    pub message: Option<FeishuMessage>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FeishuOperator {
    pub open_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FeishuSender {
    pub sender_id: Option<FeishuSenderId>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FeishuSenderId {
    pub open_id: Option<String>,
    pub user_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FeishuMessage {
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct FeishuTextContent {
    pub text: String,
}

/// 飞书卡片操作数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardActionData {
    pub action: String, // "checkin", "push_forward", "refresh"
    pub plan_id: Option<String>,
    pub task_id: Option<String>,
    pub date: Option<String>,
}

#[cfg(test)]
mod feishu_callback_tests {
    use super::{FeishuCallbackRequest, FeishuTextContent};

    #[test]
    fn old_and_new_card_payloads_decode_to_the_same_action() {
        let old: FeishuCallbackRequest = serde_json::from_str(
            r#"{"token":"secret","open_id":"ou_1","action":{"value":"{\"action\":\"checkin\",\"plan_id\":\"p\",\"task_id\":\"t\"}"}}"#,
        )
        .unwrap();
        let new: FeishuCallbackRequest = serde_json::from_str(
            r#"{"schema":"2.0","header":{"token":"secret"},"event":{"operator":{"open_id":"ou_1"},"action":{"value":{"action":"checkin","plan_id":"p","task_id":"t"}}}}"#,
        )
        .unwrap();
        let old_action = old.action.unwrap().value.decode().unwrap();
        let new_action = new.event.unwrap().action.unwrap().decode().unwrap();
        assert_eq!(old_action.action, new_action.action);
        assert_eq!(old_action.plan_id, new_action.plan_id);
        assert_eq!(old_action.task_id, new_action.task_id);
    }

    #[test]
    fn message_content_fixture_has_a_typed_text_field() {
        let content: FeishuTextContent = serde_json::from_str(r#"{"text":"/today"}"#).unwrap();
        assert_eq!(content.text, "/today");
    }
}
