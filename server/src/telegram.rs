use crate::models::StudyPlan;
use crate::store::Store;
use chrono::Local;
use planner_domain::source::video_link;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;
use tracing::{info, warn};

#[derive(Deserialize)]
struct ApiResponse<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
}

#[derive(Deserialize)]
struct SentMessage {
    message_id: i64,
}

#[derive(Deserialize)]
pub struct TelegramUpdate {
    update_id: i64,
    #[serde(default)]
    message: Option<TelegramMessage>,
    #[serde(default)]
    callback_query: Option<TelegramCallback>,
}

#[derive(Deserialize)]
struct TelegramMessage {
    message_id: Option<i64>,
    chat: TelegramChat,
    text: Option<String>,
    from: Option<TelegramUser>,
}

#[derive(Deserialize)]
struct TelegramChat {
    id: i64,
}

#[derive(Deserialize)]
struct TelegramUser {
    username: Option<String>,
    first_name: Option<String>,
}

#[derive(Deserialize)]
struct TelegramCallback {
    id: String,
    message: Option<TelegramMessage>,
    data: Option<String>,
}

#[derive(Clone)]
pub struct TelegramClient {
    bot_token: String,
    http: Client,
}

impl TelegramClient {
    pub fn new(bot_token: String) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(35))
            .build()
            .unwrap_or_default();
        Self { bot_token, http }
    }

    /// 发送文本消息（支持 HTML 格式与 Inline Keyboard）。
    pub async fn send_message(
        &self,
        chat_id: i64,
        text: &str,
        reply_markup: Option<Value>,
    ) -> Result<i64, String> {
        let url = format!("https://api.telegram.org/bot{}/sendMessage", self.bot_token);
        let mut body = json!({
            "chat_id": chat_id,
            "text": text,
            "parse_mode": "HTML",
            "disable_web_page_preview": true
        });

        if let Some(markup) = reply_markup {
            body["reply_markup"] = markup;
        }

        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Telegram 发送失败: {}", e))?;

        let res_json: ApiResponse<SentMessage> = resp
            .json()
            .await
            .map_err(|e| format!("解析 Telegram 响应失败: {}", e))?;

        if res_json.ok {
            res_json
                .result
                .map(|message| message.message_id)
                .ok_or_else(|| "Telegram 成功响应缺少 message_id".to_string())
        } else {
            Err(format!(
                "Telegram API 错误: {}",
                res_json.description.as_deref().unwrap_or("未知错误")
            ))
        }
    }

    /// 编辑已有消息（用于交互卡片原地更新）。
    pub async fn edit_message_text(
        &self,
        chat_id: i64,
        message_id: i64,
        text: &str,
        reply_markup: Option<Value>,
    ) -> Result<(), String> {
        let url = format!(
            "https://api.telegram.org/bot{}/editMessageText",
            self.bot_token
        );
        let mut body = json!({
            "chat_id": chat_id,
            "message_id": message_id,
            "text": text,
            "parse_mode": "HTML",
            "disable_web_page_preview": true
        });

        if let Some(markup) = reply_markup {
            body["reply_markup"] = markup;
        }

        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Telegram 编辑失败: {}", e))?;

        let res_json: ApiResponse<serde::de::IgnoredAny> = resp
            .json()
            .await
            .map_err(|e| format!("解析 Telegram 响应失败: {}", e))?;

        if res_json.ok {
            Ok(())
        } else {
            let desc = res_json.description.as_deref().unwrap_or("");
            if desc.contains("message is not modified") {
                Ok(())
            } else {
                Err(format!("Telegram API 错误: {}", desc))
            }
        }
    }

    /// 回复 Callback Query（弹出气泡提示）。
    pub async fn answer_callback_query(
        &self,
        query_id: &str,
        text: Option<&str>,
        show_alert: bool,
    ) -> Result<(), String> {
        let url = format!(
            "https://api.telegram.org/bot{}/answerCallbackQuery",
            self.bot_token
        );
        let mut body = json!({
            "callback_query_id": query_id,
            "show_alert": show_alert
        });

        if let Some(t) = text {
            body["text"] = json!(t);
        }

        let _ = self.http.post(&url).json(&body).send().await;
        Ok(())
    }

    /// 长轮询获取 Updates。
    pub async fn get_updates(
        &self,
        offset: i64,
        timeout: u64,
    ) -> Result<Vec<TelegramUpdate>, String> {
        let url = format!("https://api.telegram.org/bot{}/getUpdates", self.bot_token);
        let body = json!({
            "offset": offset,
            "timeout": timeout,
            "allowed_updates": ["message", "callback_query"]
        });

        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Telegram 轮询网络错误: {}", e))?;

        let res_json: ApiResponse<Vec<TelegramUpdate>> = resp
            .json()
            .await
            .map_err(|e| format!("解析 Telegram Updates 失败: {}", e))?;

        if res_json.ok {
            res_json
                .result
                .ok_or_else(|| "Telegram 成功响应缺少 Updates 列表".to_string())
        } else {
            Err(format!(
                "Telegram API 轮询错误: {}",
                res_json.description.as_deref().unwrap_or("未知")
            ))
        }
    }

    /// 发送今日学习打卡卡片。
    pub async fn send_today_study_card(
        &self,
        chat_id: i64,
        plans: &[StudyPlan],
        target_date: &str,
    ) -> Result<i64, String> {
        let (text, markup) = build_telegram_today_card(plans, target_date);
        self.send_message(chat_id, &text, markup).await
    }
}

