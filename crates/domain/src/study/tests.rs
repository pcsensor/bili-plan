use super::*;
use crate::plan::PlanEntry;

fn mock_plan_out() -> PlanOut {
    PlanOut {
        plan: vec![
            vec![
                PlanEntry {
                    vid_no: 1,
                    title: "P1 第一节".to_string(),
                    portion: 600,
                    from_prev: false,
                    remainder: 0,
                    cont_day: None,
                },
                PlanEntry {
                    vid_no: 2,
                    title: "P2 第二节".to_string(),
                    portion: 400,
                    from_prev: false,
                    remainder: 200,
                    cont_day: Some(2),
                },
            ],
            vec![
                PlanEntry {
                    vid_no: 2,
                    title: "P2 第二节".to_string(),
                    portion: 200,
                    from_prev: true,
                    remainder: 0,
                    cont_day: None,
                },
                PlanEntry {
                    vid_no: 3,
                    title: "P3 第三节".to_string(),
                    portion: 800,
                    from_prev: false,
                    remainder: 0,
                    cont_day: None,
                },
            ],
        ],
        capacities: vec![1000, 1000],
        total: 2000,
    }
}

#[test]
fn creating_a_plan_rejects_invalid_start_date() {
    let result = create_study_plan(
        "课程",
        "bilibili",
        "BV1",
        "全集",
        &mock_plan_out(),
        "2026-02-30",
        false,
    );
    assert!(
        result.is_err(),
        "invalid dates must not silently become today"
    );
}

#[test]
fn manual_move_preserves_task_and_other_schedules() {
    let mut plan = create_study_plan(
        "课程",
        "bilibili",
        "BV1",
        "全集",
        &mock_plan_out(),
        "2026-09-01",
        false,
    )
    .unwrap();
    let original = plan.clone();
    let task = original.schedules[0].tasks[1].clone();
    move_task_to_date(&mut plan, &task.id, "2026-09-05").unwrap();
    assert_eq!(
        plan.schedules[0].tasks,
        vec![original.schedules[0].tasks[0].clone()]
    );
    assert_eq!(plan.schedules[1], original.schedules[1]);
    let mut expected = task;
    expected.updated_at = plan.schedules[2].tasks[0].updated_at;
    assert_eq!(plan.schedules[2].tasks, vec![expected]);
    assert_eq!(plan.total_duration, original.total_duration);
    assert_eq!(plan.end_date, "2026-09-05");
    let roundtrip: StudyPlan =
        serde_json::from_str(&serde_json::to_string(&plan).unwrap()).unwrap();
    assert_eq!(roundtrip, plan);
}

#[test]
fn manual_move_validates_before_mutation_and_same_day_is_noop() {
    let mut plan = create_custom_study_plan("任务", "2026-09-01", 2, 30, false).unwrap();
    let id = plan.schedules[0].tasks[0].id.clone();
    let before = plan.clone();
    assert!(move_task_to_date(&mut plan, &id, "2026-02-30").is_err());
    assert!(move_task_to_date(&mut plan, "missing", "2026-09-02").is_err());
    move_task_to_date(&mut plan, &id, "2026-09-01").unwrap();
    assert_eq!(plan, before);
}

#[test]
fn manual_move_to_rest_day_merges_without_duplicate_dates() {
    let mut plan = create_custom_study_plan("任务", "2026-09-04", 2, 30, true).unwrap();
    let first = plan.schedules[0].tasks[0].id.clone();
    let last = plan.schedules[3].tasks[0].id.clone();
    move_task_to_date(&mut plan, &first, "2026-09-05").unwrap();
    move_task_to_date(&mut plan, &last, "2026-09-05").unwrap();
    assert_eq!(plan.schedules.len(), 1);
    assert!(!plan.schedules[0].is_rest_day);
    assert_eq!(plan.schedules[0].tasks.len(), 2);
    assert_eq!(plan.planned_days, 1);
    assert_eq!(plan.start_date, "2026-09-05");
}

