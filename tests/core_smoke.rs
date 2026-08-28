//! core 编排层冒烟测试：覆盖获取分派、计划生成与导出载荷的纯逻辑路径。
//!
//! 旧版（fenestra）通过访问树断言 UI 结构；迁移到 gpui-component 后，
//! UI 逻辑收敛到 `Render`（需 GPU 窗口），此处改为验证驱动 UI 的核心
//! 纯函数行为——`app.rs` 的每个交互动作最终都落到这些函数上。

use bili_planner::core::{
    export_payload, fetch_and_parse, generate_plan, parse_days, FetchSource, ReadyState, Selection,
    SourceMode,
};
use bili_planner::parse::{EpisodeItem, Group};
use bili_planner::plan::Mode;

fn sample_ready(groups: Vec<Group>) -> ReadyState {
    let selection = if groups.len() > 1 {
        Selection::All
    } else {
        Selection::Single(0)
    };
    ReadyState {
        season_title: "测试合集".into(),
        structure: "多分栏合集（每个分栏视为一门科目）".into(),
        groups,
        selection,
        plan: None,
    }
}

fn two_groups() -> Vec<Group> {
    vec![
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
    ]
}

#[test]
fn parse_days_validation() {
    assert!(parse_days("").is_err(), "空输入应报错");
    assert!(parse_days("abc").is_err(), "非数字应报错");
    assert!(parse_days("0").is_err(), "非正数应报错");
    assert_eq!(parse_days(" 30 ").unwrap(), 30);
}

#[test]
fn fetch_rejects_empty_jellyfin_credentials() {
    let err = fetch_and_parse(
        "https://jellyfin.example/web/#!/details?id=abc",
        &FetchSource::Jellyfin {
            server_url: "  ".into(),
            token: "t".into(),
        },
    )
    .expect_err("空服务器地址应报错");
    assert!(
        err.contains("服务器地址"),
        "错误信息应指向服务器地址：{err}"
    );

    let err = fetch_and_parse(
        "x",
        &FetchSource::Jellyfin {
            server_url: "https://jf.example".into(),
            token: "".into(),
        },
    )
    .expect_err("空 Token 应报错");
    assert!(err.contains("Token"), "错误信息应指向 Token：{err}");
}

#[test]
fn generate_plan_all_scope_prefixes_subjects() {
    let mut rd = sample_ready(two_groups());
    rd.selection = Selection::All;
    generate_plan(&mut rd, 3, Mode::Split).expect("生成计划");
    let plan = rd.plan.expect("plan");
    assert_eq!(plan.total, 3720 + 5400);
    assert_eq!(plan.days, 3);
    assert!(
        plan.plan
            .iter()
            .flatten()
            .any(|e| e.title.starts_with("[科目")),
        "全部科目范围应带 [科目] 前缀"
    );
}

#[test]
fn generate_plan_single_scope_uses_group_episodes() {
    let mut rd = sample_ready(two_groups());
    rd.selection = Selection::Single(1);
    generate_plan(&mut rd, 2, Mode::Split).expect("生成计划");
    let plan = rd.plan.expect("plan");
    assert_eq!(plan.total, 5400, "单科目范围只统计该科目时长");
    assert_eq!(plan.scope_desc, "第二章 进阶（1 个视频）");
}

#[test]
fn generate_plan_rejects_bad_days_and_empty_scope() {
    let mut rd = sample_ready(two_groups());
    assert!(
        generate_plan(&mut rd, 0, Mode::Split).is_err(),
        "0 天应报错"
    );

    let mut rd = sample_ready(vec![Group {
        name: "空科目".into(),
        episodes: vec![],
    }]);
    assert!(
        generate_plan(&mut rd, 1, Mode::Split).is_err(),
        "总时长为 0 应报错"
    );
}

#[test]
fn export_payload_requires_plan() {
    let mut rd = sample_ready(two_groups());
    assert!(
        export_payload(&rd, Mode::Split).is_none(),
        "未生成计划时无导出载荷"
    );

    generate_plan(&mut rd, 2, Mode::Split).expect("生成计划");
    let (text, file) = export_payload(&rd, Mode::Split).expect("导出载荷");
    assert!(text.contains("测试合集"), "导出文本应含合集标题");
    assert!(file.starts_with("观看计划_"), "建议文件名前缀正确：{file}");
}

#[test]
fn source_mode_index_roundtrip() {
    assert_eq!(SourceMode::default(), SourceMode::Bilibili);
    assert_eq!(
        SourceMode::from_index(SourceMode::Jellyfin.index()),
        SourceMode::Jellyfin
    );
}