/// 格式化秒数为时分秒。
fn fmt_dur(secs: i64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h}小时{m}分")
    } else if m > 0 {
        format!("{m}分{s}秒")
    } else {
        format!("{s}秒")
    }
}

/// 格式化并清理 Bilibili 链接。
/// HTML 字符转义，防止特殊字符（如 <, >, &, "）导致 Telegram API 解析错误
pub fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// 格式化进度条字符。
pub fn make_block_bar(pct: f64, length: usize) -> String {
    let clamped = pct.clamp(0.0, 100.0);
    let filled = ((clamped / 100.0) * (length as f64)).round() as usize;
    let filled = filled.min(length);
    let unfilled = length.saturating_sub(filled);
    format!("{}{}", "█".repeat(filled), "░".repeat(unfilled))
}

/// 构建 Telegram 今日打卡消息与 Inline Keyboard。
pub fn build_telegram_today_card(
    plans: &[StudyPlan],
    target_date: &str,
) -> (String, Option<Value>) {
    let mut total_tasks = 0;
    let mut completed_tasks = 0;
    let mut total_duration = 0;
    let mut completed_duration = 0;

    let mut task_sections = Vec::new();
    let mut keyboard_rows = Vec::new();

    for plan in plans {
        if plan.status != crate::models::PlanStatus::Active {
            continue;
        }
        if let Some(sch) = plan.schedules.iter().find(|s| s.date == target_date) {
            if !sch.tasks.is_empty() {
                let mut lines = vec![format!("📚 <b>《{}》</b>", escape_html(&plan.title))];
                for task in &sch.tasks {
                    total_tasks += 1;
                    total_duration += task.portion;
                    if task.completed {
                        completed_tasks += 1;
                        completed_duration += task.portion;
                    }

                    let dur_str = fmt_dur(task.portion);
                    let safe_title = escape_html(&task.title);
                    let (status_icon, title_text) = if task.completed {
                        ("✅", format!("<s>{} (已学 {})</s>", safe_title, dur_str))
                    } else {
                        ("⬜", format!("<b>{}</b> (⏱️ {})", safe_title, dur_str))
                    };

                    lines.push(format!(
                        "  {} (P{}) {}",
                        status_icon, task.vid_no, title_text
                    ));

                    // Inline Keyboard 打卡按钮与直达链接
                    let btn_text = if task.completed {
                        format!("✅ 已完成 P{}", task.vid_no)
                    } else {
                        format!("⬜ 打卡 P{}", task.vid_no)
                    };
                    let callback_data = format!("chk:{}:{}:{}", plan.id, task.id, target_date);
                    let direct_url = video_link(&plan.source_type, &plan.source_url, task.vid_no);

                    let mut row = vec![json!({
                        "text": btn_text,
                        "callback_data": callback_data
                    })];

                    if direct_url.starts_with("http://") || direct_url.starts_with("https://") {
                        row.push(json!({
                            "text": "🔗 直达",
                            "url": direct_url
                        }));
                    }

                    keyboard_rows.push(row);
                }
                task_sections.push(lines.join("\n"));
            }
        }
    }

    let progress_pct = if total_tasks > 0 {
        (completed_tasks as f64 / total_tasks as f64) * 100.0
    } else {
        0.0
    };

    let summary_header = if total_tasks == 0 {
        format!(
            "📅 <b>学习计划打卡</b> · <code>{}</code>\n━━━━━━━━━━━━━━━━━━\n\n☕ <b>今日暂无学习安排</b>\n可自由复习、预习或休息！\n发送 <code>/plans</code> 查看全部科目规划进度。",
            escape_html(target_date)
        )
    } else {
        let bar = make_block_bar(progress_pct, 16);
        format!(
            "📅 <b>今日学习任务早报</b> · <code>{}</code>\n━━━━━━━━━━━━━━━━━━\n🎯 <b>今日进度：</b>{}/{} 项 ({:.0}%)\n⏱️ <b>学习时长：</b>已学 {} / 规划 {}\n📊 <b>完成进度：</b><code>[{}] {:.0}%</code>\n\n{}\n━━━━━━━━━━━━━━━━━━\n👇 <i>点击下方按钮可直接在 Telegram 内打卡或直达播放：</i>",
            escape_html(target_date),
            completed_tasks,
            total_tasks,
            progress_pct,
            fmt_dur(completed_duration),
            fmt_dur(total_duration),
            bar,
            progress_pct,
            task_sections.join("\n\n")
        )
    };

    let reply_markup = if keyboard_rows.is_empty() {
        None
    } else {
        Some(json!({
            "inline_keyboard": keyboard_rows
        }))
    };

    (summary_header, reply_markup)
}