#[test]
fn manual_move_composes_with_postpone_and_advance() {
    let mut plan = create_custom_study_plan("任务", "2026-09-01", 3, 30, false).unwrap();
    let id = plan.schedules[2].tasks[0].id.clone();
    move_task_to_date(&mut plan, &id, "2026-09-02").unwrap();
    push_forward_plan(&mut plan, "2026-09-02").unwrap();
    assert!(plan
        .schedules
        .iter()
        .find(|s| s.date == "2026-09-03")
        .unwrap()
        .tasks
        .iter()
        .any(|t| t.id == id));
    let pid = plan.id.clone();
    let mut plans = vec![plan];
    toggle_task_checkin(&mut plans, &pid, &id).unwrap();
    advance_completed_tasks(&mut plans[0], "2026-09-03", "2026-09-01").unwrap();
    toggle_task_checkin(&mut plans, &pid, &id).unwrap();
    assert!(plans[0]
        .schedules
        .iter()
        .find(|s| s.date == "2026-09-03")
        .unwrap()
        .tasks
        .iter()
        .any(|t| t.id == id && !t.completed));
}

#[test]
fn manual_move_replaces_advanced_return_date_without_losing_checkin() {
    let mut plan = create_custom_study_plan("任务", "2026-09-01", 3, 30, false).unwrap();
    let id = plan.schedules[1].tasks[0].id.clone();
    let pid = plan.id.clone();
    let mut plans = vec![plan.clone()];
    toggle_task_checkin(&mut plans, &pid, &id).unwrap();
    plan = plans.remove(0);
    advance_completed_tasks(&mut plan, "2026-09-02", "2026-09-01").unwrap();
    let completed_at = plan.schedules[0]
        .tasks
        .iter()
        .find(|t| t.id == id)
        .unwrap()
        .completed_at;
    move_task_to_date(&mut plan, &id, "2026-09-06").unwrap();
    let moved = &plan.schedules.last().unwrap().tasks[0];
    assert!(moved.completed);
    assert_eq!(moved.completed_at, completed_at);
    assert_eq!(moved.advanced_from_date, None);
    let mut plans = vec![plan];
    toggle_task_checkin(&mut plans, &pid, &id).unwrap();
    assert_eq!(plans[0].schedules.last().unwrap().date, "2026-09-06");
}

#[test]
fn create_plan_continuous_days() {
    let plan_out = mock_plan_out();
    let plan = create_study_plan(
        "高数",
        "bilibili",
        "BV123",
        "全集",
        &plan_out,
        "2026-09-01",
        false,
    )
    .unwrap();

    assert_eq!(plan.title, "高数");
    assert_eq!(plan.planned_days, 2);
    assert_eq!(plan.start_date, "2026-09-01");
    assert_eq!(plan.end_date, "2026-09-02");
    assert_eq!(plan.schedules.len(), 2);
    assert_eq!(plan.schedules[0].tasks.len(), 2);
    assert_eq!(plan.schedules[1].tasks.len(), 2);
}

#[test]
fn infer_start_date_from_current_plan_day() {
    assert_eq!(
        infer_plan_start_date("2026-09-10", 1, false).unwrap(),
        "2026-09-10"
    );
    assert_eq!(
        infer_plan_start_date("2026-09-10", 4, false).unwrap(),
        "2026-09-07"
    );
}

#[test]
fn infer_start_date_counts_only_study_days_when_skipping_weekends() {
    // 2026-09-07 是周一，向前数 2 个学习日后，第 3 天为周一。
    assert_eq!(
        infer_plan_start_date("2026-09-07", 3, true).unwrap(),
        "2026-09-03"
    );
    assert!(infer_plan_start_date("2026-09-06", 2, true).is_err());
    assert!(infer_plan_start_date("2026-09-07", 0, false).is_err());
}

#[test]
fn custom_plan_uses_existing_daily_schedule_model() {
    let plan = create_custom_study_plan("背单词", "2026-08-28", 2, 30, true).unwrap();
    assert_eq!(plan.source_type, "custom");
    assert_eq!(plan.total_duration, 3_600);
    assert_eq!(plan.schedules.len(), 4); // 周六、周日作为休息日保留
    assert_eq!(plan.schedules[0].date, "2026-08-28");
    assert_eq!(plan.schedules[3].date, "2026-08-31");
    assert_eq!(plan.schedules[3].tasks[0].portion, 1_800);
}

