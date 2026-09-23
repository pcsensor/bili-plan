use super::*;
use crate::models::{DailyNote, DailySchedule, PlanStatus, TaskItem};

fn make_test_plan(id: &str, dates: &[&str]) -> StudyPlan {
    StudyPlan {
        id: id.to_string(),
        title: "测试计划".to_string(),
        source_type: "bilibili".to_string(),
        source_url: "BV123".to_string(),
        scope_desc: "全集".to_string(),
        total_duration: 1000,
        planned_days: dates.len(),
        start_date: dates.first().unwrap_or(&"2026-08-30").to_string(),
        end_date: dates.last().unwrap_or(&"2026-08-30").to_string(),
        skip_weekends: false,
        status: PlanStatus::Active,
        created_at: 100,
        schedules: dates
            .iter()
            .enumerate()
            .map(|(i, &date)| DailySchedule {
                day_index: i,
                date: date.to_string(),
                tasks: vec![TaskItem {
                    id: format!("{id}_{i}_0"),
                    vid_no: i as i64 + 1,
                    title: format!("第{}讲", i + 1),
                    portion: 500,
                    remainder: 0,
                    from_prev: false,
                    completed: false,
                    completed_at: None,
                    updated_at: 0,
                    advanced_from_date: None,
                    advance_restored: false,
                }],
                is_rest_day: false,
            })
            .collect(),
        advance_shifts: Vec::new(),
        is_series: false,
        show_in_library: true,
    }
}