/// 构建 Telegram 全部计划进度消息。
pub fn build_telegram_plans_card(plans: &[StudyPlan]) -> String {
    if plans.is_empty() {
        return "📚 <b>我的学习计划库</b>\n━━━━━━━━━━━━━━━━━━\n\n⚠️ <i>当前计划库为空</i>\n请在电脑端 bili-planner 添加学习计划并点击「云端同步」。".to_string();
    }

    let mut lines = vec![
        "📚 <b>我的学习计划库</b>".to_string(),
        "━━━━━━━━━━━━━━━━━━".to_string(),
    ];

    for (i, p) in plans.iter().enumerate() {
        let total_t = p.schedules.iter().map(|s| s.tasks.len()).sum::<usize>();
        let done_t = p
            .schedules
            .iter()
            .flat_map(|s| &s.tasks)
            .filter(|t| t.completed)
            .count();
        let status_str = match p.status {
            crate::models::PlanStatus::Active => "🟢 进行中",
            crate::models::PlanStatus::Paused => "⏸️ 已暂停",
            crate::models::PlanStatus::Completed => "🎉 已结课",
            crate::models::PlanStatus::Archived => "📦 已归档",
        };
        let pct = if total_t > 0 {
            (done_t as f64 / total_t as f64) * 100.0
        } else {
            0.0
        };
        let bar = make_block_bar(pct, 14);

        lines.push(format!(
            "{}. <b>《{}》</b> [{}]\n   • <b>任务进度：</b>{}/{} 讲 ({:.0}%)\n   • <b>进度条：</b><code>[{}] {:.0}%</code>\n   • <b>排期规划：</b>共 {} 天 ({} 至 {})\n   • <b>总计时长：</b>{}",
            i + 1,
            escape_html(&p.title),
            status_str,
            done_t,
            total_t,
            pct,
            bar,
            pct,
            p.planned_days,
            p.start_date,
            p.end_date,
            fmt_dur(p.total_duration)
        ));
    }

    lines.push("━━━━━━━━━━━━━━━━━━\n💡 <i>提示：发送 <code>/today</code> 呼出今日任务卡片并在 TG 内一键打卡！</i>".to_string());
    lines.join("\n\n")
}

