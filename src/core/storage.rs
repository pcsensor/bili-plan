use super::*;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::path::{Path, PathBuf};

/// 本机数据库路径：用户家目录下 `.bili-planner.sqlite3`。
/// 旧版 JSON 路径仅用于一次性迁移，迁移后不再作为运行时数据源。
fn config_db_path() -> Option<PathBuf> {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let home = std::env::var(key).ok()?;
    Some(PathBuf::from(home).join(".bili-planner.sqlite3"))
}

fn legacy_config_path() -> Option<PathBuf> {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let home = std::env::var(key).ok()?;
    Some(PathBuf::from(home).join(".bili-planner.json"))
}

/// 本地数据库表。配置标量、历史、计划、备注分表保存，计划和备注保留其
/// serde JSON 数据契约，避免 UI 领域模型与存储模式耦合。
pub(super) struct LocalConfigStore;

impl LocalConfigStore {
    fn open(path: &Path) -> rusqlite::Result<Connection> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(3))?;
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            CREATE TABLE IF NOT EXISTS app_meta (
                id INTEGER PRIMARY KEY CHECK(id = 1),
                payload_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS history (
                position INTEGER PRIMARY KEY,
                input TEXT NOT NULL,
                source TEXT NOT NULL,
                title TEXT NOT NULL,
                at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS plans (
                plan_id TEXT PRIMARY KEY,
                position INTEGER NOT NULL,
                payload_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS daily_notes (
                date TEXT NOT NULL,
                note_id TEXT NOT NULL,
                position INTEGER NOT NULL,
                payload_json TEXT NOT NULL,
                PRIMARY KEY(date, note_id)
            );
            CREATE INDEX IF NOT EXISTS idx_daily_notes_date_position
                ON daily_notes(date, position);
            ",
        )?;
        Ok(conn)
    }

    fn metadata(cfg: &AppConfig) -> AppConfig {
        let mut metadata = cfg.clone();
        metadata.history.clear();
        metadata.plans.clear();
        metadata.daily_notes.clear();
        metadata
    }

    fn write_history(tx: &Transaction<'_>, history: &[HistoryEntry]) -> rusqlite::Result<()> {
        tx.execute("DELETE FROM history", [])?;
        for (position, entry) in history.iter().enumerate() {
            tx.execute(
                "INSERT INTO history(position, input, source, title, at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![position as i64, entry.input, entry.source, entry.title, entry.at],
            )?;
        }
        Ok(())
    }

    fn write_plans(tx: &Transaction<'_>, plans: &[StudyPlan]) -> rusqlite::Result<()> {
        tx.execute("DELETE FROM plans", [])?;
        for (position, plan) in plans.iter().enumerate() {
            let payload = serde_json::to_string(plan).unwrap_or_else(|_| "{}".to_string());
            tx.execute(
                "INSERT INTO plans(plan_id, position, payload_json) VALUES (?1, ?2, ?3)",
                params![plan.id, position as i64, payload],
            )?;
        }
        Ok(())
    }

    fn write_notes(tx: &Transaction<'_>, notes: &DailyNotes) -> rusqlite::Result<()> {
        tx.execute("DELETE FROM daily_notes", [])?;
        for (date, entries) in notes {
            for (position, note) in entries.iter().enumerate() {
                let payload = serde_json::to_string(note).unwrap_or_else(|_| "{}".to_string());
                tx.execute(
                    "INSERT INTO daily_notes(date, note_id, position, payload_json) VALUES (?1, ?2, ?3, ?4)",
                    params![date, note.id, position as i64, payload],
                )?;
            }
        }
        Ok(())
    }

    pub(super) fn save(path: &Path, cfg: &AppConfig) -> rusqlite::Result<()> {
        let mut conn = Self::open(path)?;
        let tx = conn.transaction()?;
        let metadata =
            serde_json::to_string(&Self::metadata(cfg)).unwrap_or_else(|_| "{}".to_string());
        tx.execute(
            "INSERT INTO app_meta(id, payload_json) VALUES (1, ?1)
             ON CONFLICT(id) DO UPDATE SET payload_json = excluded.payload_json",
            [metadata],
        )?;
        Self::write_history(&tx, &cfg.history)?;
        Self::write_plans(&tx, &cfg.plans)?;
        Self::write_notes(&tx, &cfg.daily_notes)?;
        tx.commit()
    }

    pub(super) fn load(path: &Path) -> Option<AppConfig> {
        let conn = Self::open(path).ok()?;
        let metadata: String = conn
            .query_row(
                "SELECT payload_json FROM app_meta WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .ok()??;
        let mut cfg: AppConfig = serde_json::from_str(&metadata).ok()?;

        let mut history = Vec::new();
        let mut history_statement = conn
            .prepare("SELECT input, source, title, at FROM history ORDER BY position ASC")
            .ok()?;
        let rows = history_statement
            .query_map([], |row| {
                Ok(HistoryEntry {
                    input: row.get(0)?,
                    source: row.get(1)?,
                    title: row.get(2)?,
                    at: row.get(3)?,
                })
            })
            .ok()?;
        history.extend(rows.filter_map(Result::ok));
        cfg.history = history;

        let mut plans = Vec::new();
        let mut plans_statement = conn
            .prepare("SELECT payload_json FROM plans ORDER BY position ASC")
            .ok()?;
        let rows = plans_statement
            .query_map([], |row| row.get::<_, String>(0))
            .ok()?;
        plans.extend(
            rows.filter_map(Result::ok)
                .filter_map(|payload| serde_json::from_str(&payload).ok()),
        );
        cfg.plans = plans;

        let mut notes = DailyNotes::new();
        let mut notes_statement = conn
            .prepare("SELECT date, payload_json FROM daily_notes ORDER BY date ASC, position ASC")
            .ok()?;
        let rows = notes_statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .ok()?;
        for row in rows.filter_map(Result::ok) {
            if let Ok(note) = serde_json::from_str::<DailyNote>(&row.1) {
                notes.entry(row.0).or_default().push(note);
            }
        }
        cfg.daily_notes = notes;
        Some(cfg)
    }
}

/// 加载给定路径的 SQLite；数据库不存在或尚未初始化时导入旧 JSON。
pub(super) fn load_config_at(db_path: &Path, legacy_path: &Path) -> Option<AppConfig> {
    if db_path.exists() {
        if let Some(config) = LocalConfigStore::load(db_path) {
            return Some(config);
        }
    }

    let legacy_data = std::fs::read_to_string(legacy_path).ok()?;
    let config: AppConfig = serde_json::from_str(&legacy_data).ok()?;
    LocalConfigStore::save(db_path, &config).ok()?;
    Some(config)
}

/// 启动时加载 SQLite。本地尚未迁移时，导入旧 JSON 并保留原文件为备份。
pub fn load_config() -> Option<AppConfig> {
    let db_path = config_db_path()?;
    let legacy_path = legacy_config_path()?;
    load_config_at(&db_path, &legacy_path)
}

/// 把应用配置原子写入本地 SQLite，并报告失败。
pub fn try_save_config(cfg: &AppConfig) -> Result<(), String> {
    let path = config_db_path().ok_or_else(|| "无法定位本地配置数据库".to_string())?;
    LocalConfigStore::save(&path, cfg).map_err(|error| error.to_string())
}

/// 兼容现有 UI 动作的保存入口；失败会明确输出到标准错误。
pub fn save_config(cfg: &AppConfig) {
    if let Err(error) = try_save_config(cfg) {
        eprintln!("保存本地配置失败: {error}");
    }
}
