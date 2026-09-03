use crate::models::{PlanStatus, StudyPlan, TaskItem};
use chrono::{Datelike, Local, NaiveDate};
use serde_json::{json, Value};

fn fmt_seconds(seconds: f64) -> String {
    let s = seconds.round() as u64;
    let m = s / 60;
    let sec = s % 60;
    if m >= 60 {
        let h = m / 60;
        let rem_m = m % 60;
        format!("{}小时{}分", h, rem_m)
    } else {
        format!("{}分{}秒", m, sec)
    }
}

fn get_weekday_name(d: &NaiveDate) -> &'static str {
    match d.weekday() {
        chrono::Weekday::Mon => "周一",
        chrono::Weekday::Tue => "周二",
        chrono::Weekday::Wed => "周三",
        chrono::Weekday::Thu => "周四",
        chrono::Weekday::Fri => "周五",
        chrono::Weekday::Sat => "周六",
        chrono::Weekday::Sun => "周日",
    }
}

fn clean_bili_link(source_url: &str, vid_no: i64) -> String {
    let s = source_url.trim();
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    for i in 0..n.saturating_sub(11) {
        if chars[i] == 'B'
            && chars[i + 1] == 'V'
            && chars[i + 2..i + 12].iter().all(|c| c.is_ascii_alphanumeric())
        {
            let bvid: String = chars[i..i + 12].iter().collect();
            return format!("https://www.bilibili.com/video/{}?p={}", bvid, vid_no);
        }
    }
    if s.starts_with("http://") || s.starts_with("https://") {
        let base = s.split('?').next().unwrap_or(s);
        format!("{}?p={}", base, vid_no)
    } else {
        format!("https://www.bilibili.com/video/{}?p={}", s, vid_no)
    }
}

/// 构建飞书今日学习打卡交互卡片。
pub fn build_today_study_card(plans: &[StudyPlan], target_date: &str) -> Value {
    let parsed_date = NaiveDate::parse_from_str(target_date, "%Y-%m-%d")
        .unwrap_or_else(|_| Local::now().date_naive());
    let weekday_str = get_weekday_name(&parsed_date);

    let active_plans: Vec<_> = plans.iter().filter(|p| p.status == PlanStatus::Active).collect();

    let mut total_tasks = 0;
    let mut done_tasks = 0;
    let mut total_dur = 0u64;
    let mut done_dur = 0u64;

    // 收集今日所有任务
    struct PlanTasks<'a> {
        plan: &'a StudyPlan,
        day_index: usize,
        tasks: &'a [TaskItem],
        is_rest: bool,
    }

    let mut plan_tasks_list = Vec::new();
    for p in &active_plans {
        if let Some(sch) = p.schedules.iter().find(|s| s.date == target_date) {
            total_tasks += sch.tasks.len();
            for t in &sch.tasks {
                total_dur += t.portion.max(0) as u64;
                if t.completed {
                    done_tasks += 1;
                    done_dur += t.portion.max(0) as u64;
                }
            }
            plan_tasks_list.push(PlanTasks {
                plan: p,
                day_index: sch.day_index,
                tasks: &sch.tasks,
                is_rest: sch.is_rest_day,
            });
        }
    }

    let header_color = if total_tasks > 0 && done_tasks == total_tasks {
        "green"
    } else if done_tasks > 0 {
        "blue"
    } else {
        "orange"
    };

    let title_text = format!("🎓 今日学习打卡 · {} ({})", target_date, weekday_str);

    let mut elements: Vec<Value> = Vec::new();

    // 1. 统计横幅
    let stat_md = if total_tasks == 0 {
        "🎉 **今日没有学习任务安排或为休息日！适当休息，保持状态。**".to_string()
    } else {
        let percent = (done_tasks as f64 / total_tasks as f64) * 100.0;
        format!(
            "📊 **任务进度: {}/{} ({:.0}%)** | ⏱️ **已学: {} / {}**",
            done_tasks,
            total_tasks,
            percent,
            fmt_seconds(done_dur as f64),
            fmt_seconds(total_dur as f64)
        )
    };

    elements.push(json!({
        "tag": "div",
        "text": {
            "tag": "lark_md",
            "content": stat_md
        }
    }));
    elements.push(json!({ "tag": "hr" }));

    // 2. 科目与任务列表
    if plan_tasks_list.is_empty() {
        elements.push(json!({
            "tag": "div",
            "text": {
                "tag": "lark_md",
                "content": "暂无进行中的科目安排，请在电脑端 bili-planner 中设立计划并同步。"
            }
        }));
    } else {
        for pt in plan_tasks_list {
            let p_title = &pt.plan.title;
            let day_idx = pt.day_index + 1;
            let total_days = pt.plan.planned_days;

            if pt.is_rest {
                elements.push(json!({
                    "tag": "div",
                    "text": {
                        "tag": "lark_md",
                        "content": format!("📌 **《{}》** · ☕ 今日休息日", p_title)
                    }
                }));
                continue;
            }

            elements.push(json!({
                "tag": "div",
                "text": {
                    "tag": "lark_md",
                    "content": format!("📌 **《{}》** · 第 {}/{} 天", p_title, day_idx, total_days)
                }
            }));

            for task in pt.tasks {
                let vno = task.vid_no;
                let dur_str = fmt_seconds(task.portion.max(0) as f64);
                let is_done = task.completed;

                // 播放链接构造
                let video_link = if pt.plan.source_type == "bilibili" {
                    clean_bili_link(&pt.plan.source_url, vno)
                } else {
                    pt.plan.source_url.clone()
                };

                let link_suffix = if !video_link.trim().is_empty() {
                    format!(" [🔗直达]({})", video_link)
                } else {
                    String::new()
                };
                let item_md = if is_done {
                    format!("✅ ~~(P{}) {} (已学 {})~~{}", vno, task.title, dur_str, link_suffix)
                } else {
                    format!("⬜ **(P{}) {}** (⏱️ {}){}", vno, task.title, dur_str, link_suffix)
                };

                let btn_text = if is_done { "已打卡" } else { "打卡" };
                let btn_type = if is_done { "default" } else { "primary" };

                elements.push(json!({
                    "tag": "div",
                    "text": {
                        "tag": "lark_md",
                        "content": item_md
                    },
                    "extra": {
                        "tag": "button",
                        "text": {
                            "tag": "plain_text",
                            "content": btn_text
                        },
                        "type": btn_type,
                        "value": {
                            "action": "checkin",
                            "plan_id": pt.plan.id,
                            "task_id": task.id,
                            "date": target_date
                        }
                    }
                }));
            }

            elements.push(json!({ "tag": "hr" }));
        }
    }

    // 3. 底部快捷操作
    elements.push(json!({
        "tag": "note",
        "elements": [
            {
                "tag": "plain_text",
                "content": "💡 提示：点击「打卡」按钮即可随时随地完成今日打卡，电脑端将自动同步。"
            }
        ]
    }));

    json!({
        "config": {
            "wide_screen_mode": true,
            "enable_forward": true
        },
        "header": {
            "title": {
                "tag": "plain_text",
                "content": title_text
            },
            "template": header_color
        },
        "elements": elements
    })
}

