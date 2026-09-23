use super::*;

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
            let fresh_body = match request {
                CloudRequest::PostJson(body) => {
                    let mut body = body.clone();
                    body["base_revision"] = serde_json::Value::from(0);
                    Some(body)
                }
                _ => None,
            };
            let retry = fresh_body
                .as_ref()
                .map(CloudRequest::PostJson)
                .unwrap_or(request);
            let data = send_cloud(&server, path, retry, &replacement, timeout)?;
            persist_cloud_token(cfg, replacement);
            cfg.sync_revision = 0;
            Ok(data)
        }
        Err(failure) => Err(failure),
    }
}

/// 请求云端 6 位绑定验证码。
pub fn request_cloud_bind_code(cfg: &mut AppConfig) -> Result<(String, u64), String> {
    let data = cloud_request(
        cfg,
        "/api/bind/request",
        CloudRequest::PostEmpty,
        CLOUD_TIMEOUT,
    )
    .map_err(CloudFailure::message)?;
    let code = data["bind_code"]
        .as_str()
        .ok_or_else(|| "缺少 bind_code".to_string())?
        .to_string();
    let expires = data["expires_in_secs"].as_u64().unwrap_or(600);
    save_config(cfg);
    Ok((code, expires))
}

/// 查询云端飞书绑定状态。
pub fn check_cloud_bind_status(cfg: &mut AppConfig) -> Result<bool, String> {
    // 尚未注册过设备时不发请求：一次状态查询不该顺带在云端建号。
    if cfg.sync_device_token.is_none() {
        return Ok(false);
    }
    let data = cloud_request(cfg, "/api/bind/status", CloudRequest::Get, CLOUD_TIMEOUT)
        .map_err(CloudFailure::message)?;
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
        "daily_notes": cfg.daily_notes,
        "base_revision": cfg.sync_revision
    });
    let data = cloud_request(
        cfg,
        "/api/sync",
        CloudRequest::PostJson(&body),
        CLOUD_SYNC_TIMEOUT,
    )
    .map_err(CloudFailure::message)?;

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

    if let Some(revision) = data.get("revision").and_then(|value| value.as_i64()) {
        cfg.sync_revision = revision;
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

    // The caller owns the live configuration. A background sync receives a
    // snapshot and must never persist that snapshot over newer UI edits.
    Ok("云端同步完成！".to_string())
}
