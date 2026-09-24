use chrono::Utc;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info};

#[derive(Clone)]
pub struct FeishuClient {
    app_id: String,
    app_secret: String,
    http: Client,
    token_cache: Arc<RwLock<Option<(String, i64)>>>, // (token, expire_timestamp)
}

#[derive(Deserialize)]
struct TokenResponse {
    code: i32,
    msg: String,
    tenant_access_token: Option<String>,
    expire: Option<i64>,
}

#[derive(Deserialize)]
struct MessageResponse {
    code: i32,
    msg: Option<String>,
    data: Option<SentMessage>,
}

#[derive(Deserialize)]
struct SentMessage {
    message_id: String,
}

impl MessageResponse {
    fn message_id(self) -> Result<String, String> {
        if self.code != 0 {
            return Err(format!(
                "发送失败: {}",
                self.msg.as_deref().unwrap_or("未知错误")
            ));
        }
        self.data
            .map(|data| data.message_id)
            .ok_or_else(|| "飞书成功响应缺少 message_id".to_string())
    }
}

impl FeishuClient {
    pub fn new(app_id: impl Into<String>, app_secret: impl Into<String>) -> Self {
        Self {
            app_id: app_id.into(),
            app_secret: app_secret.into(),
            http: Client::new(),
            token_cache: Arc::new(RwLock::new(None)),
        }
    }

    /// 获取或自动刷新 tenant_access_token。
    pub async fn get_tenant_access_token(&self) -> Result<String, String> {
        let now = Utc::now().timestamp();
        {
            let guard = self.token_cache.read().await;
            if let Some((token, expire)) = &*guard {
                if *expire > now + 120 {
                    return Ok(token.clone());
                }
            }
        }

        let url = "https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal";
        let body = json!({
            "app_id": self.app_id,
            "app_secret": self.app_secret
        });

        let res = self
            .http
            .post(url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("请求飞书 Token 失败: {}", e))?;

        let data: TokenResponse = res
            .json()
            .await
            .map_err(|e| format!("解析飞书 Token 响应失败: {}", e))?;

        if data.code != 0 {
            return Err(format!("飞书鉴权失败 (code {}): {}", data.code, data.msg));
        }

        let token = data
            .tenant_access_token
            .ok_or_else(|| "缺少 tenant_access_token".to_string())?;
        let expire = now + data.expire.unwrap_or(7200);

        let mut guard = self.token_cache.write().await;
        *guard = Some((token.clone(), expire));
        info!("飞书 Token 刷新成功，有效期至 {}", expire);
        Ok(token)
    }

    /// 向用户发送交互式卡片消息。
    pub async fn send_card_message(&self, open_id: &str, card: Value) -> Result<String, String> {
        let token = self.get_tenant_access_token().await?;
        let url = "https://open.feishu.cn/open-apis/im/v1/messages?receive_id_type=open_id";

        let payload = json!({
            "receive_id": open_id,
            "msg_type": "interactive",
            "content": serde_json::to_string(&card).unwrap_or_default()
        });

        let res = self
            .http
            .post(url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("发送飞书卡片网络错误: {}", e))?;

        let status = res.status();
        let body: MessageResponse = res
            .json()
            .await
            .map_err(|e| format!("解析发送响应错误: {}", e))?;

        let msg_id = body.message_id().map_err(|message| {
            error!("发送飞书卡片失败 (HTTP {}): {}", status, message);
            message
        })?;
        info!("成功向飞书用户 {} 发送卡片消息 (id: {})", open_id, msg_id);
        Ok(msg_id)
    }

    /// 向用户发送纯文本消息。
    pub async fn send_text_message(&self, open_id: &str, text: &str) -> Result<String, String> {
        let token = self.get_tenant_access_token().await?;
        let url = "https://open.feishu.cn/open-apis/im/v1/messages?receive_id_type=open_id";

        let content_json = json!({ "text": text });
        let payload = json!({
            "receive_id": open_id,
            "msg_type": "text",
            "content": serde_json::to_string(&content_json).unwrap_or_default()
        });

        let res = self
            .http
            .post(url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("发送飞书文本网络错误: {}", e))?;

        let body: MessageResponse = res
            .json()
            .await
            .map_err(|e| format!("解析发送响应错误: {}", e))?;

        body.message_id()
    }
}

#[cfg(test)]
mod tests {
    use super::MessageResponse;

    #[test]
    fn send_response_requires_a_message_id_on_success() {
        let ok: MessageResponse =
            serde_json::from_str(r#"{"code":0,"msg":"success","data":{"message_id":"om_123"}}"#)
                .unwrap();
        assert_eq!(ok.message_id().unwrap(), "om_123");
        let incomplete: MessageResponse = serde_json::from_str(r#"{"code":0}"#).unwrap();
        assert!(incomplete.message_id().is_err());
        let failed: MessageResponse =
            serde_json::from_str(r#"{"code":999,"msg":"invalid token"}"#).unwrap();
        assert!(failed.message_id().unwrap_err().contains("invalid token"));
    }
}
