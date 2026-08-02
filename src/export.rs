//! 完整文本输出：与 Python 脚本 main() 的 out_lines 完全一致。

use crate::parse::Group;
use crate::plan::{fmt_human, fmt_seconds, render_group_summary, render_plan, Mode, PlanEntry};

/// 构建完整计划文本行（对应 Python main 的 out_lines）。
#[allow(clippy::too_many_arguments)]
pub fn build_output(
    season_title: &str,
    structure: &str,
    scope_desc: &str,
    total: i64,
    days: i64,
    avg: f64,
    groups: &[Group],
    plan: &[Vec<PlanEntry>],
    capacities: &[i64],
    plan_total: i64,
    mode: Mode,
) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("合集：《{season_title}》"));
    lines.push(format!("结构识别：{structure}"));
    lines.push(format!("统计范围：{scope_desc}"));
    lines.push(format!(
        "总时长：{}（{}）",
        fmt_seconds(total as f64, true),
        fmt_human(total as f64)
    ));
    lines.push(format!("目标天数：{days} 天"));
    lines.push(format!(
        "日均观看：{}（约 {:.1} 分钟/天）",
        fmt_seconds(avg, true),
        avg / 60.0
    ));
    if groups.len() > 1 {
        lines.push(String::new());
        lines.extend(render_group_summary(groups));
    }
    lines.push(String::new());
    lines.extend(render_plan(plan, capacities, plan_total, days, mode));
    lines
}

/// 完整计划文本（UTF-8，末尾换行），对应 Python 写入文件的内容。
#[allow(clippy::too_many_arguments)]
pub fn full_text(
    season_title: &str,
    structure: &str,
    scope_desc: &str,
    total: i64,
    days: i64,
    avg: f64,
    groups: &[Group],
    plan: &[Vec<PlanEntry>],
    capacities: &[i64],
    plan_total: i64,
    mode: Mode,
) -> String {
    let lines = build_output(
        season_title,
        structure,
        scope_desc,
        total,
        days,
        avg,
        groups,
        plan,
        capacities,
        plan_total,
        mode,
    );
    format!("{}\n", lines.join("\n"))
}
