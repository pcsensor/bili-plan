use bili_planner::schedule_recovery;
use bili_planner::study::*;
use std::collections::{BTreeMap, HashSet};

fn plan() -> StudyPlan {
    create_custom_study_plan("课程", "2026-09-01", 5, 30, false).unwrap()
}
fn id(p: &StudyPlan, date: &str) -> String {
    p.schedules.iter().find(|s| s.date == date).unwrap().tasks[0]
        .id
        .clone()
}
fn date(p: &StudyPlan, id: &str) -> String {
    p.schedules
        .iter()
        .find(|s| s.tasks.iter().any(|t| t.id == id))
        .unwrap()
        .date
        .clone()
}
fn dates(p: &StudyPlan) -> BTreeMap<String, String> {
    p.schedules
        .iter()
        .flat_map(|s| s.tasks.iter().map(|t| (t.id.clone(), s.date.clone())))
        .collect()
}
fn toggle(p: &mut StudyPlan, id: &str) {
    let pid = p.id.clone();
    toggle_task_checkin(std::slice::from_mut(p), &pid, id).unwrap();
}
fn advance(p: &mut StudyPlan, date: &str) {
    let pid = p.id.clone();
    checkin_entire_day(std::slice::from_mut(p), &pid, date).unwrap();
    advance_completed_tasks(p, date, "2026-09-01").unwrap();
}
fn invariant(p: &StudyPlan, original: &StudyPlan) {
    let task_ids: Vec<_> = p
        .schedules
        .iter()
        .flat_map(|s| &s.tasks)
        .map(|t| &t.id)
        .collect();
    assert_eq!(
        task_ids.len(),
        task_ids.iter().collect::<HashSet<_>>().len()
    );
    assert_eq!(
        task_ids.len(),
        original
            .schedules
            .iter()
            .map(|s| s.tasks.len())
            .sum::<usize>()
    );
    assert_eq!(p.total_duration, original.total_duration);
    let all_dates: Vec<_> = p.schedules.iter().map(|s| &s.date).collect();
    assert!(all_dates.windows(2).all(|ds| ds[0] < ds[1]));
    assert!(p
        .schedules
        .iter()
        .all(|s| s.is_rest_day == s.tasks.is_empty()));
}

#[test]
fn moving_last_task_earlier_compacts_and_later_move_does_not() {
    let mut p = plan();
    let original = p.clone();
    let second = id(&p, "2026-09-02");
    let third = id(&p, "2026-09-03");
    move_task_to_date(&mut p, &second, "2026-09-01").unwrap();
    assert_eq!(date(&p, &third), "2026-09-02");
    assert_eq!(p.end_date, "2026-09-04");
    let before = dates(&p);
    move_task_to_date(&mut p, &third, "2026-09-10").unwrap();
    for (task, d) in before {
        if task != third {
            assert_eq!(date(&p, &task), d);
        }
    }
    invariant(&p, &original);
}

#[test]
fn first_cancel_restores_followers_once_other_checkins_stay_today() {
    let mut p = plan();
    let second = id(&p, "2026-09-02");
    let third = id(&p, "2026-09-03");
    move_task_to_date(&mut p, &third, "2026-09-02").unwrap();
    let original = p.clone();
    let follower = id(&p, "2026-09-03");
    advance(&mut p, "2026-09-02");
    assert_eq!(date(&p, &follower), "2026-09-02");
    toggle(&mut p, &second);
    assert_eq!(date(&p, &follower), "2026-09-03");
    assert_eq!(date(&p, &third), "2026-09-01");
    toggle(&mut p, &third);
    assert_eq!(dates(&p), dates(&original));
    invariant(&p, &original);
}

#[test]
fn partial_then_remaining_advance_undoes_compaction_for_either_task() {
    for cancel_first in [true, false] {
        let mut p = plan();
        let second = id(&p, "2026-09-02");
        let third = id(&p, "2026-09-03");
        move_task_to_date(&mut p, &third, "2026-09-02").unwrap();
        let original = p.clone();
        toggle(&mut p, &second);
        advance_completed_tasks(&mut p, "2026-09-02", "2026-09-01").unwrap();
        advance(&mut p, "2026-09-02");
        let (a, b) = if cancel_first {
            (&second, &third)
        } else {
            (&third, &second)
        };
        toggle(&mut p, a);
        toggle(&mut p, b);
        assert_eq!(dates(&p), dates(&original));
        invariant(&p, &original);
    }
}

#[test]
fn consecutive_advance_batches_can_be_cancelled_in_either_order() {
    for forward in [true, false] {
        let mut p = plan();
        let original = p.clone();
        let second = id(&p, "2026-09-02");
        let third = id(&p, "2026-09-03");
        advance(&mut p, "2026-09-02");
        advance(&mut p, "2026-09-02");
        let (a, b) = if forward {
            (&second, &third)
        } else {
            (&third, &second)
        };
        toggle(&mut p, a);
        toggle(&mut p, b);
        assert_eq!(dates(&p), dates(&original));
        invariant(&p, &original);
    }
}

