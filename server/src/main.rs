mod auth;
mod ratelimit;
use planner_domain::schedule_recovery;
mod card;
mod feishu;
mod models;
mod scheduler;
mod store;
mod telegram;

use axum::{
    extract::{ConnectInfo, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use card::{build_my_plans_card, build_today_study_card};
use chrono::Local;
use feishu::FeishuClient;
use models::{
    BindRequestResponse, BindStatusResponse, CardActionData, DeviceUser, FeishuCallbackRequest,
    RegisterResponse, SyncError, SyncPayload, SyncResponse,
};
use ratelimit::RateLimiter;
use serde::Deserialize;
use serde_json::{json, Value};
use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;
use store::Store;
use telegram::TelegramClient;
use tower_http::trace::TraceLayer;
use tracing::{error, info, info_span, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

const MINUTE: Duration = Duration::from_secs(60);
const HOUR: Duration = Duration::from_secs(3600);

/// 注册是唯一的匿名写入口，每台设备一生只需一次，因此按 IP 卡得最紧。
const REGISTER_PER_IP: u32 = 20;
/// 同步由客户端每次改动触发，人类点击速率下 120/min 远超实际用量。
const SYNC_PER_TOKEN: u32 = 120;
/// 同一出口 IP 可能有多名 NAT 后的用户，放宽到单设备额度的数倍。
const SYNC_PER_IP: u32 = 600;
const BIND_REQUEST_PER_TOKEN: u32 = 10;
const BIND_REQUEST_PER_IP: u32 = 40;
const BIND_STATUS_PER_TOKEN: u32 = 60;
const BIND_STATUS_PER_IP: u32 = 300;

#[derive(Clone)]
struct AppState {
    store: Store,
    feishu: FeishuClient,
    feishu_verification_token: String,
    limiter: RateLimiter,
    allow_legacy_token_transport: bool,
}

type ApiError = (StatusCode, Json<Value>);

#[derive(Clone, Copy)]
struct RatePolicy {
    scope: &'static str,
    token_limit: u32,
    ip_limit: u32,
    window: Duration,
}

fn api_error(status: StatusCode, message: &str) -> ApiError {
    (
        status,
        Json(json!({ "success": false, "message": message })),
    )
}

fn api_error_code(status: StatusCode, code: &str, message: &str) -> ApiError {
    (
        status,
        Json(json!({ "success": false, "code": code, "message": message })),
    )
}

/// 校验 `Authorization: Bearer <device_token>` 并施加限流，通过后返回设备。
///
/// 令牌缺失或畸形回 401；格式正确但服务端查无此设备回 404，由客户端据此重新注册，
/// 而不是像从前那样在同步路径上静默建号。
async fn authorize_device(
    state: &AppState,
    headers: &HeaderMap,
    peer: &SocketAddr,
    policy: RatePolicy,
    legacy_token: Option<&str>,
) -> Result<DeviceUser, ApiError> {
    let credential = auth::device_credential(headers, legacy_token).ok_or_else(|| {
        api_error(
            StatusCode::UNAUTHORIZED,
            "缺少或无效的 Authorization: Bearer 设备令牌",
        )
    })?;
    let ip = auth::client_ip(headers, Some(peer));
    if !state.limiter.allow(
        &format!("{}:ip:{ip}", policy.scope),
        policy.ip_limit,
        policy.window,
    ) {
        return Err(api_error(
            StatusCode::TOO_MANY_REQUESTS,
            "请求过于频繁，请稍后再试",
        ));
    }

    if credential.legacy_transport && !state.allow_legacy_token_transport {
        return Err(api_error(
            StatusCode::UNAUTHORIZED,
            "旧版设备令牌传输已停用，请升级桌面客户端",
        ));
    }

    // 旧客户端只允许继续使用服务端已存在的设备，绝不能借兼容路径匿名建号。
    let user = state
        .store
        .get_device_by_token(&credential.token)
        .await
        .map_err(|_| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "设备查询失败，请稍后重试",
            )
        })?
        .ok_or_else(|| {
            api_error_code(
                StatusCode::NOT_FOUND,
                "unknown_device",
                "设备未注册，请重新注册",
            )
        })?;

    if !state.limiter.allow(
        &format!("{}:token:{}", policy.scope, credential.token),
        policy.token_limit,
        policy.window,
    ) {
        return Err(api_error(
            StatusCode::TOO_MANY_REQUESTS,
            "请求过于频繁，请稍后再试",
        ));
    }
    if credential.legacy_transport {
        warn!("来源 {} 使用旧版设备令牌传输；请尽快升级桌面客户端", ip);
    }
    Ok(user)
}

#[tokio::main]
async fn main() {
    // 优先加载本地 .env 文件中的环境变量配置
    dotenvy::dotenv().ok();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let app_id = match env::var("FEISHU_APP_ID") {
        Ok(val) if !val.trim().is_empty() => val,
        _ => {
            error!("❌ 未配置 FEISHU_APP_ID 环境变量！请在 .env 或容器环境变量中配置。");
            std::process::exit(1);
        }
    };
    let app_secret = match env::var("FEISHU_APP_SECRET") {
        Ok(val) if !val.trim().is_empty() => val,
        _ => {
            error!("❌ 未配置 FEISHU_APP_SECRET 环境变量！请在 .env 或容器环境变量中配置。");
            std::process::exit(1);
        }
    };
    let feishu_verification_token = match env::var("FEISHU_VERIFICATION_TOKEN") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => {
            error!("❌ 未配置 FEISHU_VERIFICATION_TOKEN；拒绝启动未校验回调的服务。");
            std::process::exit(1);
        }
    };
    let data_dir = env::var("DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("./data"));
    let port: u16 = env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3005);
    let allow_legacy_token_transport = env::var("ALLOW_LEGACY_TOKEN_TRANSPORT")
        .map(|value| {
            !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no"
            )
        })
        .unwrap_or(true);

    info!(
        "🚀 正在启动 bili-plan-server (端口: {}, App ID: {})",
        port, app_id
    );

    let store = Store::new(&data_dir);
    let feishu = FeishuClient::new(app_id, app_secret);

    // 检查并初始化 Telegram 机器人
    let tg_token_opt = env::var("TELEGRAM_BOT_TOKEN")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let telegram_client = if let Some(tg_token) = tg_token_opt {
        info!("🤖 已检测到 TELEGRAM_BOT_TOKEN，正在启动 Telegram 机器人...");
        let tg_client = TelegramClient::new(tg_token);
        telegram::start_telegram_polling(store.clone(), tg_client.clone());
        Some(tg_client)
    } else {
        info!("ℹ️ 未配置 TELEGRAM_BOT_TOKEN，Telegram 机器人未启用。");
        None
    };

    // 启动后台定时推送调度器
    scheduler::start_scheduler(store.clone(), feishu.clone(), telegram_client);

    let state = AppState {
        store,
        feishu,
        feishu_verification_token,
        limiter: RateLimiter::new(),
        allow_legacy_token_transport,
    };

    let app = Router::new()
        .route("/api/health", get(health_check))
        .route("/api/device/register", post(register_device))
        .route("/api/bind/request", post(request_bind_code))
        .route("/api/bind/status", get(query_bind_status))
        .route("/api/sync", post(sync_plans))
        .route("/api/feishu/callback", post(feishu_callback))
        // 只记录 path，不记录 query；旧客户端迁移期的 query 可能含设备令牌。
        .layer(
            TraceLayer::new_for_http().make_span_with(|request: &axum::http::Request<_>| {
                info_span!(
                    "http_request",
                    method = %request.method(),
                    path = %request.uri().path()
                )
            }),
        )
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    info!("✅ 服务已就绪，监听于 http://{}", addr);

    // 限流需要真实对端地址，因此必须启用 connect info。
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .unwrap();
}

