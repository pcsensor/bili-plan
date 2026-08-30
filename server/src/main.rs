mod card;
mod feishu;
mod models;
mod scheduler;
mod store;
mod telegram;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use card::{build_my_plans_card, build_today_study_card};
use chrono::Local;
use feishu::FeishuClient;
use models::{
    BindRequestResponse, BindStatusResponse, CardActionData, FeishuCallbackRequest,
    SyncPayload, SyncResponse,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use store::Store;
use telegram::TelegramClient;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Clone)]
struct AppState {
    store: Store,
    feishu: FeishuClient,
}

#[tokio::main]
async fn main() {
    // 优先加载本地 .env 文件中的环境变量配置
    dotenvy::dotenv().ok();

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
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
    let data_dir = env::var("DATA_DIR").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("./data"));
    let port: u16 = env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(3005);

    info!("🚀 正在启动 bili-plan-server (端口: {}, App ID: {})", port, app_id);

    let store = Store::new(&data_dir);
    let feishu = FeishuClient::new(app_id, app_secret);

    // 检查并初始化 Telegram 机器人
    let tg_token_opt = env::var("TELEGRAM_BOT_TOKEN").ok().filter(|s| !s.trim().is_empty());
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
    };

    let app = Router::new()
        .route("/api/health", get(health_check))
        .route("/api/bind/request", post(request_bind_code))
        .route("/api/bind/status", get(query_bind_status))
        .route("/api/sync", post(sync_plans))
        .route("/api/feishu/callback", post(feishu_callback))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    info!("✅ 服务已就绪，监听于 http://{}", addr);

    axum::serve(listener, app).await.unwrap();
}

async fn health_check() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "app": "bili-plan-server",
        "time": Local::now().to_rfc3339()
    }))
}

#[derive(Deserialize)]
struct BindReq {
    device_token: Option<String>,
}

/// 客户端请求 6 位绑定码
async fn request_bind_code(
    State(state): State<AppState>,
    Json(payload): Json<BindReq>,
) -> Result<Json<BindRequestResponse>, (StatusCode, String)> {
    let user = state.store.get_or_create_device(payload.device_token.as_deref()).await;
    let code = state
        .store
        .generate_bind_code(&user.device_token)
        .await
        .ok_or_else(|| (StatusCode::INTERNAL_SERVER_ERROR, "生成绑定码失败".to_string()))?;

    Ok(Json(BindRequestResponse {
        bind_code: code,
        device_token: user.device_token,
        expires_in_secs: 600,
    }))
}

#[derive(Deserialize)]
struct BindStatusQuery {
    device_token: String,
}

/// 客户端查询当前设备是否已被飞书或 Telegram 绑定
async fn query_bind_status(
    State(state): State<AppState>,
    Query(query): Query<BindStatusQuery>,
) -> Json<BindStatusResponse> {
    let user = state.store.get_or_create_device(Some(&query.device_token)).await;
    let feishu_bound = user.feishu_open_id.is_some();
    let telegram_bound = user.telegram_chat_id.is_some();
    Json(BindStatusResponse {
        bound: feishu_bound || telegram_bound,
        feishu_bound,
        feishu_user_name: user.feishu_user_name,
        telegram_bound,
        telegram_user_name: user.telegram_user_name,
    })
}

/// 双向同步接口
async fn sync_plans(
    State(state): State<AppState>,
    Json(payload): Json<SyncPayload>,
) -> Json<SyncResponse> {
    let (plans, feishu_bound, feishu_user_name, telegram_bound, telegram_user_name) = state
        .store
        .sync_plans(&payload.device_token, payload.plans)
        .await;

    Json(SyncResponse {
        success: true,
        plans,
        feishu_bound,
        feishu_user_name,
        telegram_bound,
        telegram_user_name,
        message: "同步成功".to_string(),
    })
}

/// 飞书事件订阅与卡片回调总入口
async fn feishu_callback(
    State(state): State<AppState>,
    Json(req): Json<FeishuCallbackRequest>,
) -> impl IntoResponse {
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
                match state.store.toggle_task_by_open_id(&open_id, &pid, &tid).await {
                    Ok(is_done) => {
                        let msg = if is_done { "已完成打卡！保持专注 🔥" } else { "已撤销该项打卡" };
                        let target_date = act_data.date.unwrap_or_else(|| Local::now().format("%Y-%m-%d").to_string());
                        let (_, plans) = state.store.get_plans_by_open_id(&open_id).await.unwrap_or_default();
                        let updated_card = build_today_study_card(&plans, &target_date);
                        info!("✅ 打卡状态更新成功 (is_done={}), 正在向飞书返回更新后的卡片", is_done);
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
                        })).into_response();
                    }
                    Err(e) => {
                        error!("❌ 打卡处理失败: {}", e);
                        return Json(json!({
                            "toast": {
                                "type": "error",
                                "content": format!("打卡失败: {}", e)
                            }
                        })).into_response();
                    }
                }
            }
        }
        return Json(json!({ "toast": { "type": "info", "content": "已处理" } })).into_response();
    }

    // 3. 飞书用户文字消息事件 (im.message.receive_v1)
    if let Some(event) = req.event {
        if let Some(msg) = event.get("message") {
            let open_id = event["sender"]["sender_id"]["open_id"].as_str().unwrap_or_default();
            let user_name = event["sender"]["sender_id"]["user_id"].as_str().unwrap_or("学习者");
            let content_str = msg["content"].as_str().unwrap_or_default();

            // 解析飞书文本内容 JSON 格式: {"text":"/bind 123456"}
            let text = serde_json::from_str::<Value>(content_str)
                .ok()
                .and_then(|v| v["text"].as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| content_str.to_string())
                .trim()
                .to_string();

            info!("收到飞书用户 [{}] 消息: {}", open_id, text);

            if text.starts_with("/bind") {
                let parts: Vec<&str> = text.split_whitespace().collect();
                if parts.len() >= 2 {
                    let code = parts[1].trim();
                    match state.store.bind_by_code(code, open_id, Some(user_name)).await {
                        Ok(_) => {
                            let reply = format!(
                                "🎉 **绑定成功！**\n\n已成功与您的电脑端 bili-planner 建立连接。\n• 每日早 08:30 将为您推送今日任务早报\n• 每日晚 21:30 将提醒您复盘打卡\n• 发送 `/today` 随时呼出今日打卡卡片\n• 发送 `/plans` 随时查看计划库科目总览！"
                            );
                            let _ = state.feishu.send_text_message(open_id, &reply).await;
                        }
                        Err(e) => {
                            let reply = format!("❌ 绑定失败: {}\n请在电脑端重新生成绑定码后再试。", e);
                            let _ = state.feishu.send_text_message(open_id, &reply).await;
                        }
                    }
                } else {
                    let _ = state.feishu.send_text_message(open_id, "请输入正确的格式：`/bind <电脑端6位验证码>`").await;
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
            } else if text == "/plans" || text == "/list" || text == "/plan" || text == "计划库" || text == "我的计划" || text == "我的计划库" || text == "计划" {
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
