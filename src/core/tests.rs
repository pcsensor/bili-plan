use super::*;

#[test]
fn parse_days_rejects_empty_and_nonpositive() {
    assert!(parse_days("").is_err());
    assert!(parse_days("  ").is_err());
    assert!(parse_days("abc").is_err());
    assert!(parse_days("0").is_err());
    assert!(parse_days("-3").is_err());
    assert_eq!(parse_days(" 30 ").unwrap(), 30);
}

#[test]
fn sanitize_strips_path_characters() {
    assert_eq!(sanitize("a/b:c*d?e\"f<g>h|i"), "abcdefghi");
    assert_eq!(sanitize("正常标题").len(), 12);
    assert_eq!(sanitize(&"x".repeat(100)).len(), 40);
}

#[test]
fn source_mode_roundtrip() {
    assert_eq!(SourceMode::from_index(0), SourceMode::Bilibili);
    assert_eq!(SourceMode::from_index(1), SourceMode::Jellyfin);
    assert_eq!(SourceMode::from_index(2), SourceMode::FnOs);
    // 越界索引回退到最后一个来源，不 panic。
    assert_eq!(SourceMode::from_index(9), SourceMode::FnOs);
    assert_eq!(SourceMode::Bilibili.index(), 0);
    assert_eq!(SourceMode::Jellyfin.index(), 1);
    assert_eq!(SourceMode::FnOs.index(), 2);
}

#[test]
fn fnos_credentials_roundtrip_and_legacy_defaults() {
    let cfg = AppConfig {
        fnos_server_url: "http://nas:5666".to_string(),
        fnos_username: "admin".to_string(),
        fnos_password: "secret".to_string(),
        ..Default::default()
    };
    let json = serde_json::to_string(&cfg).unwrap();
    let parsed: AppConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.fnos_server_url, "http://nas:5666");
    assert_eq!(parsed.fnos_username, "admin");
    assert_eq!(parsed.fnos_password, "secret");

    // 旧配置没有这三个键时必须能无损读入。
    let legacy: AppConfig = serde_json::from_str(r#"{"server_url":"http://jf"}"#).unwrap();
    assert!(legacy.fnos_server_url.is_empty());
    assert!(legacy.fnos_username.is_empty());
    assert!(legacy.fnos_password.is_empty());
    assert_eq!(legacy.sync_revision, 0);
}

#[test]
fn history_distinguishes_fnos_source_tag() {
    let mut cfg = AppConfig::default();
    record_history(&mut cfg, SourceMode::FnOs, "fv_abc", "飞牛剧集");
    assert_eq!(cfg.history[0].source, "fnos");
    // 同一输入在不同来源下视为两条独立记录。
    record_history(&mut cfg, SourceMode::Jellyfin, "fv_abc", "");
    assert_eq!(cfg.history.len(), 2);
}

#[test]
fn record_history_dedupes_and_prepends() {
    let mut cfg = AppConfig::default();
    record_history(&mut cfg, SourceMode::Bilibili, " BV1abc ", "合集A");
    record_history(&mut cfg, SourceMode::Jellyfin, "http://jf/?id=1", "课程B");
    assert_eq!(cfg.history.len(), 2);
    assert_eq!(cfg.history[0].source, "jellyfin");

    // 相同（来源, 输入）去重并置顶，不产生重复项。
    record_history(&mut cfg, SourceMode::Bilibili, "BV1abc", "合集A·新标题");
    assert_eq!(cfg.history.len(), 2);
    assert_eq!(cfg.history[0].title, "合集A·新标题");
    // 相同输入、不同来源视为两条。
    record_history(&mut cfg, SourceMode::Jellyfin, "BV1abc", "");
    assert_eq!(cfg.history.len(), 3);
    // 空白输入不记录。
    record_history(&mut cfg, SourceMode::Bilibili, "   ", "x");
    assert_eq!(cfg.history.len(), 3);
}

#[test]
fn record_history_caps_at_limit() {
    let total = HISTORY_LIMIT as i64 + 5;
    let mut cfg = AppConfig::default();
    for i in 0..total {
        record_history(&mut cfg, SourceMode::Bilibili, &format!("BV{i:04}"), "");
    }
    assert_eq!(cfg.history.len(), HISTORY_LIMIT);
    // 最新的留在最前，最旧的被截断。
    assert_eq!(cfg.history[0].input, format!("BV{:04}", total - 1));
    assert_eq!(
        cfg.history.last().unwrap().input,
        format!("BV{:04}", total - HISTORY_LIMIT as i64)
    );
}