#[test]
fn daily_notes_support_multiple_items_and_tombstones() {
    let mut notes = DailyNotes::new();
    let first = add_daily_note(&mut notes, "2026-09-03", "完成练习").unwrap();
    let _second = add_daily_note(&mut notes, "2026-09-03", "整理错题").unwrap();
    assert_eq!(get_daily_notes(&notes, "2026-09-03").len(), 2);
    assert!(delete_daily_note(&mut notes, "2026-09-03", &first.id));
    assert_eq!(get_daily_notes(&notes, "2026-09-03").len(), 1);
}

#[test]
fn calendar_series_expands_from_manually_added_dates() {
    let mut plan = create_calendar_series("英语冲刺", "背 50 个单词", "2026-09-10", 30).unwrap();
    append_calendar_series_task(&mut plan, "完成阅读", "2026-09-08", 45).unwrap();
    append_calendar_series_task(&mut plan, "整理错题", "2026-09-10", 20).unwrap();

    assert!(plan.is_series);
    assert!(plan.show_in_library);
    assert_eq!(plan.start_date, "2026-09-08");
    assert_eq!(plan.end_date, "2026-09-10");
    assert_eq!(plan.planned_days, 2);
    assert_eq!(plan.total_duration, (30 + 45 + 20) * 60);
    let tenth = plan
        .schedules
        .iter()
        .find(|schedule| schedule.date == "2026-09-10")
        .unwrap();
    assert_eq!(tenth.tasks.len(), 2);
}

#[test]
fn one_off_calendar_task_is_not_a_library_plan() {
    let plan = create_one_off_calendar_task("预约体检", "2026-09-12", 20).unwrap();
    assert!(!plan.is_series);
    assert!(!plan.show_in_library);
    assert_eq!(plan.schedules[0].date, "2026-09-12");
}

#[test]
fn calendar_task_can_be_edited_moved_and_deleted() {
    let mut plan = create_calendar_series("英语冲刺", "背单词", "2026-09-10", 30).unwrap();
    let task_id = plan.schedules[0].tasks[0].id.clone();
    update_calendar_task(&mut plan, &task_id, "精读文章", "2026-09-12", 50).unwrap();

    assert_eq!(plan.start_date, "2026-09-12");
    assert_eq!(plan.end_date, "2026-09-12");
    assert_eq!(plan.total_duration, 3_000);
    assert_eq!(plan.schedules[0].tasks[0].title, "精读文章");
    assert!(delete_calendar_task(&mut plan, &task_id).unwrap());
}

#[test]
fn create_plan_skip_weekends() {
    let plan_out = mock_plan_out();
    // 2026-08-28 是周五，下两天是周六、周日
    let plan = create_study_plan(
        "高数",
        "bilibili",
        "BV123",
        "全集",
        &plan_out,
        "2026-08-28",
        true,
    )
    .unwrap();

    assert_eq!(plan.start_date, "2026-08-28");
    // 28(五任务), 29(六休息), 30(日休息), 31(一任务)
    assert_eq!(plan.end_date, "2026-08-31");
    assert_eq!(plan.schedules.len(), 4);
    assert_eq!(plan.schedules[0].date, "2026-08-28");
    assert!(!plan.schedules[0].is_rest_day);
    assert_eq!(plan.schedules[1].date, "2026-08-29");
    assert!(plan.schedules[1].is_rest_day);
    assert_eq!(plan.schedules[2].date, "2026-08-30");
    assert!(plan.schedules[2].is_rest_day);
    assert_eq!(plan.schedules[3].date, "2026-08-31");
    assert!(!plan.schedules[3].is_rest_day);
}