#[test]
fn later_manual_move_survives_undo_and_edit_preserves_return_marker() {
    let mut p = create_calendar_series("系列", "任务1", "2026-09-01", 20).unwrap();
    append_calendar_series_task(&mut p, "任务2", "2026-09-04", 20).unwrap();
    append_calendar_series_task(&mut p, "任务3", "2026-09-10", 20).unwrap();
    let second = id(&p, "2026-09-04");
    let third = id(&p, "2026-09-10");
    advance(&mut p, "2026-09-04");
    update_calendar_task(&mut p, &second, "改名称", "2026-09-01", 20).unwrap();
    move_task_to_date(&mut p, &third, "2026-09-12").unwrap();
    toggle(&mut p, &second);
    assert_eq!(date(&p, &second), "2026-09-04");
    assert_eq!(date(&p, &third), "2026-09-12");
}

#[test]
fn weekend_and_sparse_dates_compact_by_existing_slots_and_restore_exactly() {
    let mut p = create_custom_study_plan("课程", "2026-09-04", 3, 30, true).unwrap();
    let monday = id(&p, "2026-09-07");
    move_task_to_date(&mut p, &monday, "2026-09-05").unwrap();
    let original = p.clone();
    advance(&mut p, "2026-09-05");
    assert_eq!(
        p.schedules
            .iter()
            .filter(|s| s.date == "2026-09-05")
            .count(),
        1
    );
    toggle(&mut p, &monday);
    assert_eq!(dates(&p), dates(&original));
    invariant(&p, &original);
}

#[test]
fn cloud_cancel_recheck_survives_background_and_ui_merge_and_repeated_sync() {
    let mut local = plan();
    let original = local.clone();
    let second = id(&local, "2026-09-02");
    advance(&mut local, "2026-09-02");
    let snapshot = local.clone();
    let mut server = local.clone();
    schedule_recovery::toggle(&mut server, &second, 1, true).unwrap();
    schedule_recovery::toggle(&mut server, &second, 1, true).unwrap();
    let mut background = snapshot;
    schedule_recovery::merge_checkins(&mut background, &server, true);
    schedule_recovery::merge_checkins(&mut local, &background, false);
    assert_eq!(dates(&local), dates(&original));
    let task = local
        .schedules
        .iter()
        .flat_map(|s| &s.tasks)
        .find(|t| t.id == second)
        .unwrap();
    assert!(task.completed);
    assert!(!task.advance_restored);
    for _ in 0..3 {
        let mut incoming = local.clone();
        schedule_recovery::merge_checkins(&mut incoming, &server, true);
        server = incoming;
        schedule_recovery::merge_checkins(&mut local, &server, false);
        assert_eq!(dates(&local), dates(&original));
    }
}

#[test]
fn same_second_toggles_are_monotonic_and_pause_is_preserved() {
    let mut p = plan();
    let first = id(&p, "2026-09-01");
    p.status = PlanStatus::Paused;
    let mut versions = Vec::new();
    for _ in 0..4 {
        schedule_recovery::toggle(&mut p, &first, 100, false).unwrap();
        versions.push(p.schedules[0].tasks[0].updated_at);
        assert_eq!(p.status, PlanStatus::Paused);
    }
    assert!(versions.windows(2).all(|v| v[0] < v[1]));
}

#[test]
fn history_roundtrips_and_old_plans_load_without_history() {
    let mut p = plan();
    let original = p.clone();
    let second = id(&p, "2026-09-02");
    advance(&mut p, "2026-09-02");
    let mut restored: StudyPlan =
        serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
    toggle(&mut restored, &second);
    assert_eq!(dates(&restored), dates(&original));
    let mut legacy = serde_json::to_value(&original).unwrap();
    legacy.as_object_mut().unwrap().remove("advance_shifts");
    for day in legacy["schedules"].as_array_mut().unwrap() {
        for t in day["tasks"].as_array_mut().unwrap() {
            t.as_object_mut().unwrap().remove("advance_restored");
        }
    }
    let legacy: StudyPlan = serde_json::from_value(legacy).unwrap();
    assert!(legacy.advance_shifts.is_empty());
}

#[test]
fn invalid_dates_do_not_mutate_schedule() {
    let mut p = plan();
    let before = p.clone();
    assert!(push_forward_plan(&mut p, "bad").is_err());
    assert!(advance_completed_tasks(&mut p, "2026-02-30", "2026-09-01").is_err());
    assert_eq!(p, before);
}

