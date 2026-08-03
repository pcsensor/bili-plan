//! 端到端验证：命令行方式运行核心逻辑（与 Python CLI 输出对比用）。
//! 用法：cargo run --example live_check -- "<链接/BV/sid>" [days] [all|科目号] [split|whole]
//! 环境变量 BILI_COOKIE 可传 Cookie。

use bili_planner::app::{fetch_and_parse, generate_plan, FetchSource, Selection};
use bili_planner::export;
use bili_planner::plan::Mode;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let input = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "BV1ps4y1d73V".to_string());
    let days: i64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(30);
    let select = args.get(3).map(|s| s.as_str()).unwrap_or("all");
    let mode = if args.get(4).map(|s| s.as_str()) == Some("whole") {
        Mode::Whole
    } else {
        Mode::Split
    };
    let cookie = std::env::var("BILI_COOKIE").ok();

    let mut rd =
        fetch_and_parse(&input, &FetchSource::Bilibili { cookie }).expect("fetch/parse 失败");
    eprintln!("结构识别：{}", rd.structure);
    eprintln!("科目数：{}", rd.groups.len());
    // 与 Python 脚本一致：仅多科目时 select 生效；单科目保持默认
    if rd.groups.len() > 1 {
        if select == "all" {
            rd.selection = Selection::All;
        } else if let Ok(i) = select.parse::<usize>() {
            rd.selection = Selection::Single(i.saturating_sub(1));
        }
    }
    generate_plan(&mut rd, days, mode).expect("生成计划失败");
    let p = rd.plan.as_ref().expect("plan");
    let text = export::full_text(
        &rd.season_title,
        &rd.structure,
        &p.scope_desc,
        p.total,
        p.days,
        p.avg,
        &rd.groups,
        &p.plan,
        &p.capacities,
        p.total,
        mode,
    );
    print!("{text}");
}