#[tokio::test]
async fn stale_desktop_snapshot_cannot_delete_newer_plan() {
    let dir = std::env::temp_dir().join(format!("store_revision_{}", rand::random::<u64>()));
    let store = Store::new(&dir);
    let user = store.register_device().await.unwrap();
    let token = &user.device_token;
    let first = make_test_plan("first", &["2026-09-13"]);
    let second = make_test_plan("second", &["2026-09-14"]);
    let outcome = store
        .sync_plans_versioned(token, vec![first.clone()], DailyNotes::new(), Some(0))
        .await
        .unwrap();
    assert_eq!(outcome.revision, 1);
    let outcome = store
        .sync_plans_versioned(
            token,
            vec![first.clone(), second.clone()],
            DailyNotes::new(),
            Some(1),
        )
        .await
        .unwrap();
    assert_eq!(outcome.revision, 2);
    assert!(matches!(
        store
            .sync_plans_versioned(token, vec![first], DailyNotes::new(), Some(1))
            .await,
        Err(SyncError::StaleSnapshot)
    ));
    assert!(matches!(
        store
            .sync_plans_versioned(token, vec![], DailyNotes::new(), None)
            .await,
        Err(SyncError::StaleSnapshot)
    ));
    let conn = store.conn.lock().unwrap();
    let plans = Store::load_plans(&conn, token);
    assert_eq!(plans.len(), 2);
    assert!(plans.iter().any(|plan| plan.id == second.id));
    drop(conn);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn feishu_rebinding_moves_lookup_and_scheduler_without_losing_plans() {
    let dir = std::env::temp_dir().join(format!("store_rebind_{}", rand::random::<u64>()));
    let store = Store::new(&dir);
    for token in ["old", "current"] {
        store.register_device_with_token(token).await;
        store
            .sync_plans(
                token,
                vec![make_test_plan(token, &["2026-09-13"])],
                DailyNotes::new(),
            )
            .await
            .unwrap();
    }
    let code = store.generate_bind_code("old").await.unwrap();
    store.bind_feishu_for_test(&code, "reader").await;
    let code = store.generate_bind_code("current").await.unwrap();
    assert!(store
        .bind_by_code("invalid-code", "reader", None)
        .await
        .is_err());
    assert_eq!(
        store
            .get_plans_by_open_id("reader")
            .await
            .unwrap()
            .0
            .device_token,
        "old"
    );
    store.bind_feishu_for_test(&code, "reader").await;
    assert!(store.bind_by_code(&code, "reader", None).await.is_err());
    let (user, plans) = store.get_plans_by_open_id("reader").await.unwrap();
    assert_eq!(user.device_token, "current");
    assert_eq!(plans[0].id, "current");
    assert_eq!(store.get_all_bound_users().await.len(), 1);
    // 旧客户端继续同步也不能恢复已经解除的绑定。
    let outcome = store
        .sync_plans(
            "old",
            vec![make_test_plan("old", &["2026-09-13"])],
            DailyNotes::new(),
        )
        .await
        .unwrap();
    assert!(!outcome.feishu_bound);
    assert_eq!(
        Store::load_plans(&store.conn.lock().unwrap(), "old").len(),
        1
    );
    drop(store);
    let restarted = Store::new(&dir);
    assert_eq!(restarted.get_all_bound_users().await.len(), 1);
    assert_eq!(
        restarted
            .get_plans_by_open_id("reader")
            .await
            .unwrap()
            .0
            .device_token,
        "current"
    );
    drop(restarted);
    fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn sync_rejects_device_that_was_never_registered() {
    let dir = std::env::temp_dir().join(format!("store_unregistered_{}", rand::random::<u64>()));
    let store = Store::new(&dir);
    // 匿名令牌不得在同步路径上凭空建号，否则 /api/sync 就成了无限写入口。
    assert!(matches!(
        store
            .sync_plans(
                "made_up_token",
                vec![make_test_plan("p1", &["2026-09-13"])],
                DailyNotes::new()
            )
            .await,
        Err(SyncError::UnknownDevice)
    ));
    assert!(Store::get_device(&store.conn.lock().unwrap(), "made_up_token").is_none());

    let registered = store.register_device().await.unwrap();
    assert!(!registered.device_token.is_empty());
    assert!(store
        .sync_plans(
            &registered.device_token,
            vec![make_test_plan("p1", &["2026-09-13"])],
            DailyNotes::new()
        )
        .await
        .is_ok());
    // 服务端签发的令牌互不相同。
    assert_ne!(
        store.register_device().await.unwrap().device_token,
        registered.device_token
    );
    drop(store);
    fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn repeated_bad_bind_codes_lock_out_the_identity() {
    let dir = std::env::temp_dir().join(format!("store_bind_lock_{}", rand::random::<u64>()));
    let store = Store::new(&dir);
    let user = store.register_device().await.unwrap();
    let code = store.generate_bind_code(&user.device_token).await.unwrap();

    // 前 BIND_MAX_FAILURES-1 次只报验证码无效，第 BIND_MAX_FAILURES 次当次即锁定。
    for _ in 0..BIND_MAX_FAILURES - 1 {
        let err = store
            .bind_by_code("000000", "attacker", None)
            .await
            .unwrap_err();
        assert!(err.contains("验证码无效"), "unexpected: {err}");
    }
    let locked = store
        .bind_by_code("000000", "attacker", None)
        .await
        .unwrap_err();
    assert!(locked.contains("分钟后重试"), "unexpected: {locked}");
    // 锁定后即便给出正确验证码也被拒，直到锁定期结束。
    let still_locked = store
        .bind_by_code(&code, "attacker", None)
        .await
        .unwrap_err();
    assert!(
        still_locked.contains("分钟后重试"),
        "unexpected: {still_locked}"
    );
    // 锁定按身份计数：换一个 open_id 仍可正常绑定。
    store.bind_feishu_for_test(&code, "legit").await;
    assert_eq!(
        store
            .get_plans_by_open_id("legit")
            .await
            .unwrap()
            .0
            .device_token,
        user.device_token
    );
    // Telegram 侧独立计数，不受飞书锁定影响。
    let tg_code = store.generate_bind_code(&user.device_token).await.unwrap();
    store
        .bind_telegram_by_code(&tg_code, 424242, Some("tg"))
        .await
        .unwrap();
    drop(store);
    fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn bind_codes_are_six_digits_and_database_unique() {
    let dir =
        std::env::temp_dir().join(format!("store_bind_code_unique_{}", rand::random::<u64>()));
    let store = Store::new(&dir);
    let first = store.register_device().await.unwrap();
    let second = store.register_device().await.unwrap();
    let code = store.generate_bind_code(&first.device_token).await.unwrap();
    assert_eq!(code.len(), 6);
    assert!(code.bytes().all(|byte| byte.is_ascii_digit()));

    let conn = store.conn.lock().unwrap();
    let duplicate = conn.execute(
        "UPDATE devices SET bind_code = ?1 WHERE device_token = ?2",
        params![code, second.device_token],
    );
    assert!(duplicate.is_err());
    drop(conn);
    drop(store);
    fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn duplicate_legacy_bind_codes_are_cleared_during_migration() {
    let dir = std::env::temp_dir().join(format!(
        "store_bind_code_migration_{}",
        rand::random::<u64>()
    ));
    let store = Store::new(&dir);
    let first = store.register_device().await.unwrap();
    let second = store.register_device().await.unwrap();
    {
        let conn = store.conn.lock().unwrap();
        conn.execute("DROP INDEX idx_devices_bind_code_unique", [])
            .unwrap();
        conn.execute(
            "UPDATE devices SET bind_code = '123456', bind_code_expires_at = 4102444800",
            [],
        )
        .unwrap();
    }
    drop(store);

    let migrated = Store::new(&dir);
    let conn = migrated.conn.lock().unwrap();
    let remaining: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM devices WHERE bind_code = '123456'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(remaining, 1);
    assert!(Store::get_device(&conn, &first.device_token).is_some());
    assert!(Store::get_device(&conn, &second.device_token).is_some());
    drop(conn);
    drop(migrated);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn bind_failures_outside_the_window_do_not_accumulate() {
    let dir = std::env::temp_dir().join(format!("store_bind_window_{}", rand::random::<u64>()));
    let store = Store::new(&dir);
    let conn = store.conn.lock().unwrap();
    let now = 1_000_000;
    for _ in 0..BIND_MAX_FAILURES - 1 {
        assert_eq!(
            Store::record_bind_failure(&conn, "feishu:slow-typer", now).unwrap(),
            0
        );
    }
    assert_eq!(
        Store::record_bind_failure(
            &conn,
            "feishu:slow-typer",
            now + BIND_FAILURE_WINDOW_SECS + 1,
        )
        .unwrap(),
        0
    );
    let failures: i64 = conn
        .query_row(
            "SELECT failures FROM bind_attempts WHERE identity = ?1",
            ["feishu:slow-typer"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(failures, 1);
    drop(conn);
    drop(store);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn old_bind_attempt_schema_is_migrated() {
    let dir = std::env::temp_dir().join(format!("store_bind_schema_{}", rand::random::<u64>()));
    fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("store.sqlite3");
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE bind_attempts (
                 identity TEXT PRIMARY KEY,
                 failures INTEGER NOT NULL DEFAULT 0,
                 locked_until INTEGER NOT NULL DEFAULT 0
             );",
    )
    .unwrap();
    drop(conn);

    let store = Store::new(&dir);
    let conn = store.conn.lock().unwrap();
    let mut statement = conn.prepare("PRAGMA table_info(bind_attempts)").unwrap();
    let columns: Vec<String> = statement
        .query_map([], |row| row.get(1))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(columns.iter().any(|name| name == "last_failed_at"));
    drop(statement);
    drop(conn);
    drop(store);
    fs::remove_dir_all(dir).unwrap();
}

impl Store {
    async fn bind_feishu_for_test(&self, code: &str, open_id: &str) {
        self.bind_by_code(code, open_id, Some("test user"))
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn legacy_duplicate_feishu_bindings_are_migrated_and_constrained() {
    let dir =
        std::env::temp_dir().join(format!("store_binding_migration_{}", rand::random::<u64>()));
    let store = Store::new(&dir);
    for token in ["old", "current", "other"] {
        store.register_device_with_token(token).await;
        store
            .sync_plans(
                token,
                vec![make_test_plan(token, &["2026-09-13"])],
                DailyNotes::new(),
            )
            .await
            .unwrap();
    }
    {
        let conn = store.conn.lock().unwrap();
        conn.execute_batch(
                "DROP INDEX idx_devices_feishu_unique;
                 UPDATE devices SET feishu_open_id='reader', feishu_user_name='name', created_at='2026-09-01' WHERE device_token='old';
                 UPDATE devices SET telegram_chat_id=123 WHERE device_token='old';
                 UPDATE devices SET feishu_open_id='reader', feishu_user_name='name', created_at='2026-09-09' WHERE device_token='current';
                 UPDATE devices SET feishu_open_id='another-reader' WHERE device_token='other';"
            ).unwrap();
    }
    drop(store);
    let store = Store::new(&dir);
    assert_eq!(
        store
            .get_plans_by_open_id("reader")
            .await
            .unwrap()
            .0
            .device_token,
        "current"
    );
    assert_eq!(store.get_all_bound_users().await.len(), 2);
    {
        let conn = store.conn.lock().unwrap();
        let old = Store::get_device(&conn, "old").unwrap();
        assert!(old.feishu_open_id.is_none());
        assert!(old.feishu_user_name.is_none());
        assert_eq!(old.telegram_chat_id, Some(123));
        assert_eq!(Store::load_plans(&conn, "old").len(), 1);
        assert!(conn
            .execute(
                "UPDATE devices SET feishu_open_id='reader' WHERE device_token='old'",
                []
            )
            .is_err());
    }
    drop(store);
    fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn sqlite_sync_preserves_postponed_schedules_and_notes() {
    let dir = std::env::temp_dir().join(format!("store_test_{}", rand::random::<u64>()));
    let store = Store::new(&dir);
    let initial = make_test_plan("p1", &["2026-08-25", "2026-08-26"]);
    store.register_device_with_token("dev_123").await;
    store
        .sync_plans("dev_123", vec![initial], DailyNotes::new())
        .await
        .unwrap();
    let mut notes = DailyNotes::new();
    notes.insert(
        "2026-08-30".to_string(),
        vec![DailyNote {
            id: "n1".to_string(),
            content: "复习错题".to_string(),
            created_at: 10,
            updated_at: 10,
            deleted: false,
        }],
    );
    let outcome = store
        .sync_plans(
            "dev_123",
            vec![make_test_plan("p1", &["2026-08-30", "2026-08-31"])],
            notes,
        )
        .await
        .unwrap();
    assert_eq!(outcome.plans[0].schedules[0].date, "2026-08-30");
    assert_eq!(outcome.daily_notes["2026-08-30"][0].content, "复习错题");
}

#[tokio::test]
async fn sqlite_telegram_bind_and_toggle() {
    let dir = std::env::temp_dir().join(format!("store_test_tg_{}", rand::random::<u64>()));
    let store = Store::new(&dir);
    let user = store.register_device().await.unwrap();
    let token = user.device_token;
    let code = store.generate_bind_code(&token).await.unwrap();
    store
        .sync_plans(
            &token,
            vec![make_test_plan("p1", &["2026-08-30"])],
            DailyNotes::new(),
        )
        .await
        .unwrap();
    store
        .bind_telegram_by_code(&code, 987654321, Some("tg_user"))
        .await
        .unwrap();
    assert!(store
        .toggle_task_by_telegram_chat_id(987654321, "p1", "p1_0_0")
        .await
        .unwrap());
}

#[tokio::test]
async fn robot_cancel_restores_an_advanced_task_and_syncs_its_location() {
    let dir = std::env::temp_dir().join(format!("store_restore_{}", rand::random::<u64>()));
    let store = Store::new(&dir);
    let user = store.register_device().await.unwrap();
    let token = user.device_token;
    let code = store.generate_bind_code(&token).await.unwrap();
    store
        .bind_telegram_by_code(&code, 123456789, Some("tg_user"))
        .await
        .unwrap();

    let mut client_plan = make_test_plan("advanced", &["2026-09-01", "2026-09-02"]);
    let mut advanced = client_plan.schedules[1].tasks.remove(0);
    let task_id = advanced.id.clone();
    advanced.completed = true;
    advanced.completed_at = Some(10);
    advanced.updated_at = 10;
    advanced.advanced_from_date = Some("2026-09-02".to_string());
    client_plan.schedules[0].tasks.push(advanced);
    client_plan.schedules.remove(1);
    store
        .sync_plans(&token, vec![client_plan.clone()], DailyNotes::new())
        .await
        .unwrap();

    assert!(!store
        .toggle_task_by_telegram_chat_id(123456789, "advanced", &task_id)
        .await
        .unwrap());
    let merged = store
        .sync_plans(&token, vec![client_plan], DailyNotes::new())
        .await
        .unwrap()
        .plans;
    let restored_day = merged[0]
        .schedules
        .iter()
        .find(|schedule| schedule.date == "2026-09-02")
        .unwrap();
    let restored = restored_day
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .unwrap();
    assert!(!restored.completed);
    assert_eq!(restored.advanced_from_date.as_deref(), Some("2026-09-02"));
}

#[tokio::test]
async fn robot_cancel_full_day_restores_followers_after_stale_sync_and_restart() {
    let dir = std::env::temp_dir().join(format!("store_full_restore_{}", rand::random::<u64>()));
    let store = Store::new(&dir);
    let user = store.register_device().await.unwrap();
    let token = user.device_token;
    let code = store.generate_bind_code(&token).await.unwrap();
    store
        .bind_telegram_by_code(&code, 987123, Some("learner"))
        .await
        .unwrap();
    let mut client = make_test_plan("p", &["2026-09-01", "2026-09-02", "2026-09-03"]);
    let mut task = client.schedules[1].tasks.remove(0);
    task.completed = true;
    task.completed_at = Some(1);
    task.updated_at = 1;
    task.advanced_from_date = Some("2026-09-02".into());
    crate::schedule_recovery::compact_day(&mut client, "2026-09-02", vec![task.id.clone()]);
    client.schedules[0].tasks.push(task);
    crate::schedule_recovery::refresh(&mut client);
    store
        .sync_plans(&token, vec![client.clone()], DailyNotes::new())
        .await
        .unwrap();
    assert!(!store
        .toggle_task_by_telegram_chat_id(987123, "p", "p_1_0")
        .await
        .unwrap());
    // 同步前再次打卡：仍要把之前补位的日程还原到客户端。
    assert!(store
        .toggle_task_by_telegram_chat_id(987123, "p", "p_1_0")
        .await
        .unwrap());
    drop(store);
    let store = Store::new(&dir);
    let merged = store
        .sync_plans(&token, vec![client.clone()], DailyNotes::new())
        .await
        .unwrap()
        .plans;
    crate::schedule_recovery::merge_checkins(&mut client, &merged[0], false);
    assert_eq!(client.schedules[1].date, "2026-09-02");
    assert_eq!(client.schedules[1].tasks[0].id, "p_1_0");
    assert!(client.schedules[1].tasks[0].completed);
    assert_eq!(client.schedules[2].date, "2026-09-03");
    assert_eq!(client.schedules[2].tasks[0].id, "p_2_0");
    assert!(client.advance_shifts.is_empty());
    for _ in 0..3 {
        let merged = store
            .sync_plans(&token, vec![client.clone()], DailyNotes::new())
            .await
            .unwrap()
            .plans;
        assert_eq!(merged[0].schedules, client.schedules);
    }
    drop(store);
    fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn imports_legacy_json_once_without_deleting_backup() {
    let dir = std::env::temp_dir().join(format!("store_migration_{}", rand::random::<u64>()));
    fs::create_dir_all(&dir).unwrap();
    let mut legacy = StoreData::default();
    legacy.devices.insert(
        "legacy_device".to_string(),
        DeviceUser {
            device_token: "legacy_device".to_string(),
            feishu_open_id: Some("open_legacy".to_string()),
            feishu_user_name: Some("学习者".to_string()),
            telegram_chat_id: None,
            telegram_user_name: None,
            bind_code: None,
            bind_code_expires_at: 0,
            created_at: "2026-09-03T00:00:00+08:00".to_string(),
        },
    );
    legacy.plans.insert(
        "legacy_device".to_string(),
        vec![make_test_plan("legacy_plan", &["2026-09-03"])],
    );
    fs::write(
        dir.join("store.json"),
        serde_json::to_string(&legacy).unwrap(),
    )
    .unwrap();

    let store = Store::new(&dir);
    assert!(store.database_path().exists());
    assert!(dir.join("store.json").exists());
    let (_, plans) = store.get_plans_by_open_id("open_legacy").await.unwrap();
    assert_eq!(plans[0].id, "legacy_plan");
}