#[test]
fn postpone_catches_all_backlog_keeps_completed_and_is_idempotent() {
    let mut p = plan();
    let original = p.clone();
    let first = id(&p, "2026-09-01");
    let second = id(&p, "2026-09-02");
    let third = id(&p, "2026-09-03");
    let fourth = id(&p, "2026-09-04");
    toggle(&mut p, &second);
    push_forward_plan(&mut p, "2026-09-04").unwrap();
    assert_eq!(date(&p, &first), "2026-09-04");
    assert_eq!(date(&p, &second), "2026-09-02");
    assert_eq!(date(&p, &third), "2026-09-05");
    assert_eq!(date(&p, &fourth), "2026-09-06");
    let before = p.clone();
    assert!(!push_forward_plan(&mut p, "2026-09-04").unwrap());
    assert_eq!(p, before);
    invariant(&p, &original);
}

#[test]
fn postpone_includes_manual_weekend_backlog_without_colliding() {
    let mut p = create_custom_study_plan("课程", "2026-09-04", 3, 30, true).unwrap();
    let monday = id(&p, "2026-09-07");
    move_task_to_date(&mut p, &monday, "2026-09-05").unwrap();
    let original = p.clone();
    push_forward_plan(&mut p, "2026-09-07").unwrap();
    assert_eq!(date(&p, &monday), "2026-09-08");
    invariant(&p, &original);
}

#[test]
fn advance_then_postpone_then_cancel_preserves_postponement() {
    let mut p = plan();
    let second = id(&p, "2026-09-02");
    let third = id(&p, "2026-09-03");
    advance(&mut p, "2026-09-02");
    push_forward_plan(&mut p, "2026-09-02").unwrap();
    toggle(&mut p, &second);
    assert_eq!(date(&p, &second), "2026-09-03");
    assert_eq!(date(&p, &third), "2026-09-04");
}

#[test]
fn three_advance_batches_restore_for_every_cancellation_order() {
    for order in [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ] {
        let mut p = plan();
        let original = p.clone();
        let tasks = [
            id(&p, "2026-09-02"),
            id(&p, "2026-09-03"),
            id(&p, "2026-09-04"),
        ];
        for _ in 0..3 {
            advance(&mut p, "2026-09-02");
        }
        for i in order {
            toggle(&mut p, &tasks[i]);
            invariant(&p, &original);
        }
        assert_eq!(dates(&p), dates(&original));
    }
}

#[test]
fn manual_series_edit_compacts_sparse_followers_like_date_adjustment() {
    let mut p = create_calendar_series("系列", "任务1", "2026-09-01", 20).unwrap();
    append_calendar_series_task(&mut p, "任务2", "2026-09-04", 20).unwrap();
    append_calendar_series_task(&mut p, "任务3", "2026-09-10", 20).unwrap();
    let second = id(&p, "2026-09-04");
    let third = id(&p, "2026-09-10");
    update_calendar_task(&mut p, &second, "任务2", "2026-09-01", 20).unwrap();
    assert_eq!(date(&p, &third), "2026-09-04");
}

#[test]
fn repeated_entire_day_checkin_keeps_original_completion_time() {
    let mut p = plan();
    let first = id(&p, "2026-09-01");
    schedule_recovery::toggle(&mut p, &first, 10, false).unwrap();
    let before = p.schedules[0].tasks[0].clone();
    let pid = p.id.clone();
    checkin_entire_day(std::slice::from_mut(&mut p), &pid, "2026-09-01").unwrap();
    assert_eq!(p.schedules[0].tasks[0], before);
}

#[test]
fn appended_tasks_never_reuse_ids_after_move_compaction_or_deletion() {
    let mut p = create_calendar_series("系列", "一", "2026-09-01", 20).unwrap();
    append_calendar_series_task(&mut p, "二", "2026-09-01", 20).unwrap();
    let first = p.schedules[0].tasks[0].id.clone();
    let second = p.schedules[0].tasks[1].id.clone();
    delete_calendar_task(&mut p, &first).unwrap();
    append_calendar_series_task(&mut p, "三", "2026-09-01", 20).unwrap();
    let ids: HashSet<_> = p.schedules[0].tasks.iter().map(|t| &t.id).collect();
    assert_eq!(ids.len(), 2);
    assert!(!ids.contains(&first));
    move_task_to_date(&mut p, &second, "2026-09-04").unwrap();
    append_calendar_series_task(&mut p, "四", "2026-09-04", 20).unwrap();
    let ids: Vec<_> = p
        .schedules
        .iter()
        .flat_map(|s| &s.tasks)
        .map(|t| &t.id)
        .collect();
    assert_eq!(ids.len(), ids.iter().collect::<HashSet<_>>().len());
}

#[test]
fn later_manual_move_does_not_destroy_slots_needed_to_undo_earlier_batch() {
    let mut p = plan();
    let second = id(&p, "2026-09-02");
    let third = id(&p, "2026-09-03");
    let fourth = id(&p, "2026-09-04");
    advance(&mut p, "2026-09-02");
    advance(&mut p, "2026-09-02");
    move_task_to_date(&mut p, &third, "2026-09-10").unwrap();
    toggle(&mut p, &second);
    assert_eq!(date(&p, &third), "2026-09-10");
    assert_eq!(date(&p, &fourth), "2026-09-03");
}