#[test]
fn task_checkin_and_progress() {
    let plan_out = mock_plan_out();
    let plan = create_study_plan(
        "高数",
        "bilibili",
        "BV123",
        "全集",
        &plan_out,
        "2026-09-01",
        false,
    )
    .unwrap();

    let task_id = plan.schedules[0].tasks[0].id.clone();
    let plan_id = plan.id.clone();
    let mut plans = vec![plan];

    let state = toggle_task_checkin(&mut plans, &plan_id, &task_id).unwrap();
    assert!(state);
    assert!(plans[0].schedules[0].tasks[0].completed);

    let (done, total, done_dur, total_dur, ratio) = compute_plan_progress(&plans[0]);
    assert_eq!(done, 1);
    assert_eq!(total, 4);
    assert_eq!(done_dur, 600);
    assert_eq!(total_dur, 2000);
    assert_eq!(ratio, 0.3);
}

#[test]
fn push_forward_plan_test() {
    let plan_out = mock_plan_out();
    let mut plan = create_study_plan(
        "高数",
        "bilibili",
        "BV123",
        "全集",
        &plan_out,
        "2026-09-01",
        false,
    )
    .unwrap();

    // 打卡第 1 天任务 0
    plan.schedules[0].tasks[0].completed = true;

    assert!(push_forward_plan(&mut plan, "2026-09-02").unwrap());

    let first = plan
        .schedules
        .iter()
        .find(|schedule| schedule.date == "2026-09-01")
        .unwrap();
    let second = plan
        .schedules
        .iter()
        .find(|schedule| schedule.date == "2026-09-02")
        .unwrap();
    let third = plan
        .schedules
        .iter()
        .find(|schedule| schedule.date == "2026-09-03")
        .unwrap();
    assert_eq!(first.tasks.len(), 1);
    assert!(first.tasks[0].completed);
    assert_eq!(second.tasks.len(), 1); // 只保留 9/1 未完成部分
    assert!(!second.tasks[0].completed);
    assert_eq!(third.tasks.len(), 2); // 原 9/2 日程整体后移
    assert_eq!(plan.end_date, "2026-09-03");
}

#[test]
fn reschedule_unfinished_plan_moves_only_open_tasks_and_keeps_batches() {
    let mut plan = create_study_plan(
        "高数",
        "bilibili",
        "BV123",
        "全集",
        &mock_plan_out(),
        "2026-09-01",
        false,
    )
    .unwrap();
    let completed_id = plan.schedules[0].tasks[0].id.clone();
    plan.schedules[0].tasks[0].completed = true;
    plan.schedules[0].tasks[0].completed_at = Some(100);

    assert!(reschedule_unfinished_plan(&mut plan, "2026-09-05").unwrap());
    let completed_day = plan
        .schedules
        .iter()
        .find(|schedule| schedule.date == "2026-09-01")
        .unwrap();
    assert_eq!(completed_day.tasks.len(), 1);
    assert_eq!(completed_day.tasks[0].id, completed_id);
    assert!(completed_day.tasks[0].completed);
    assert_eq!(
        plan.schedules
            .iter()
            .find(|schedule| schedule.date == "2026-09-05")
            .unwrap()
            .tasks
            .len(),
        1
    );
    assert_eq!(
        plan.schedules
            .iter()
            .find(|schedule| schedule.date == "2026-09-06")
            .unwrap()
            .tasks
            .len(),
        2
    );
    assert_eq!(
        plan.schedules
            .iter()
            .flat_map(|schedule| &schedule.tasks)
            .count(),
        4
    );
    assert_eq!(plan.end_date, "2026-09-06");
}

