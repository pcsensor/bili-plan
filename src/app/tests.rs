use super::*;
use crate::parse::{EpisodeItem, Group};
use gpui::TestAppContext;

#[test]
fn compact_subject_shortens_prefix() {
    assert_eq!(table::compact_subject("[科目 1] P1 前言"), "科1·P1 前言");
    assert_eq!(table::compact_subject("[科目12] 集合"), "科12·集合");
    assert_eq!(table::compact_subject("普通标题"), "普通标题");
    assert_eq!(table::compact_subject("[科目"), "[科目");
}

/// 构造带两门科目、已生成 3 天计划的就绪状态。
fn ready_with_plan() -> ReadyState {
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
    ReadyState {
        season_title: "测试合集".into(),
        structure: "多分栏合集".into(),
        groups,
        selection: Selection::All,
        plan: None,
    }
}

/// 无头渲染冒烟：窗口构建、计划表委托、亮/暗主题下的元素树构建
/// 都不应 panic。
#[gpui::test]
async fn render_smoke_both_themes(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::theme::init(cx);
    });

    let window = cx.add_window(|window, cx| {
        let mut app = PlannerApp::new(window, cx);
        let mut rd = ready_with_plan();
        let mut expanded = false;
        PlannerApp::run_generate(
            &mut rd,
            Mode::Split,
            3,
            &mut app.plan_table,
            &mut expanded,
            window,
            cx,
        );
        assert!(rd.plan.is_some(), "计划应已生成");
        app.phase = Phase::Ready(rd);
        app
    });

    window
        .update(cx, |app, window, cx| {
            // 亮色：构建完整元素树（表单卡 + 信息卡 + 科目选择 + 表格）。
            let _ = app.render(window, cx);

            // 切换暗色后再次构建（主题配置重套用 + 渲染路径不 panic）。
            Theme::change(ThemeMode::Dark, Some(window), cx);
            let _ = app.render(window, cx);

            app.source = SourceMode::FnOs;
            app.phase = Phase::Loading;
            app.fetch_progress = "当前目录：11 / 36 个视频时长已就绪".into();
            app.fetch_elapsed_secs = 42;
            let _ = app.render(window, cx);
            Theme::change(ThemeMode::Light, Some(window), cx);
            let _ = app.render(window, cx);
            let generation = app.fetch_generation;
            app.switch_source(SourceMode::Jellyfin, window, cx);
            assert!(matches!(app.phase, Phase::Input));
            assert_ne!(app.fetch_generation, generation);
        })
        .expect("window update should succeed");
}