async fn health_check() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "app": "bili-plan-server",
        "time": Local::now().to_rfc3339()
    }))
}

/// 签发新设备令牌。客户端首次同步前调用一次，之后持久化复用。
async fn register_device(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Result<Json<RegisterResponse>, ApiError> {
    let ip = auth::client_ip(&headers, Some(&peer));
    if !state
        .limiter
        .allow(&format!("register:ip:{ip}"), REGISTER_PER_IP, HOUR)
    {
        return Err(api_error(
            StatusCode::TOO_MANY_REQUESTS,
            "注册过于频繁，请稍后再试",
        ));
    }
    let user = state
        .store
        .register_device()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    // 令牌是不记名密钥，不能落进日志；只记来源 IP 供排查滥用。
    info!("已为 {} 注册新设备", ip);
    Ok(Json(RegisterResponse {
        device_token: user.device_token,
    }))
}

/// 客户端请求 6 位绑定码
#[derive(Deserialize, Default)]
struct LegacyBindRequest {
    #[serde(default)]
    device_token: Option<String>,
}

async fn request_bind_code(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    payload: Option<Json<LegacyBindRequest>>,
) -> Result<Json<BindRequestResponse>, ApiError> {
    let user = authorize_device(
        &state,
        &headers,
        &peer,
        RatePolicy {
            scope: "bind_request",
            token_limit: BIND_REQUEST_PER_TOKEN,
            ip_limit: BIND_REQUEST_PER_IP,
            window: HOUR,
        },
        payload
            .as_ref()
            .and_then(|Json(payload)| payload.device_token.as_deref()),
    )
    .await?;
    let code = state
        .store
        .generate_bind_code(&user.device_token)
        .await
        .ok_or_else(|| api_error(StatusCode::INTERNAL_SERVER_ERROR, "生成绑定码失败"))?;

    Ok(Json(BindRequestResponse {
        bind_code: code,
        device_token: user.device_token,
        expires_in_secs: 600,
    }))
}

/// 客户端查询当前设备是否已被飞书或 Telegram 绑定
#[derive(Deserialize, Default)]
struct LegacyBindStatusQuery {
    #[serde(default)]
    device_token: Option<String>,
}

async fn query_bind_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Query(query): Query<LegacyBindStatusQuery>,
) -> Result<Json<BindStatusResponse>, ApiError> {
    let user = authorize_device(
        &state,
        &headers,
        &peer,
        RatePolicy {
            scope: "bind_status",
            token_limit: BIND_STATUS_PER_TOKEN,
            ip_limit: BIND_STATUS_PER_IP,
            window: MINUTE,
        },
        query.device_token.as_deref(),
    )
    .await?;
    let feishu_bound = user.feishu_open_id.is_some();
    let telegram_bound = user.telegram_chat_id.is_some();
    Ok(Json(BindStatusResponse {
        bound: feishu_bound || telegram_bound,
        feishu_bound,
        feishu_user_name: user.feishu_user_name,
        telegram_bound,
        telegram_user_name: user.telegram_user_name,
    }))
}