#[test]
fn reschedule_unfinished_plan_can_move_backward_and_detaches_old_restore_history() {
    let mut plan = create_study_plan(
        "高数",
        "bilibili",
        "BV123",
        "全集",
        &mock_plan_out(),
        "2026-09-01",
        false,
    )
    .unwrap();
    plan.schedules[0].tasks[0].completed = true;
    let unfinished_id = plan.schedules[0].tasks[1].id.clone();
    plan.schedules[0].tasks[1].advanced_from_date = Some("2026-09-08".to_string());
    plan.advance_shifts
        .push(crate::schedule_recovery::ScheduleShift {
            trigger_task_ids: vec![unfinished_id.clone()],
            moves: vec![crate::schedule_recovery::TaskDateMove {
                task_id: unfinished_id.clone(),
                from: "2026-09-08".to_string(),
                to: "2026-09-01".to_string(),
            }],
            date_slots: vec![("2026-09-08".to_string(), "2026-09-01".to_string())],
        });

    assert!(reschedule_unfinished_plan(&mut plan, "2026-08-30").unwrap());
    let moved = plan
        .schedules
        .iter()
        .find(|schedule| schedule.date == "2026-08-30")
        .unwrap()
        .tasks
        .iter()
        .find(|task| task.id == unfinished_id)
        .unwrap();
    assert!(moved.advanced_from_date.is_none());
    assert!(!moved.advance_restored);
    assert!(plan.advance_shifts.is_empty());
    assert!(plan.schedules.iter().any(|schedule| {
        schedule.date == "2026-09-01" && schedule.tasks.iter().any(|task| task.completed)
    }));
    assert!(plan
        .schedules
        .iter()
        .any(|schedule| schedule.date == "2026-08-31"));
}

#[test]
fn reschedule_unfinished_plan_is_noop_without_a_new_start_or_open_tasks() {
    let mut plan = create_study_plan(
        "高数",
        "bilibili",
        "BV123",
        "全集",
        &mock_plan_out(),
        "2026-09-01",
        false,
    )
    .unwrap();
    assert!(!reschedule_unfinished_plan(&mut plan, "2026-09-01").unwrap());
    for task in plan
        .schedules
        .iter_mut()
        .flat_map(|schedule| &mut schedule.tasks)
    {
        task.completed = true;
    }
    assert!(!reschedule_unfinished_plan(&mut plan, "2026-09-10").unwrap());
    assert!(reschedule_unfinished_plan(&mut plan, "not-a-date").is_err());
}

#[test]
fn multi_plan_superposition_and_stats_test() {
    let plan_out = mock_plan_out();
    let plan1 = create_study_plan(
        "高数",
        "bilibili",
        "BV1",
        "全集",
        &plan_out,
        "2026-09-01",
        false,
    )
    .unwrap();
    let plan2 = create_study_plan(
        "计网",
        "jellyfin",
        "item_123",
        "全集",
        &plan_out,
        "2026-09-01",
        false,
    )
    .unwrap();

    let mut plans = vec![plan1, plan2];

    // 9月1日应聚合两个科目的任务（每个科目2项，共4项）
    let today_tasks = get_tasks_for_date(&plans, "2026-09-01");
    assert_eq!(today_tasks.len(), 4);
    assert_eq!(today_tasks[0].plan_title, "高数");
    assert_eq!(today_tasks[2].plan_title, "计网");

    // 一键打卡高数第1天全部任务
    let pid0 = plans[0].id.clone();
    checkin_entire_day(&mut plans, &pid0, "2026-09-01").unwrap();
    assert!(plans[0].schedules[0].tasks[0].completed);
    assert!(plans[0].schedules[0].tasks[1].completed);

    // 统计信息
    let stats = compute_study_stats(&plans, "2026-09-01");
    assert_eq!(stats.active_plans, 2);
    assert_eq!(stats.today_total_tasks, 4);
    assert_eq!(stats.today_completed_tasks, 2);
    assert_eq!(stats.total_days_checked_in, 1);
    assert_eq!(stats.current_streak, 1);
}