#[test]
fn remove_and_clear_history() {
    let mut cfg = AppConfig::default();
    record_history(&mut cfg, SourceMode::Bilibili, "BV1", "A");
    record_history(&mut cfg, SourceMode::Bilibili, "BV2", "B");
    remove_history(&mut cfg, 0);
    assert_eq!(cfg.history.len(), 1);
    assert_eq!(cfg.history[0].input, "BV1");
    remove_history(&mut cfg, 99); // 越界静默
    assert_eq!(cfg.history.len(), 1);
    clear_history(&mut cfg);
    assert!(cfg.history.is_empty());
}

/// 旧版配置文件（只有 Jellyfin 两键）必须能无损读入，且 auto_sync 缺省为 true。
#[test]
fn legacy_config_parses_with_empty_history() {
    let cfg: AppConfig =
        serde_json::from_str(r#"{"server_url": "http://jf:8096", "token": "t"}"#).unwrap();
    assert_eq!(cfg.server_url, "http://jf:8096");
    assert!(cfg.history.is_empty());
    assert!(cfg.auto_sync);
    // 云端地址不再默认填充：缺字段时必须是空，由用户显式配置。
    assert_eq!(cfg.sync_server_url, "");
}

#[test]
fn cloud_server_url_defaults_to_empty() {
    assert_eq!(AppConfig::default().sync_server_url, "");
    let cfg: AppConfig = serde_json::from_str("{}").unwrap();
    assert_eq!(cfg.sync_server_url, "");
}

#[test]
fn bind_and_sync_fail_fast_when_cloud_server_is_unset() {
    let mut cfg = AppConfig {
        sync_server_url: "".to_string(),
        ..Default::default()
    };
    let bind_err = request_cloud_bind_code(&mut cfg).unwrap_err();
    assert!(bind_err.contains("云端服务器地址"), "{bind_err}");
    let sync_err = sync_with_cloud(&mut cfg).unwrap_err();
    assert!(sync_err.contains("云端服务器地址"), "{sync_err}");
    // 未配置地址时不得顺带注册设备。
    assert!(cfg.sync_device_token.is_none());
}

#[test]
fn auto_sync_config_roundtrip() {
    let cfg_default = AppConfig::default();
    assert!(cfg_default.auto_sync);

    let json = serde_json::to_string(&cfg_default).unwrap();
    let parsed: AppConfig = serde_json::from_str(&json).unwrap();
    assert!(parsed.auto_sync);

    let parsed_false: AppConfig = serde_json::from_str(r#"{"auto_sync": false}"#).unwrap();
    assert!(!parsed_false.auto_sync);
}

#[test]
fn daily_notes_migrate_from_legacy_single_string_format() {
    let cfg: AppConfig =
        serde_json::from_str(r#"{"daily_notes":{"2026-09-03":"旧版单条备注"}}"#).unwrap();
    let notes = get_daily_notes(&cfg, "2026-09-03");
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].content, "旧版单条备注");
}

#[test]
fn local_sqlite_roundtrip_keeps_all_desktop_data() {
    let dir = std::env::temp_dir().join(format!("bili_planner_local_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let db_path = dir.join("config.sqlite3");
    let mut cfg = AppConfig {
        server_url: "https://media.example.com".to_string(),
        token: "jf-token".to_string(),
        sync_revision: 7,
        ..Default::default()
    };
    record_history(&mut cfg, SourceMode::Bilibili, "BV1test", "测试合集");
    cfg.plans
        .push(study::create_custom_study_plan("背单词", "2026-09-03", 3, 30, false).unwrap());
    let plan_id = cfg.plans[0].id.clone();
    study::checkin_entire_day(&mut cfg.plans, &plan_id, "2026-09-04").unwrap();
    study::advance_completed_tasks(&mut cfg.plans[0], "2026-09-04", "2026-09-03").unwrap();
    assert!(!cfg.plans[0].advance_shifts.is_empty());
    study::add_daily_note(&mut cfg.daily_notes, "2026-09-03", "完成第一组单词").unwrap();

    LocalConfigStore::save(&db_path, &cfg).unwrap();
    let loaded = LocalConfigStore::load(&db_path).unwrap();
    assert_eq!(loaded.server_url, cfg.server_url);
    assert_eq!(loaded.sync_revision, 7);
    assert_eq!(loaded.history, cfg.history);
    assert_eq!(loaded.plans, cfg.plans);
    assert_eq!(
        study::get_daily_notes(&loaded.daily_notes, "2026-09-03").len(),
        1
    );
}

#[test]
fn local_json_is_imported_to_sqlite_and_kept_as_backup() {
    let dir = std::env::temp_dir().join(format!("bili_planner_migration_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let db_path = dir.join("config.sqlite3");
    let json_path = dir.join("legacy.json");
    let cfg = AppConfig {
        sync_device_token: Some("device-token".to_string()),
        ..Default::default()
    };
    std::fs::write(&json_path, serde_json::to_string(&cfg).unwrap()).unwrap();

    let imported = load_config_at(&db_path, &json_path).unwrap();
    assert_eq!(imported.sync_device_token, cfg.sync_device_token);
    assert!(db_path.exists());
    assert!(json_path.exists());
    assert_eq!(
        LocalConfigStore::load(&db_path).unwrap().sync_device_token,
        cfg.sync_device_token
    );
}

/// 旧库里的默认云端地址在加载时清空；用户显式配置的其它地址保持不变。
#[test]
fn legacy_default_sync_server_url_is_cleared_on_load() {
    let dir = std::env::temp_dir().join(format!("bili_planner_sync_url_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
    let db_path = dir.join("config.sqlite3");
    let legacy_path = dir.join("legacy.json");
    LocalConfigStore::save(
        &db_path,
        &AppConfig {
            sync_server_url: "https://plan.pcsensor.cloud".to_string(),
            sync_device_token: Some("device-token".to_string()),
            ..Default::default()
        },
    )
    .unwrap();
    let loaded = load_config_at(&db_path, &legacy_path).unwrap();
    assert_eq!(loaded.sync_server_url, "");
    assert_eq!(loaded.sync_device_token.as_deref(), Some("device-token"));
    // 迁移结果已落盘，再次加载仍为空。
    assert_eq!(
        LocalConfigStore::load(&db_path).unwrap().sync_server_url,
        ""
    );

    let custom_db = dir.join("custom.sqlite3");
    LocalConfigStore::save(
        &custom_db,
        &AppConfig {
            sync_server_url: "https://plan.example.com".to_string(),
            ..Default::default()
        },
    )
    .unwrap();
    let kept = load_config_at(&custom_db, &legacy_path).unwrap();
    assert_eq!(kept.sync_server_url, "https://plan.example.com");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn clear_cloud_binding_state_keeps_server_url_and_plans() {
    let mut cfg = AppConfig {
        sync_server_url: "https://plan.example".to_string(),
        sync_device_token: Some("device-token".to_string()),
        sync_revision: 9,
        feishu_bound: true,
        feishu_user_name: Some("飞书用户".to_string()),
        telegram_bound: true,
        telegram_user_name: Some("tg".to_string()),
        plans: vec![study::create_custom_study_plan("背单词", "2026-09-03", 3, 30, false).unwrap()],
        ..Default::default()
    };
    clear_cloud_binding_state(&mut cfg);
    assert_eq!(cfg.sync_server_url, "https://plan.example");
    assert!(cfg.sync_device_token.is_none());
    assert_eq!(cfg.sync_revision, 0);
    assert!(!cfg.feishu_bound);
    assert!(cfg.feishu_user_name.is_none());
    assert!(!cfg.telegram_bound);
    assert!(cfg.telegram_user_name.is_none());
    assert_eq!(cfg.plans.len(), 1);
}

#[test]
fn delete_cloud_binding_clears_local_state_even_when_cloud_is_unreachable() {
    let mut cfg = AppConfig {
        // 保证连接被立刻拒绝，模拟云端失联。
        sync_server_url: "http://127.0.0.1:1".to_string(),
        sync_device_token: Some("device-token".to_string()),
        sync_revision: 4,
        feishu_bound: true,
        feishu_user_name: Some("飞书用户".to_string()),
        telegram_bound: true,
        telegram_user_name: Some("tg".to_string()),
        plans: vec![study::create_custom_study_plan("背单词", "2026-09-03", 3, 30, false).unwrap()],
        ..Default::default()
    };
    let report = delete_cloud_binding(&mut cfg);
    assert!(!report.remote_cleaned);
    assert!(report.message.contains("已解除本地绑定"));
    assert!(cfg.sync_device_token.is_none());
    assert_eq!(cfg.sync_revision, 0);
    assert!(!cfg.feishu_bound);
    assert!(!cfg.telegram_bound);
    assert_eq!(cfg.sync_server_url, "http://127.0.0.1:1");
    assert_eq!(cfg.plans.len(), 1);
}
