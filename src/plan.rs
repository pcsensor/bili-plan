//! 观看计划算法与格式化，与 Python 脚本逐行对齐。

use crate::parse::{EpisodeItem, Group};
use serde::Serialize;

/// 计划模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// split：按日均精确切分并标注跨天分割点（默认）
    #[default]
    Split,
    /// whole：视频保持完整不拆分
    Whole,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Split => "split",
            Mode::Whole => "whole",
        }
    }

    pub fn from_index(i: usize) -> Self {
        if i == 0 {
            Mode::Split
        } else {
            Mode::Whole
        }
    }

    pub fn index(self) -> usize {
        match self {
            Mode::Split => 0,
            Mode::Whole => 1,
        }
    }
}

/// 计划中的一个观看条目（对应 Python plan[d][k] 字典）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanEntry {
    pub vid_no: i64,
    pub title: String,
    pub portion: i64,
    pub from_prev: bool,
    pub remainder: i64,
    pub cont_day: Option<i64>,
}

/// build_plan 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanOut {
    pub plan: Vec<Vec<PlanEntry>>,
    pub capacities: Vec<i64>,
    pub total: i64,
}

/// 按视频顺序将观看任务分配到每天（与 `build_plan` 对齐）。
pub fn build_plan(items: &[EpisodeItem], days: i64, mode: Mode) -> Result<PlanOut, String> {
    let total: i64 = items.iter().map(|i| i.duration).sum();
    if days <= 0 {
        return Err("目标天数必须为正整数".to_string());
    }
    if total <= 0 {
        return Err("总时长为 0，无法生成计划".to_string());
    }

    let base = total / days;
    let rem = total % days;
    let capacities: Vec<i64> = (0..days)
        .map(|i| base + if i < rem { 1 } else { 0 })
        .collect();

    let mut plan: Vec<Vec<PlanEntry>> = Vec::new();
    let mut idx: usize = 0;
    let mut cur_title = String::new();
    let mut cur_dur: i64 = 0;
    let mut cur_no: i64 = 0;
    let mut from_prev = false;

    for day in 0..days as usize {
        let cap = capacities[day];
        let mut assigned: i64 = 0;
        let mut entries: Vec<PlanEntry> = Vec::new();
        // 整集模式的最后一天承接所有剩余视频：目标天数是“尽量均衡”的目标，
        // 不能以丢弃未排视频为代价。最后一天允许超过目标时长。
        while assigned < cap || (mode == Mode::Whole && day + 1 == days as usize) {
            if cur_dur <= 0 {
                if idx >= items.len() {
                    break;
                }
                cur_title = items[idx].title.clone();
                cur_dur = items[idx].duration;
                cur_no = idx as i64 + 1;
                from_prev = false;
                idx += 1;
            }
            let need = cap - assigned;
            if cur_dur <= need {
                // 整个视频（或上一日拆出的剩余部分）在今日看完
                entries.push(PlanEntry {
                    vid_no: cur_no,
                    title: cur_title.clone(),
                    portion: cur_dur,
                    from_prev,
                    remainder: 0,
                    cont_day: None,
                });
                assigned += cur_dur;
                cur_dur = 0;
                from_prev = false;
            } else {
                if mode == Mode::Whole {
                    // 整集模式优先保持视频完整。若当天尚未安排任何视频，
                    // 允许该视频超过日均目标；否则它会在下一天开始。
                    // 这样不会因为某个视频长于每一天的容量而永远无法排入计划。
                    if entries.is_empty() || day + 1 == days as usize {
                        entries.push(PlanEntry {
                            vid_no: cur_no,
                            title: cur_title.clone(),
                            portion: cur_dur,
                            from_prev,
                            remainder: 0,
                            cont_day: None,
                        });
                        cur_dur = 0;
                        from_prev = false;
                    }
                    if day + 1 != days as usize {
                        break; // 当日已有安排时，今日剩余时间留空，视频顺延
                    }
                    continue;
                }
                let mut cont_day: Option<i64> = None;
                for (j, cap) in capacities.iter().enumerate().skip(day + 1) {
                    if *cap > 0 {
                        cont_day = Some(j as i64 + 1);
                        break;
                    }
                }
                entries.push(PlanEntry {
                    vid_no: cur_no,
                    title: cur_title.clone(),
                    portion: need,
                    from_prev,
                    remainder: cur_dur - need,
                    cont_day,
                });
                cur_dur -= need;
                assigned += need;
                from_prev = true;
            }
        }
        plan.push(entries);
    }

    Ok(PlanOut {
        plan,
        capacities,
        total,
    })
}