/// Exercise the state transitions behind source, mode, selection, history,
/// and calendar controls without external services or local writes.
#[gpui::test]
async fn planner_controls_keep_state_consistent(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::theme::init(cx);
    });
    let window = cx.add_window(|window, cx| {
        let mut app = PlannerApp::new(window, cx);
        app.config = AppConfig::default();
        app
    });
    window
        .update(cx, |app, window, cx| {
            app.days_input.update(cx, |input, cx| {
                input.set_value("3", window, cx);
            });
            let mut ready = ready_with_plan();
            PlannerApp::run_generate(
                &mut ready,
                Mode::Split,
                3,
                &mut app.plan_table,
                &mut app.window_expanded,
                window,
                cx,
            );
            app.phase = Phase::Ready(ready);
            assert!(app.plan_table.is_some());

            app.set_selection(Selection::Single(0), window, cx);
            let Phase::Ready(ready) = &app.phase else {
                panic!("selecting a subject should keep the ready state");
            };
            assert_eq!(ready.selection, Selection::Single(0));
            assert!(ready.plan.is_some());
            app.switch_mode(Mode::Whole, window, cx);
            assert_eq!(app.mode, Mode::Whole);
            assert!(matches!(&app.phase, Phase::Ready(ready) if ready.plan.is_some()));

            app.switch_source(SourceMode::FnOs, window, cx);
            assert!(matches!(app.phase, Phase::Input));
            assert!(app.plan_table.is_none());
            app.config.history.push(crate::core::HistoryEntry {
                input: "https://jf.example/web/?id=123".into(),
                source: "jellyfin".into(),
                title: "合集".into(),
                at: 1,
            });
            app.use_history_entry(0, window, cx);
            assert_eq!(app.source, SourceMode::Jellyfin);
            assert_eq!(
                app.link_input.read(cx).value().to_string(),
                "https://jf.example/web/?id=123"
            );

            app.selected_date = "2024-02-29".into();
            app.next_date_action(cx);
            assert_eq!(app.selected_date, "2024-03-01");
            app.prev_date_action(cx);
            assert_eq!(app.selected_date, "2024-02-29");
            app.calendar_year = 2024;
            app.calendar_month = 12;
            app.next_calendar_month_action(cx);
            assert_eq!((app.calendar_year, app.calendar_month), (2025, 1));
            app.prev_calendar_month_action(cx);
            assert_eq!((app.calendar_year, app.calendar_month), (2024, 12));

            let mut local =
                crate::study::create_custom_study_plan("本地编辑", "2024-03-01", 1, 30, false)
                    .expect("valid plan");
            let mut remote = local.clone();
            remote.title = "同步前的标题".into();
            remote.schedules[0].tasks[0].completed = true;
            remote.schedules[0].tasks[0].updated_at = 42;
            local.schedules[0].tasks[0].updated_at = 1;
            app.config.plans = vec![local];
            app.merge_synced_plans(vec![remote.clone()]);
            assert_eq!(app.config.plans[0].title, "本地编辑");
            assert!(app.config.plans[0].schedules[0].tasks[0].completed);
            app.config.plans.clear();
            app.merge_synced_plans(vec![remote]);
            assert!(
                app.config.plans.is_empty(),
                "old sync must not revive a deleted plan"
            );
        })
        .expect("window update should succeed");
}

#[gpui::test]
async fn modal_actions_prefill_and_reset_edit_state(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::theme::init(cx);
    });
    let window = cx.add_window(|window, cx| {
        let mut app = PlannerApp::new(window, cx);
        app.config = AppConfig::default();
        app
    });
    window
        .update(cx, |app, window, cx| {
            let plan =
                crate::study::create_custom_study_plan("练习", "2024-03-02", 2, 20, false).unwrap();
            let plan_id = plan.id.clone();
            app.config.plans.push(plan);
            app.open_plan_reschedule_action(&plan_id, window, cx);
            assert!(app.plan_reschedule_modal_open);
            assert_eq!(
                app.plan_reschedule_plan_id.as_deref(),
                Some(plan_id.as_str())
            );
            assert_eq!(
                app.plan_reschedule_date_input.read(cx).value().to_string(),
                "2024-03-02"
            );

            app.open_calendar_task_modal_action("2024-03-04", window, cx);
            assert!(app.calendar_task_modal_open);
            assert_eq!(app.calendar_selected_date, "2024-03-04");
            app.calendar_task_title_input
                .update(cx, |input, cx| input.set_value("旧标题", window, cx));
            app.open_calendar_task_modal_action("2024-03-05", window, cx);
            assert_eq!(
                app.calendar_task_title_input.read(cx).value().to_string(),
                ""
            );
            assert_eq!(
                app.calendar_task_date_input.read(cx).value().to_string(),
                "2024-03-05"
            );

            app.open_calendar_task_edit_modal_action(
                CalendarTaskEditSeed {
                    plan_id: plan_id.clone(),
                    task_id: "task-1".into(),
                    title: "复习".into(),
                    date: "2024-03-06".into(),
                    portion: 1800,
                },
                window,
                cx,
            );
            assert!(app.calendar_task_edit_modal_open);
            assert_eq!(
                app.calendar_task_edit_duration_input
                    .read(cx)
                    .value()
                    .to_string(),
                "30"
            );

            app.config.sync_server_url = "https://plan.example".into();
            app.open_cloud_settings_action(window, cx);
            assert!(app.cloud_settings_modal_open);
            assert_eq!(
                app.cloud_server_input.read(cx).value().to_string(),
                "https://plan.example"
            );
        })
        .expect("window update should succeed");
}