/// 双向同步接口
async fn sync_plans(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(payload): Json<SyncPayload>,
) -> Result<Json<SyncResponse>, ApiError> {
    let legacy_token = payload.device_token.as_deref();
    let user = authorize_device(
        &state,
        &headers,
        &peer,
        RatePolicy {
            scope: "sync",
            token_limit: SYNC_PER_TOKEN,
            ip_limit: SYNC_PER_IP,
            window: MINUTE,
        },
        legacy_token,
    )
    .await?;

    match state
        .store
        .sync_plans_versioned(
            &user.device_token,
            payload.plans,
            payload.daily_notes,
            payload.base_revision,
        )
        .await
    {
        Ok(outcome) => Ok(Json(SyncResponse {
            success: true,
            revision: outcome.revision,
            plans: outcome.plans,
            daily_notes: outcome.daily_notes,
            feishu_bound: outcome.feishu_bound,
            feishu_user_name: outcome.feishu_user_name,
            telegram_bound: outcome.telegram_bound,
            telegram_user_name: outcome.telegram_user_name,
            message: "同步成功".to_string(),
        })),
        Err(SyncError::UnknownDevice) => Err(api_error_code(
            StatusCode::NOT_FOUND,
            "unknown_device",
            "设备未注册，请重新注册",
        )),
        Err(SyncError::StaleSnapshot) => Err(api_error_code(
            StatusCode::CONFLICT,
            "stale_snapshot",
            "计划快照已过期；请升级客户端并在原设备核对数据后重试",
        )),
        Err(SyncError::Storage) => {
            // 同样不落令牌明文，只记来源 IP。
            error!(
                "同步时存储失败（来源 {}）",
                auth::client_ip(&headers, Some(&peer))
            );
            Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "同步失败，请稍后重试",
            ))
        }
    }
}

