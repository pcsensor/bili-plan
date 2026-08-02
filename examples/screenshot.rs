//! Headless UI 渲染截图（需要 GPU 适配器；用于人工/工具验证界面）。
//! 运行：cargo run --example screenshot

use bili_planner::app::{Phase, PlannerApp, ReadyState, Selection};
use bili_planner::parse::{EpisodeItem, Group};
use bili_planner::plan::{build_plan, Mode};
use fenestra::prelude::*;
use fenestra::shell::render_app;

fn main() {
    let groups = vec![
        Group {
            name: "第一章 基础".into(),
            episodes: vec![
                EpisodeItem {
                    title: "1.1 集合与映射".into(),
                    duration: 3720,
                },
                EpisodeItem {
                    title: "1.2 逻辑与证明".into(),
                    duration: 4200,
                },
                EpisodeItem {
                    title: "1.3 关系与函数".into(),
                    duration: 2100,
                },
            ],
        },
        Group {
            name: "第二章 进阶".into(),
            episodes: vec![
                EpisodeItem {
                    title: "2.1 图论初步".into(),
                    duration: 5400,
                },
                EpisodeItem {
                    title: "2.2 代数结构".into(),
                    duration: 2700,
                },
                EpisodeItem {
                    title: "2.3 组合数学".into(),
                    duration: 3600,
                },
            ],
        },
    ];
    let items: Vec<EpisodeItem> = groups.iter().flat_map(|g| g.episodes.clone()).collect();
    let total: i64 = items.iter().map(|i| i.duration).sum();
    let days = 5;
    let out = build_plan(&items, days, Mode::Split).expect("plan");
    let plan = bili_planner::app::PlanData {
        plan: out.plan,
        capacities: out.capacities,
        total,
        days,
        avg: total as f64 / days as f64,
        scope_desc: "整个合集（全部科目）".to_string(),
    };
    let rd = ReadyState {
        season_title: "高等数学教程（示例数据）".to_string(),
        structure: "多分栏合集（每个分栏视为一门科目）".to_string(),
        groups,
        selection: Selection::All,
        plan: Some(plan),
    };
    let mut app = PlannerApp::new();
    app.days_text = "5".to_string();
    app.phase = Phase::Ready(rd);

    let light = render_app(&mut app, &[], (1120, 820), &Theme::light());
    light.save("ui_preview_light.png").expect("save light png");

    app.dark = true;
    let dark = render_app(&mut app, &[], (1120, 820), &Theme::dark());
    dark.save("ui_preview_dark.png").expect("save dark png");

    println!("saved ui_preview_light.png / ui_preview_dark.png");
}