/// 构建飞书「我的计划库」信息卡片。
pub fn build_my_plans_card(plans: &[StudyPlan]) -> Value {
    let title_text = format!("📚 我的计划库 · 共 {} 门科目", plans.len());
    let mut elements: Vec<Value> = Vec::new();

    if plans.is_empty() {
        elements.push(json!({
            "tag": "div",
            "text": {
                "tag": "lark_md",
                "content": "📭 **当前计划库为空**\n\n请在电脑端 `bili-planner` 客户端中设立您的学习计划，并点击「云端同步」推送到云端。"
            }
        }));
    } else {
        for (idx, p) in plans.iter().enumerate() {
            let status_badge = match p.status {
                PlanStatus::Active => "🟢 进行中",
                PlanStatus::Completed => "✅ 已完成",
                PlanStatus::Paused => "⏸️ 已暂停",
                PlanStatus::Archived => "📦 已归档",
            };

            let mut total_tasks = 0;
            let mut done_tasks = 0;
            let mut total_dur = 0u64;
            let mut done_dur = 0u64;

            for sch in &p.schedules {
                total_tasks += sch.tasks.len();
                for t in &sch.tasks {
                    total_dur += t.portion.max(0) as u64;
                    if t.completed {
                        done_tasks += 1;
                        done_dur += t.portion.max(0) as u64;
                    }
                }
            }

            let percent = if total_tasks > 0 {
                (done_tasks as f64 / total_tasks as f64) * 100.0
            } else {
                0.0
            };

            let weekend_tag = if p.skip_weekends { " · 跳过周末" } else { "" };
            let source_icon = if p.source_type == "bilibili" { "📺 B站" } else { "🎬 Jellyfin" };

            let plan_md = format!(
                "**{}. 《{}》** · {}\n• 📊 **任务进度**: {}/{} 课 ({:.1}%)\n• ⏱️ **时长进度**: {} / {}\n• 📅 **排期范围**: {} ~ {} (共 {} 天{})\n• 🏷️ **视频来源**: {} | {}",
                idx + 1,
                p.title,
                status_badge,
                done_tasks,
                total_tasks,
                percent,
                fmt_seconds(done_dur as f64),
                fmt_seconds(total_dur as f64),
                p.start_date,
                p.end_date,
                p.planned_days,
                weekend_tag,
                source_icon,
                p.scope_desc,
            );

            elements.push(json!({
                "tag": "div",
                "text": {
                    "tag": "lark_md",
                    "content": plan_md
                }
            }));

            if idx + 1 < plans.len() {
                elements.push(json!({ "tag": "hr" }));
            }
        }
    }

    elements.push(json!({ "tag": "hr" }));
    elements.push(json!({
        "tag": "note",
        "elements": [
            {
                "tag": "plain_text",
                "content": "💡 提示：如需新建计划、调整排期或删除科目，请在电脑端 bili-planner 客户端操作后点击「云端同步」。"
            }
        ]
    }));

    json!({
        "config": {
            "wide_screen_mode": true,
            "enable_forward": true
        },
        "header": {
            "title": {
                "tag": "plain_text",
                "content": title_text
            },
            "template": "indigo"
        },
        "elements": elements
    })
}