/// 飞书事件订阅与卡片回调总入口
async fn feishu_callback(
    State(state): State<AppState>,
    Json(req): Json<FeishuCallbackRequest>,
) -> impl IntoResponse {
    // 飞书事件与卡片回调必须携带在开放平台配置的 Verification Token。
    // 未校验的公开回调会允许伪造打卡或绑定消息，因此在处理任何内容前拒绝它。
    let received_token = req.token.as_deref().or_else(|| {
        req.header
            .as_ref()
            .and_then(|header| header.token.as_deref())
    });
    if received_token != Some(state.feishu_verification_token.as_str()) {
        error!("拒绝缺少或无效 Verification Token 的飞书回调");
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "code": 401, "msg": "invalid verification token" })),
        )
            .into_response();
    }

    // 1. 飞书首次配置事件回调时的 URL 校验挑战 (Challenge)
    if let Some(challenge) = req.challenge {
        info!("响应飞书 URL 校验 Challenge: {}", challenge);
        return Json(json!({ "challenge": challenge })).into_response();
    }
    if let Some(t) = &req.r#type {
        if t == "url_verification" {
            if let Some(c) = req.challenge {
                return Json(json!({ "challenge": c })).into_response();
            }
        }
    }

    // 2. 飞书消息卡片交互回调 (兼容 v1 顶层 action 与 v2 event.action)
    let card_action_info: Option<(String, CardActionData)> = if let Some(action) = &req.action {
        let open_id = req.open_id.clone().unwrap_or_default();
        let act_val: Result<CardActionData, _> = if action.value.is_string() {
            serde_json::from_str(action.value.as_str().unwrap_or_default())
        } else {
            serde_json::from_value(action.value.clone())
        };
        act_val.ok().map(|data| (open_id, data))
    } else if let Some(event) = &req.event {
        if let Some(action_val) = event.get("action") {
            let open_id = event
                .get("operator")
                .and_then(|op| op.get("open_id"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let raw_val = action_val.get("value").unwrap_or(action_val);
            let act_val: Result<CardActionData, _> = if raw_val.is_string() {
                serde_json::from_str(raw_val.as_str().unwrap_or_default())
            } else {
                serde_json::from_value(raw_val.clone())
            };
            act_val.ok().map(|data| (open_id, data))
        } else {
            None
        }
    } else {
        None
    };

    if let Some((open_id, act_data)) = card_action_info {
        info!("🎯 收到飞书卡片操作 (用户: {}): {:?}", open_id, act_data);
        if act_data.action == "checkin" {
            if let (Some(pid), Some(tid)) = (act_data.plan_id, act_data.task_id) {
                match state
                    .store
                    .toggle_task_by_open_id(&open_id, &pid, &tid)
                    .await
                {
                    Ok(is_done) => {
                        let msg = if is_done {
                            "已完成打卡！保持专注 🔥"
                        } else {
                            "已撤销该项打卡"
                        };
                        let target_date = act_data
                            .date
                            .unwrap_or_else(|| Local::now().format("%Y-%m-%d").to_string());
                        let (_, plans) = state
                            .store
                            .get_plans_by_open_id(&open_id)
                            .await
                            .unwrap_or_default();
                        let updated_card = build_today_study_card(&plans, &target_date);
                        info!(
                            "✅ 打卡状态更新成功 (is_done={}), 正在向飞书返回更新后的卡片",
                            is_done
                        );
                        let is_v2 = req.event.is_some() || req.schema.as_deref() == Some("2.0");
                        let card_val = if is_v2 {
                            json!({
                                "type": "raw",
                                "data": updated_card
                            })
                        } else {
                            updated_card
                        };

                        return Json(json!({
                            "toast": {
                                "type": "success",
                                "content": msg
                            },
                            "card": card_val
                        }))
                        .into_response();
                    }
                    Err(e) => {
                        error!("❌ 打卡处理失败: {}", e);
                        return Json(json!({
                            "toast": {
                                "type": "error",
                                "content": format!("打卡失败: {}", e)
                            }
                        }))
                        .into_response();
                    }
                }
            }
        }
        return Json(json!({ "toast": { "type": "info", "content": "已处理" } })).into_response();
    }

    // 3. 飞书用户文字消息事件 (im.message.receive_v1)
    if let Some(event) = req.event {
        if let Some(msg) = event.get("message") {
            let open_id = event["sender"]["sender_id"]["open_id"]
                .as_str()
                .unwrap_or_default();
            let user_name = event["sender"]["sender_id"]["user_id"]
                .as_str()
                .unwrap_or("学习者");
            let content_str = msg["content"].as_str().unwrap_or_default();

            // 解析飞书文本内容 JSON 格式: {"text":"/bind 123456"}
            let text = serde_json::from_str::<Value>(content_str)
                .ok()
                .and_then(|v| v["text"].as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| content_str.to_string())
                .trim()
                .to_string();

            info!(
                "收到飞书用户 [{}] 消息: {}",
                open_id,
                auth::redact_bind_code(&text)
            );

            if text.starts_with("/bind") {
                let parts: Vec<&str> = text.split_whitespace().collect();
                if parts.len() >= 2 {
                    let code = parts[1].trim();
                    match state
                        .store
                        .bind_by_code(code, open_id, Some(user_name))
                        .await
                    {
                        Ok(_) => {
                            let reply = "🎉 **绑定成功！**\n\n已成功与您的电脑端 bili-planner 建立连接。\n• 每日早 08:30 将为您推送今日任务早报\n• 每日晚 21:30 将提醒您复盘打卡\n• 发送 `/today` 随时呼出今日打卡卡片\n• 发送 `/plans` 随时查看计划库科目总览！".to_string();
                            let _ = state.feishu.send_text_message(open_id, &reply).await;
                        }
                        Err(e) => {
                            // 错误文案已自带下一步指引（重新生成 / 等待锁定结束），不再追加。
                            let reply = format!("❌ 绑定失败: {}", e);
                            let _ = state.feishu.send_text_message(open_id, &reply).await;
                        }
                    }
                } else {
                    let _ = state
                        .feishu
                        .send_text_message(open_id, "请输入正确的格式：`/bind <电脑端6位验证码>`")
                        .await;
                }
            } else if text == "/today" || text == "今天" || text == "打卡" {
                if let Some((_, plans)) = state.store.get_plans_by_open_id(open_id).await {
                    let today_str = Local::now().format("%Y-%m-%d").to_string();
                    let card = build_today_study_card(&plans, &today_str);
                    if let Err(e) = state.feishu.send_card_message(open_id, card).await {
                        error!("发送今日卡片失败: {}", e);
                    }
                } else {
                    let reply = "⚠️ 您尚未绑定任何电脑端 bili-planner 设备。\n请先在电脑端点击「飞书云同步」，获取6位验证码后在此发送 `/bind 验证码` 完成绑定。";
                    let _ = state.feishu.send_text_message(open_id, reply).await;
                }
            } else if text == "/plans"
                || text == "/list"
                || text == "/plan"
                || text == "计划库"
                || text == "我的计划"
                || text == "我的计划库"
                || text == "计划"
            {
                if let Some((_, plans)) = state.store.get_plans_by_open_id(open_id).await {
                    let card = build_my_plans_card(&plans);
                    if let Err(e) = state.feishu.send_card_message(open_id, card).await {
                        error!("发送计划库卡片失败: {}", e);
                    }
                } else {
                    let reply = "⚠️ 您尚未绑定任何电脑端 bili-planner 设备。\n请先在电脑端点击「飞书云同步」，获取6位验证码后在此发送 `/bind 验证码` 完成绑定。";
                    let _ = state.feishu.send_text_message(open_id, reply).await;
                }
            } else if text == "/help" || text == "帮助" {
                let help_text = "📖 **学习打卡助手指令列表**：\n• `/today`：查看今日任务打卡卡片\n• `/plans`：查看我的计划库总览（科目/进度/排期）\n• `/bind <验证码>`：绑定电脑端学习计划\n• `/help`：查看指令帮助";
                let _ = state.feishu.send_text_message(open_id, help_text).await;
            }
        }
    }

    Json(json!({ "code": 0, "msg": "success" })).into_response()
}