#[test]
fn push_forward_to_today_with_partial_checkin() {
    let plan_out = mock_plan_out();
    let mut plan = create_study_plan(
        "高数",
        "bilibili",
        "BV123",
        "全集",
        &plan_out,
        "2026-08-30",
        false,
    )
    .unwrap();

    // 8月30日当天完成任务0，但任务1未完成
    plan.schedules[0].tasks[0].completed = true;

    // 8月31日开始时，将 8月30日未完成条目顺延到当天
    push_forward_plan(&mut plan, "2026-08-31").unwrap();

    // 8月30日仅保留已完成任务；未完成任务独占下一日。
    let sch_today = plan
        .schedules
        .iter()
        .find(|s| s.date == "2026-08-30")
        .unwrap();
    assert_eq!(sch_today.tasks.len(), 1);
    assert!(sch_today.tasks[0].completed);
    let next = plan
        .schedules
        .iter()
        .find(|schedule| schedule.date == "2026-08-31")
        .unwrap();
    assert_eq!(next.tasks.len(), 1);
    assert!(!next.tasks[0].completed);
    assert!(plan.schedules.iter().any(|s| s.date == "2026-09-01"));
}

#[test]
fn advance_partial_future_day_then_postpone_today_composes_correctly() {
    let plan_out = mock_plan_out();
    let mut plan = create_study_plan(
        "高数",
        "bilibili",
        "BV123",
        "全集",
        &plan_out,
        "2026-09-01",
        false,
    )
    .unwrap();
    // 今天完成 A、未完成 B；未来 9/2 提前完成 C、未完成 D。
    plan.schedules[0].tasks[0].completed = true;
    plan.schedules[1].tasks[0].completed = true;

    assert_eq!(
        advance_completed_tasks(&mut plan, "2026-09-02", "2026-09-01").unwrap(),
        1
    );
    let today = plan
        .schedules
        .iter()
        .find(|schedule| schedule.date == "2026-09-01")
        .unwrap();
    assert_eq!(today.tasks.len(), 3);
    assert_eq!(today.tasks.iter().filter(|task| task.completed).count(), 2);
    let future = plan
        .schedules
        .iter()
        .find(|schedule| schedule.date == "2026-09-02")
        .unwrap();
    assert_eq!(future.tasks.len(), 1); // 部分提前，后续日期不移动

    assert!(push_forward_plan(&mut plan, "2026-09-02").unwrap());
    let today = plan
        .schedules
        .iter()
        .find(|schedule| schedule.date == "2026-09-01")
        .unwrap();
    assert_eq!(today.tasks.len(), 2); // A、C 的完成记录都留在今天
    let next = plan
        .schedules
        .iter()
        .find(|schedule| schedule.date == "2026-09-02")
        .unwrap();
    assert_eq!(next.tasks.len(), 1); // 只放今天剩余的 B
    let day_after = plan
        .schedules
        .iter()
        .find(|schedule| schedule.date == "2026-09-03")
        .unwrap();
    assert_eq!(day_after.tasks.len(), 1); // 原 9/2 剩余 D 后移
}

#[test]
fn advance_full_future_day_shifts_later_schedule_one_day_earlier() {
    let mut plan = create_custom_study_plan("背单词", "2026-09-01", 3, 30, false).unwrap();
    plan.schedules[1].tasks[0].completed = true;
    assert_eq!(
        advance_completed_tasks(&mut plan, "2026-09-02", "2026-09-01").unwrap(),
        1
    );
    let today = plan
        .schedules
        .iter()
        .find(|schedule| schedule.date == "2026-09-01")
        .unwrap();
    assert_eq!(today.tasks.len(), 2);
    let shifted = plan
        .schedules
        .iter()
        .find(|schedule| schedule.date == "2026-09-02")
        .unwrap();
    assert_eq!(shifted.tasks.len(), 1); // 原 9/3 日程提前到 9/2
    assert_eq!(plan.end_date, "2026-09-02");
}

