//! Headless UI 渲染截图（需要 GPU 适配器；用于人工/工具验证界面）。
//! 运行：cargo run --example screenshot
//!
//! 视口加高到 1560 以在一张图里捕获完整页面（真实窗口为 1120x820，
//! 超出部分在应用内滚动查看）。

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

    let light_theme = app.theme();
    let light = render_app(&mut app, &[], (1120, 1560), &light_theme);
    light.save("ui_preview_light.png").expect("save light png");

    app.dark = true;
    let dark_theme = app.theme();
    let dark = render_app(&mut app, &[], (1120, 1560), &dark_theme);
    dark.save("ui_preview_dark.png").expect("save dark png");

    // 长计划场景（60 视频 / 10 天 ≈ 70 行）：超过 data_table 的 50 行
    // 自动虚拟化阈值时，表体切换为内部滚动 + 虚拟化（每帧只构建可视窗口
    // 内的行，长计划滑动保持 60fps；2026-08-03 修复）。截图在真实窗口
    // 高度即可看到卡片内的固定高度表体与吸顶表头。
    let long_episodes: Vec<EpisodeItem> = (1..=60)
        .map(|i| EpisodeItem {
            title: format!("P{i} 离散数学 第{i}讲"),
            duration: 3600,
        })
        .collect();
    let long_groups = vec![Group {
        name: "全部课程".into(),
        episodes: long_episodes.clone(),
    }];
    let long_total: i64 = long_episodes.iter().map(|e| e.duration).sum();
    let long_days = 10;
    let long_out = build_plan(&long_episodes, long_days, Mode::Split).expect("plan");
    let long_plan = bili_planner::app::PlanData {
        plan: long_out.plan,
        capacities: long_out.capacities,
        total: long_total,
        days: long_days,
        avg: long_total as f64 / long_days as f64,
        scope_desc: "全部课程（60 个视频）".to_string(),
    };
    let long_rd = ReadyState {
        season_title: "离散数学（长计划回归验证）".to_string(),
        structure: "单分栏合集".to_string(),
        groups: long_groups,
        selection: Selection::Single(0),
        plan: Some(long_plan),
    };
    let mut long_app = PlannerApp::new();
    long_app.days_text = long_days.to_string();
    long_app.phase = Phase::Ready(long_rd);
    let long_theme = long_app.theme();
    let long = render_app(&mut long_app, &[], (1120, 1560), &long_theme);
    long.save("ui_preview_long.png").expect("save long png");

    println!("saved ui_preview_light.png / ui_preview_dark.png / ui_preview_long.png");
}