/// 启动 Telegram 机器人后台长轮询。
pub fn start_telegram_polling(store: Store, telegram: TelegramClient) {
    tokio::spawn(async move {
        info!("🤖 Telegram 机器人长轮询服务已启动");
        let mut offset: i64 = 0;

        loop {
            match telegram.get_updates(offset, 25).await {
                Ok(updates) => {
                    for u in updates {
                        if u.update_id >= offset {
                            offset = u.update_id + 1;
                        }

                        // 1. 处理用户发送的消息
                        if let Some(msg) = &u.message {
                            let chat_id = msg.chat.id;
                            let text = msg.text.as_deref().unwrap_or("").trim();
                            let user_name = msg
                                .from
                                .as_ref()
                                .and_then(|user| {
                                    user.username.as_deref().or(user.first_name.as_deref())
                                })
                                .unwrap_or("学习者");

                            let first_word =
                                text.split_whitespace().next().unwrap_or("").to_lowercase();

                            if first_word == "/bind" || first_word.starts_with("/bind@") {
                                let parts: Vec<&str> = text.split_whitespace().collect();
                                if parts.len() < 2 {
                                    let _ = telegram
                                        .send_message(
                                            chat_id,
                                            "⚠️ 请提供 6 位绑定验证码，例如：<code>/bind 123456</code>",
                                            None,
                                        )
                                        .await;
                                } else {
                                    let code = parts[1].trim();
                                    match store
                                        .bind_telegram_by_code(code, chat_id, Some(user_name))
                                        .await
                                    {
                                        Ok(_) => {
                                            let reply = "🎉 <b>绑定成功！</b>\n━━━━━━━━━━━━━━━━━━\n已与您的电脑端 <b>bili-planner</b> 建立双向连接。\n\n• 每日 <b>08:30</b> 自动推送今日学习早报\n• 每日 <b>21:30</b> 自动提醒晚间复盘\n• 发送 <code>/today</code> 随时呼出今日任务卡片并在 TG 内一键打卡\n• 发送 <code>/plans</code> 查看所有科目总体进度".to_string();
                                            let _ =
                                                telegram.send_message(chat_id, &reply, None).await;
                                        }
                                        Err(e) => {
                                            // 错误文案已自带下一步指引（重新生成 / 等待锁定结束），不再追加。
                                            let reply = format!("❌ 绑定失败：{}", escape_html(&e));
                                            let _ =
                                                telegram.send_message(chat_id, &reply, None).await;
                                        }
                                    }
                                }
                            } else if first_word == "/today"
                                || first_word.starts_with("/today@")
                                || first_word == "/start"
                                || first_word.starts_with("/start@")
                                || text == "今天"
                                || text == "打卡"
                            {
                                if let Some((_, plans)) =
                                    store.get_plans_by_telegram_chat_id(chat_id).await
                                {
                                    let today = Local::now().format("%Y-%m-%d").to_string();
                                    let (card_text, markup) =
                                        build_telegram_today_card(&plans, &today);
                                    if let Err(e) =
                                        telegram.send_message(chat_id, &card_text, markup).await
                                    {
                                        warn!("发送今日打卡卡片失败: {}", e);
                                    }
                                } else {
                                    let reply = "⚠️ 您尚未绑定设备！\n请在电脑端 bili-planner 点击「绑定」获取 6 位验证码，并发送 <code>/bind 验证码</code> 进行连接。";
                                    let _ = telegram.send_message(chat_id, reply, None).await;
                                }
                            } else if first_word == "/plans"
                                || first_word.starts_with("/plans@")
                                || text == "计划库"
                                || text == "进度"
                            {
                                if let Some((_, plans)) =
                                    store.get_plans_by_telegram_chat_id(chat_id).await
                                {
                                    let reply = build_telegram_plans_card(&plans);
                                    if let Err(e) =
                                        telegram.send_message(chat_id, &reply, None).await
                                    {
                                        warn!("发送计划库卡片失败: {}", e);
                                    }
                                } else {
                                    let reply = "⚠️ 您尚未绑定设备！\n请在电脑端 bili-planner 点击「绑定」获取 6 位验证码，并发送 <code>/bind 验证码</code> 进行连接。";
                                    let _ = telegram.send_message(chat_id, reply, None).await;
                                }
                            } else if first_word == "/help"
                                || first_word.starts_with("/help@")
                                || text == "帮助"
                            {
                                let help_text = "📖 <b>bili-planner Telegram 助手指令指南</b>\n━━━━━━━━━━━━━━━━━━\n\n• <code>/bind &lt;验证码&gt;</code> - 绑定电脑端应用\n• <code>/today</code> - 查看今日任务并直接打卡\n• <code>/plans</code> - 查看全部学习计划与科目进度\n• <code>/help</code> - 显示帮助菜单";
                                let _ = telegram.send_message(chat_id, help_text, None).await;
                            }
                        }

                        // 2. 处理 Inline Keyboard 点击回调
                        if let Some(cb) = &u.callback_query {
                            let (Some(message), Some(data)) = (&cb.message, cb.data.as_deref())
                            else {
                                continue;
                            };
                            let Some(msg_id) = message.message_id else {
                                continue;
                            };
                            let query_id = cb.id.as_str();
                            let chat_id = message.chat.id;

                            if data.starts_with("chk:") {
                                let parts: Vec<&str> = data.split(':').collect();
                                if parts.len() >= 4 {
                                    let plan_id = parts[1];
                                    let task_id = parts[2];
                                    let target_date = parts[3];

                                    match store
                                        .toggle_task_by_telegram_chat_id(chat_id, plan_id, task_id)
                                        .await
                                    {
                                        Ok(is_done) => {
                                            let alert_msg = if is_done {
                                                "✅ 打卡成功！保持专注 🔥"
                                            } else {
                                                "已撤销打卡 ↩️"
                                            };
                                            let _ = telegram
                                                .answer_callback_query(
                                                    query_id,
                                                    Some(alert_msg),
                                                    false,
                                                )
                                                .await;

                                            // 重新获取最新计划并原地编辑消息卡片
                                            if let Some((_, plans)) =
                                                store.get_plans_by_telegram_chat_id(chat_id).await
                                            {
                                                let (new_text, new_markup) =
                                                    build_telegram_today_card(&plans, target_date);
                                                let _ = telegram
                                                    .edit_message_text(
                                                        chat_id, msg_id, &new_text, new_markup,
                                                    )
                                                    .await;
                                            }
                                        }
                                        Err(e) => {
                                            let _ = telegram
                                                .answer_callback_query(query_id, Some(e), true)
                                                .await;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("Telegram 轮询异常: {}，将在 5 秒后重试", e);
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{ApiResponse, TelegramUpdate};

    #[test]
    fn update_fixture_parses_messages_and_callbacks() {
        let response: ApiResponse<Vec<TelegramUpdate>> = serde_json::from_str(
            r#"{"ok":true,"result":[{"update_id":7,"message":{"message_id":1,"chat":{"id":42},"text":"/today","from":{"username":"learner"}}},{"update_id":8,"callback_query":{"id":"query","message":{"message_id":2,"chat":{"id":42}},"data":"chk:p:t:2024-03-01"}}]}"#,
        )
        .unwrap();
        let updates = response.result.unwrap();
        assert_eq!(updates[0].message.as_ref().unwrap().chat.id, 42);
        assert_eq!(
            updates[1].callback_query.as_ref().unwrap().data.as_deref(),
            Some("chk:p:t:2024-03-01")
        );
        assert!(serde_json::from_str::<ApiResponse<Vec<TelegramUpdate>>>(
            r#"{"ok":true,"result":[{"message":{"chat":{"id":42}}}]}"#
        )
        .is_err());
    }
}