#[test]
fn cancelling_an_advanced_checkin_restores_its_original_date() {
    let plan_out = mock_plan_out();
    let mut plan = create_study_plan(
        "高数",
        "bilibili",
        "BV123",
        "全集",
        &plan_out,
        "2026-09-01",
        false,
    )
    .unwrap();
    plan.schedules[1].tasks[0].completed = true;
    let advanced_task_id = plan.schedules[1].tasks[0].id.clone();
    assert_eq!(
        advance_completed_tasks(&mut plan, "2026-09-02", "2026-09-01").unwrap(),
        1
    );
    let today = plan
        .schedules
        .iter()
        .find(|schedule| schedule.date == "2026-09-01")
        .unwrap();
    assert!(today
        .tasks
        .iter()
        .any(|task| task.id == advanced_task_id && task.completed));

    let plan_id = plan.id.clone();
    let mut plans = vec![plan];
    assert!(!toggle_task_checkin(&mut plans, &plan_id, &advanced_task_id).unwrap());
    let today = plans[0]
        .schedules
        .iter()
        .find(|schedule| schedule.date == "2026-09-01")
        .unwrap();
    assert!(!today.tasks.iter().any(|task| task.id == advanced_task_id));
    let original = plans[0]
        .schedules
        .iter()
        .find(|schedule| schedule.date == "2026-09-02")
        .unwrap();
    let restored = original
        .tasks
        .iter()
        .find(|task| task.id == advanced_task_id)
        .unwrap();
    assert!(!restored.completed);
    assert_eq!(restored.advanced_from_date, None);
}

#[test]
fn advance_and_postpone_leave_other_plans_unchanged() {
    let mut missed = create_custom_study_plan("计划B", "2026-09-01", 2, 30, false).unwrap();
    let mut other = create_custom_study_plan("计划D", "2026-09-02", 1, 45, false).unwrap();
    let other_before = other.clone();

    assert!(push_forward_plan(&mut missed, "2026-09-02").unwrap());
    assert!(!push_forward_plan(&mut other, "2026-09-02").unwrap());
    assert_eq!(other, other_before);

    let mut early = create_custom_study_plan("计划C", "2026-09-01", 3, 20, false).unwrap();
    early.schedules[1].tasks[0].completed = true;
    assert_eq!(
        advance_completed_tasks(&mut early, "2026-09-02", "2026-09-01").unwrap(),
        1
    );
    assert_eq!(
        advance_completed_tasks(&mut other, "2026-09-02", "2026-09-01").unwrap(),
        0
    );
    assert_eq!(other, other_before);
}

#[test]
fn streak_with_skip_weekends() {
    let plan_out = mock_plan_out();
    // 2026-08-28 是周五，08-29 周六，08-30 周日，08-31 周一
    let mut plan = create_study_plan(
        "高数",
        "bilibili",
        "BV123",
        "全集",
        &plan_out,
        "2026-08-28",
        true, // 跳过周末
    )
    .unwrap();

    // 周五 (08-28) 打卡
    plan.schedules[0].tasks[0].completed = true;

    let plans = vec![plan];

    // 在周日 08-30 查询 streak（周五已打卡，周末休息），连续打卡应当为 1
    let stats_sunday = compute_study_stats(&plans, "2026-08-30");
    assert_eq!(stats_sunday.current_streak, 1);

    // 在周一 08-31（尚未打卡）查询 streak，应追溯到周五，连续打卡应当仍为 1
    let stats_monday = compute_study_stats(&plans, "2026-08-31");
    assert_eq!(stats_monday.current_streak, 1);
}

#[test]
fn calendar_matrix_and_month_stats_test() {
    let plan_out = mock_plan_out();
    let plan = create_study_plan(
        "高数",
        "bilibili",
        "BV123",
        "全集",
        &plan_out,
        "2026-08-15",
        false,
    )
    .unwrap();

    let plans = vec![plan];
    let matrix = generate_month_calendar_matrix(2026, 8, &plans);

    // 2026年8月网格行数应为 5 或 6 周（35 或 42 格）
    assert!(matrix.len() == 35 || matrix.len() == 42);

    // 验证 8月15日 当天包含高数的任务
    let day_15 = matrix.iter().find(|d| d.date == "2026-08-15").unwrap();
    assert_eq!(day_15.total_tasks, 2);
    assert_eq!(day_15.total_duration, 1000);
    assert_eq!(day_15.plan_titles, vec!["高数"]);

    // 月度统计
    let month_stats = compute_month_study_stats(2026, 8, &plans);
    assert_eq!(month_stats.total_tasks, 4);
    assert_eq!(month_stats.total_duration, 2000);
    assert_eq!(month_stats.active_study_days, 2);
}