/// 秒 -> 时:分:秒（无小时时输出 分:秒），与 `fmt_seconds` 对齐。
pub fn fmt_seconds(sec: f64, force_hours: bool) -> String {
    let sec = sec.round() as i64;
    let h = sec / 3600;
    let m = (sec % 3600) / 60;
    let s = sec % 60;
    if force_hours || h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// 人类可读时长（如 1小时2分5秒），与 `fmt_human` 对齐。
pub fn fmt_human(sec: f64) -> String {
    let sec = sec.round() as i64;
    let h = sec / 3600;
    let m = (sec % 3600) / 60;
    let s = sec % 60;
    let mut parts: Vec<String> = Vec::new();
    if h > 0 {
        parts.push(format!("{h}小时"));
    }
    if m > 0 || h > 0 {
        parts.push(format!("{m}分"));
    }
    parts.push(format!("{s}秒"));
    parts.concat()
}

/// 近似显示宽度（中日韩全角字符按 2 列计算），与 `disp_width` 对齐。
pub fn disp_width(s: &str) -> usize {
    s.chars()
        .map(|ch| {
            let o = ch as u32;
            if (0x2E80..=0x9FFF).contains(&o)
                || (0xF900..=0xFAFF).contains(&o)
                || (0xFF00..=0xFF60).contains(&o)
                || (0xAC00..=0xD7AF).contains(&o)
            {
                2
            } else {
                1
            }
        })
        .sum()
}

/// 右填充到指定显示宽度。
pub fn pad(s: &str, width: usize) -> String {
    let w = disp_width(s);
    if w < width {
        format!("{s}{}", " ".repeat(width - w))
    } else {
        s.to_string()
    }
}

/// 按显示宽度截断，超长加省略号。
pub fn trunc(s: &str, width: usize) -> String {
    if disp_width(s) <= width {
        return s.to_string();
    }
    let mut out = String::new();
    let mut cur = 0usize;
    for ch in s.chars() {
        let cw = disp_width(&ch.to_string());
        if cur + cw > width - 1 {
            break;
        }
        out.push(ch);
        cur += cw;
    }
    out + "…"
}

/// 备注文字（与 `note_for` 对齐）。
pub fn note_for(e: &PlanEntry, _day_i: usize) -> String {
    let portion = fmt_seconds(e.portion as f64, true);
    if e.remainder > 0 {
        let rem = fmt_seconds(e.remainder as f64, true);
        if e.from_prev {
            return format!("接续自上一日 → 本日 {portion}，仍未看完，剩余 {rem} 继续顺延");
        }
        let cont = match e.cont_day {
            Some(d) => format!("第{d}天"),
            None => "后续有安排的日期".to_string(),
        };
        return format!("跨天分割 → 本日 {portion}，剩余 {rem} 顺延至{cont}");
    }
    if e.from_prev {
        return format!("接续自上一日 → 本日 {portion}，本视频观看完毕");
    }
    "完整观看".to_string()
}

/// 渲染每日观看计划文本（与 `render_plan` 对齐）。
pub fn render_plan(
    plan: &[Vec<PlanEntry>],
    capacities: &[i64],
    total: i64,
    days: i64,
    mode: Mode,
) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let bar = "=".repeat(78);
    lines.push(bar.clone());
    if mode == Mode::Whole {
        lines.push("每日观看计划（视频保持完整，不跨天拆分）".to_string());
    } else {
        lines.push("每日观看计划（跨天视频已标注分割点）".to_string());
    }
    lines.push(bar.clone());

    let mut cumulative: i64 = 0;
    for (day_i, entries) in plan.iter().enumerate() {
        let day_total: i64 = entries.iter().map(|e| e.portion).sum();
        cumulative += day_total;
        let remaining = total - cumulative;
        lines.push(String::new());
        lines.push(format!(
            "【第 {} 天】目标 {} ｜ 当日累计 {} ｜ 进度 {:.1}% ｜ 剩余总时长 {}",
            day_i + 1,
            fmt_seconds(capacities[day_i] as f64, true),
            fmt_seconds(day_total as f64, true),
            cumulative as f64 / total as f64 * 100.0,
            fmt_seconds(remaining as f64, true),
        ));
        if entries.is_empty() {
            lines.push("   （本日无安排 / 休息）".to_string());
            continue;
        }
        lines.push(pad("  视频", 8) + &pad("标题", 50) + &pad("本日时长", 12) + "备注");
        lines.push("-".repeat(78));
        for e in entries {
            lines.push(
                pad(&format!("  #{}", e.vid_no), 8)
                    + &pad(&trunc(&e.title, 48), 50)
                    + &pad(&fmt_seconds(e.portion as f64, true), 12)
                    + &note_for(e, day_i),
            );
        }
    }

    lines.push(String::new());
    lines.push(bar.clone());
    lines.push(format!(
        "统计：共 {days} 天，总时长 {}（{}）",
        fmt_seconds(total as f64, true),
        fmt_human(total as f64)
    ));
    lines.push(bar);
    lines
}

/// 各科目统计文本（与 `render_group_summary` 对齐）。
pub fn render_group_summary(groups: &[Group]) -> Vec<String> {
    let mut lines = vec!["各科目统计：".to_string()];
    for (i, g) in groups.iter().enumerate() {
        let total: i64 = g.episodes.iter().map(|e| e.duration).sum();
        lines.push(format!(
            "  {}. {} —— {} 个视频，共 {}（{}）",
            i + 1,
            g.name,
            g.episodes.len(),
            fmt_seconds(total as f64, true),
            fmt_human(total as f64)
        ));
    }
    lines
}
