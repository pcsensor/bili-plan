//! UI 冒烟测试：用 fenestra 访问树（AccessKit）断言关键组件与文案，
//! 不依赖 GPU/窗口（纯布局）。

use bili_planner::app::{Phase, PlannerApp, ReadyState, Selection};
use bili_planner::parse::{EpisodeItem, Group};
use bili_planner::plan::{build_plan, Mode};
use fenestra::prelude::*;
use fenestra::{build_frame, by, Frame, FrameState, Theme};

fn frame_for(app: &PlannerApp) -> Frame {
    build_frame(
        &app.view(),
        &Theme::light(),
        &mut Fonts::embedded(),
        &mut FrameState::new(),
        (1120.0, 820.0),
        1.0,
    )
}

fn ready_app() -> PlannerApp {
    let groups = vec![
        Group {
            name: "第一章 基础".into(),
            episodes: vec![EpisodeItem {
                title: "1.1 集合".into(),
                duration: 3720,
            }],
        },
        Group {
            name: "第二章 进阶".into(),
            episodes: vec![EpisodeItem {
                title: "2.1 图论".into(),
                duration: 5400,
            }],
        },
    ];
    let items: Vec<EpisodeItem> = groups.iter().flat_map(|g| g.episodes.clone()).collect();
    let total: i64 = items.iter().map(|i| i.duration).sum();
    let out = build_plan(&items, 3, Mode::Split).expect("plan");
    let plan = bili_planner::app::PlanData {
        plan: out.plan,
        capacities: out.capacities,
        total,
        days: 3,
        avg: total as f64 / 3.0,
        scope_desc: "整个合集（全部科目）".to_string(),
    };
    let rd = ReadyState {
        season_title: "测试合集".into(),
        structure: "多分栏合集（每个分栏视为一门科目）".into(),
        groups,
        selection: Selection::All,
        plan: Some(plan),
    };
    let mut app = PlannerApp::new();
    app.days_text = "3".to_string();
    app.phase = Phase::Ready(rd);
    app
}

#[test]
fn form_has_controls() {
    let frame = frame_for(&PlannerApp::new());
    assert!(frame.query(&by::id("input")).is_some(), "链接输入框存在");
    assert!(frame.query(&by::id("days")).is_some(), "天数输入框存在");
    assert!(
        frame.query(&by::id("cookie")).is_some(),
        "Cookie 输入框存在"
    );
    assert!(
        frame
            .query(&by::role(Semantics::Button).name("获取视频信息"))
            .is_some(),
        "获取按钮存在"
    );
}

#[test]
fn ready_state_shows_structure_and_actions() {
    let frame = frame_for(&ready_app());
    assert!(
        !frame.get_all(&by::label_contains("测试合集")).is_empty(),
        "显示合集标题"
    );
    assert!(
        !frame
            .get_all(&by::label_contains("结构识别：多分栏合集"))
            .is_empty(),
        "显示结构识别"
    );
    assert!(
        !frame
            .get_all(&by::label_contains("整个合集（全部科目）"))
            .is_empty(),
        "显示全部科目选项"
    );
    assert!(
        frame
            .query(&by::role(Semantics::Button).name("生成观看计划"))
            .is_some(),
        "生成按钮存在"
    );
    assert!(
        frame
            .query(&by::role(Semantics::Button).name("导出计划文本（UTF-8）"))
            .is_some(),
        "导出按钮存在"
    );
    assert!(
        !frame.get_all(&by::label_contains("总时长：")).is_empty(),
        "显示总时长"
    );
    assert!(
        !frame.get_all(&by::label_contains("第 1 天")).is_empty(),
        "计划表包含每日行"
    );
}

#[test]
fn dark_theme_does_not_panic() {
    let mut app = ready_app();
    app.dark = true;
    let frame = build_frame(
        &app.view(),
        &Theme::dark(),
        &mut Fonts::embedded(),
        &mut FrameState::new(),
        (1120.0, 820.0),
        1.0,
    );
    assert!(frame
        .query(&by::role(Semantics::Button).name("亮色"))
        .is_some());
}
