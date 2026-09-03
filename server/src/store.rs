use crate::models::{DailyNotes, DeviceUser, StudyPlan};
use chrono::{Local, Utc};
use rand::Rng;
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tracing::{info, warn};

/// 旧版 JSON 存储格式，仅用于首次迁移到 SQLite。
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct StoreData {
    pub devices: HashMap<String, DeviceUser>,
    pub plans: HashMap<String, Vec<StudyPlan>>,
    #[serde(default)]
    pub push_logs: HashMap<String, String>,
}

/// SQLite 存储。计划与备注保存为 JSON 文档，设备、计划主键与推送去重键建立索引。
#[derive(Clone)]
pub struct Store {
    data_path: PathBuf,
    conn: Arc<Mutex<Connection>>,
}

impl Store {
    pub fn new(data_dir: impl AsRef<Path>) -> Self {
        let dir = data_dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir).expect("无法创建数据目录");
        let data_path = dir.join("store.sqlite3");
        let mut conn = Connection::open(&data_path).expect("无法打开 SQLite 数据库");
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .expect("无法配置 SQLite busy timeout");
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;
            CREATE TABLE IF NOT EXISTS devices (
                device_token TEXT PRIMARY KEY,
                feishu_open_id TEXT,
                feishu_user_name TEXT,
                telegram_chat_id INTEGER,
                telegram_user_name TEXT,
                bind_code TEXT,
                bind_code_expires_at INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_devices_feishu_open_id ON devices(feishu_open_id);
            CREATE INDEX IF NOT EXISTS idx_devices_telegram_chat_id ON devices(telegram_chat_id);
            CREATE TABLE IF NOT EXISTS plans (
                device_token TEXT NOT NULL,
                plan_id TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                PRIMARY KEY(device_token, plan_id),
                FOREIGN KEY(device_token) REFERENCES devices(device_token) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS daily_notes (
                device_token TEXT PRIMARY KEY,
                payload_json TEXT NOT NULL,
                FOREIGN KEY(device_token) REFERENCES devices(device_token) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS push_logs (
                log_key TEXT PRIMARY KEY,
                pushed_at TEXT NOT NULL
            );
            ",
        )
        .expect("无法初始化 SQLite 表结构");
        Self::migrate_legacy_json(&mut conn, &dir);
        Self {
            data_path,
            conn: Arc::new(Mutex::new(conn)),
        }
    }

    /// 自动导入首次发现的旧 `store.json`；原文件保留作备份。
    fn migrate_legacy_json(conn: &mut Connection, dir: &Path) {
        let device_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM devices", [], |row| row.get(0))
            .unwrap_or(0);
        let legacy_path = dir.join("store.json");
        if device_count != 0 || !legacy_path.exists() {
            return;
        }
        let Ok(raw) = fs::read_to_string(&legacy_path) else {
            return;
        };
        let Ok(legacy) = serde_json::from_str::<StoreData>(&raw) else {
            warn!("检测到旧 store.json，但内容无效，跳过 SQLite 迁移");
            return;
        };
        let Ok(tx) = conn.transaction() else {
            warn!("无法开始旧数据迁移事务");
            return;
        };
        for device in legacy.devices.values() {
            if Self::upsert_device_tx(&tx, device).is_err() {
                warn!("迁移设备失败，已回滚 SQLite 迁移");
                return;
            }
        }
        for (token, plans) in legacy.plans {
            if Self::ensure_device_tx(&tx, &token).is_err()
                || Self::save_plans_tx(&tx, &token, &plans).is_err()
            {
                warn!("迁移计划失败，已回滚 SQLite 迁移");
                return;
            }
        }
        for (key, pushed_at) in legacy.push_logs {
            if tx
                .execute(
                    "INSERT OR IGNORE INTO push_logs(log_key, pushed_at) VALUES (?1, ?2)",
                    params![key, pushed_at],
                )
                .is_err()
            {
                warn!("迁移推送记录失败，已回滚 SQLite 迁移");
                return;
            }
        }
        if tx.commit().is_ok() {
            info!("已将旧 store.json 迁移至 SQLite: {}", legacy_path.display());
        }
    }

    fn device_from_row(row: &Row<'_>) -> rusqlite::Result<DeviceUser> {
        Ok(DeviceUser {
            device_token: row.get(0)?,
            feishu_open_id: row.get(1)?,
            feishu_user_name: row.get(2)?,
            telegram_chat_id: row.get(3)?,
            telegram_user_name: row.get(4)?,
            bind_code: row.get(5)?,
            bind_code_expires_at: row.get(6)?,
            created_at: row.get(7)?,
        })
    }

    fn get_device(conn: &Connection, token: &str) -> Option<DeviceUser> {
        conn.query_row(
            "SELECT device_token, feishu_open_id, feishu_user_name, telegram_chat_id, telegram_user_name, bind_code, bind_code_expires_at, created_at FROM devices WHERE device_token = ?1",
            [token],
            Self::device_from_row,
        )
        .optional()
        .ok()
        .flatten()
    }

    fn get_device_by_feishu(conn: &Connection, open_id: &str) -> Option<DeviceUser> {
        conn.query_row(
            "SELECT device_token, feishu_open_id, feishu_user_name, telegram_chat_id, telegram_user_name, bind_code, bind_code_expires_at, created_at FROM devices WHERE feishu_open_id = ?1",
            [open_id],
            Self::device_from_row,
        )
        .optional()
        .ok()
        .flatten()
    }

    fn get_device_by_telegram(conn: &Connection, chat_id: i64) -> Option<DeviceUser> {
        conn.query_row(
            "SELECT device_token, feishu_open_id, feishu_user_name, telegram_chat_id, telegram_user_name, bind_code, bind_code_expires_at, created_at FROM devices WHERE telegram_chat_id = ?1",
            [chat_id],
            Self::device_from_row,
        )
        .optional()
        .ok()
        .flatten()
    }

    fn new_device(token: String) -> DeviceUser {
        DeviceUser {
            device_token: token,
            feishu_open_id: None,
            feishu_user_name: None,
            telegram_chat_id: None,
            telegram_user_name: None,
            bind_code: None,
            bind_code_expires_at: 0,
            created_at: Local::now().to_rfc3339(),
        }
    }

    fn upsert_device_tx(tx: &Transaction<'_>, user: &DeviceUser) -> rusqlite::Result<()> {
        tx.execute(
            "INSERT INTO devices (device_token, feishu_open_id, feishu_user_name, telegram_chat_id, telegram_user_name, bind_code, bind_code_expires_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(device_token) DO UPDATE SET
               feishu_open_id=excluded.feishu_open_id,
               feishu_user_name=excluded.feishu_user_name,
               telegram_chat_id=excluded.telegram_chat_id,
               telegram_user_name=excluded.telegram_user_name,
               bind_code=excluded.bind_code,
               bind_code_expires_at=excluded.bind_code_expires_at",
            params![
                user.device_token,
                user.feishu_open_id,
                user.feishu_user_name,
                user.telegram_chat_id,
                user.telegram_user_name,
                user.bind_code,
                user.bind_code_expires_at,
                user.created_at,
            ],
        )?;
        Ok(())
    }

    fn ensure_device_tx(tx: &Transaction<'_>, token: &str) -> rusqlite::Result<()> {
        let user = Self::new_device(token.to_string());
        tx.execute(
            "INSERT OR IGNORE INTO devices (device_token, feishu_open_id, feishu_user_name, telegram_chat_id, telegram_user_name, bind_code, bind_code_expires_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                user.device_token,
                user.feishu_open_id,
                user.feishu_user_name,
                user.telegram_chat_id,
                user.telegram_user_name,
                user.bind_code,
                user.bind_code_expires_at,
                user.created_at,
            ],
        )?;
        Ok(())
    }

    fn load_plans(conn: &Connection, token: &str) -> Vec<StudyPlan> {
        let Ok(mut statement) = conn.prepare(
            "SELECT payload_json FROM plans WHERE device_token = ?1 ORDER BY rowid ASC",
        ) else {
            return Vec::new();
        };
        let Ok(rows) = statement.query_map([token], |row| row.get::<_, String>(0)) else {
            return Vec::new();
        };
        rows.filter_map(Result::ok)
            .filter_map(|json| serde_json::from_str(&json).ok())
            .collect()
    }

    fn save_plans_tx(tx: &Transaction<'_>, token: &str, plans: &[StudyPlan]) -> rusqlite::Result<()> {
        tx.execute("DELETE FROM plans WHERE device_token = ?1", [token])?;
        for plan in plans {
            let payload = serde_json::to_string(plan).unwrap_or_else(|_| "{}".to_string());
            tx.execute(
                "INSERT INTO plans(device_token, plan_id, payload_json) VALUES (?1, ?2, ?3)",
                params![token, plan.id, payload],
            )?;
        }
        Ok(())
    }

    fn load_notes(conn: &Connection, token: &str) -> DailyNotes {
        conn.query_row(
            "SELECT payload_json FROM daily_notes WHERE device_token = ?1",
            [token],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
    }

    fn save_notes_tx(tx: &Transaction<'_>, token: &str, notes: &DailyNotes) -> rusqlite::Result<()> {
        let payload = serde_json::to_string(notes).unwrap_or_else(|_| "{}".to_string());
        tx.execute(
            "INSERT INTO daily_notes(device_token, payload_json) VALUES (?1, ?2)
             ON CONFLICT(device_token) DO UPDATE SET payload_json = excluded.payload_json",
            params![token, payload],
        )?;
        Ok(())
    }

    fn merge_notes(server: &mut DailyNotes, incoming: DailyNotes) {
        for (date, incoming_items) in incoming {
            let server_items = server.entry(date).or_default();
            for incoming_note in incoming_items {
                match server_items.iter().position(|note| note.id == incoming_note.id) {
                    Some(index) if incoming_note.updated_at > server_items[index].updated_at => {
                        server_items[index] = incoming_note;
                    }
                    Some(_) => {}
                    None => server_items.push(incoming_note),
                }
            }
        }
    }

    pub async fn get_or_create_device(&self, device_token: Option<&str>) -> DeviceUser {
        let mut conn = self.conn.lock().expect("SQLite mutex poisoned");
        if let Some(token) = device_token {
            if let Some(user) = Self::get_device(&conn, token) {
                return user;
            }
        }
        let user = Self::new_device(format!("{:x}{:x}", rand::random::<u64>(), rand::random::<u64>()));
        let tx = conn.transaction().expect("无法写入设备");
        Self::upsert_device_tx(&tx, &user).expect("无法写入设备");
        tx.commit().expect("无法提交设备");
        user
    }

    /// 为设备生成 6 位绑定验证码（有效期 10 分钟）。
    pub async fn generate_bind_code(&self, device_token: &str) -> Option<String> {
        let mut conn = self.conn.lock().expect("SQLite mutex poisoned");
        let mut user = Self::get_device(&conn, device_token)?;
        let code = rand::thread_rng().gen_range(100_000..999_999).to_string();
        user.bind_code = Some(code.clone());
        user.bind_code_expires_at = Utc::now().timestamp() + 600;
        let tx = conn.transaction().ok()?;
        Self::upsert_device_tx(&tx, &user).ok()?;
        tx.commit().ok()?;
        Some(code)
    }

    pub async fn bind_by_code(&self, code: &str, open_id: &str, user_name: Option<&str>) -> Result<DeviceUser, &'static str> {
        let mut conn = self.conn.lock().map_err(|_| "SQLite 锁异常")?;
        let now = Utc::now().timestamp();
        let user = conn
            .query_row(
                "SELECT device_token, feishu_open_id, feishu_user_name, telegram_chat_id, telegram_user_name, bind_code, bind_code_expires_at, created_at FROM devices WHERE bind_code = ?1",
                [code],
                Self::device_from_row,
            )
            .optional()
            .map_err(|_| "数据库查询失败")?
            .ok_or("验证码无效或未找到对应设备")?;
        if user.bind_code_expires_at < now {
            return Err("验证码已过期，请在电脑客户端重新生成");
        }
        let mut updated = user;
        updated.feishu_open_id = Some(open_id.to_string());
        updated.feishu_user_name = user_name.map(str::to_string);
        updated.bind_code = None;
        let tx = conn.transaction().map_err(|_| "数据库写入失败")?;
        Self::upsert_device_tx(&tx, &updated).map_err(|_| "数据库写入失败")?;
        tx.commit().map_err(|_| "数据库写入失败")?;
        Ok(updated)
    }

    pub async fn bind_telegram_by_code(&self, code: &str, chat_id: i64, user_name: Option<&str>) -> Result<DeviceUser, &'static str> {
        let mut conn = self.conn.lock().map_err(|_| "SQLite 锁异常")?;
        let now = Utc::now().timestamp();
        let user = conn
            .query_row(
                "SELECT device_token, feishu_open_id, feishu_user_name, telegram_chat_id, telegram_user_name, bind_code, bind_code_expires_at, created_at FROM devices WHERE bind_code = ?1",
                [code],
                Self::device_from_row,
            )
            .optional()
            .map_err(|_| "数据库查询失败")?
            .ok_or("验证码无效或已过期，请在电脑端重新生成")?;
        if user.bind_code_expires_at <= now {
            return Err("验证码无效或已过期，请在电脑端重新生成");
        }
        let mut updated = user;
        updated.telegram_chat_id = Some(chat_id);
        updated.telegram_user_name = user_name.map(str::to_string);
        updated.bind_code = None;
        let tx = conn.transaction().map_err(|_| "数据库写入失败")?;
        Self::upsert_device_tx(&tx, &updated).map_err(|_| "数据库写入失败")?;
        tx.commit().map_err(|_| "数据库写入失败")?;
        Ok(updated)
    }

    pub async fn get_plans_by_open_id(&self, open_id: &str) -> Option<(DeviceUser, Vec<StudyPlan>)> {
        let conn = self.conn.lock().ok()?;
        let user = Self::get_device_by_feishu(&conn, open_id)?;
        Some((user.clone(), Self::load_plans(&conn, &user.device_token)))
    }

    pub async fn get_plans_by_telegram_chat_id(&self, chat_id: i64) -> Option<(DeviceUser, Vec<StudyPlan>)> {
        let conn = self.conn.lock().ok()?;
        let user = Self::get_device_by_telegram(&conn, chat_id)?;
        Some((user.clone(), Self::load_plans(&conn, &user.device_token)))
    }

    pub async fn get_all_bound_users(&self) -> Vec<(DeviceUser, Vec<StudyPlan>)> {
        let conn = match self.conn.lock() { Ok(conn) => conn, Err(_) => return Vec::new() };
        let Ok(mut statement) = conn.prepare(
            "SELECT device_token, feishu_open_id, feishu_user_name, telegram_chat_id, telegram_user_name, bind_code, bind_code_expires_at, created_at FROM devices WHERE feishu_open_id IS NOT NULL",
        ) else { return Vec::new() };
        let Ok(rows) = statement.query_map([], Self::device_from_row) else { return Vec::new() };
        rows.filter_map(Result::ok)
            .map(|user| {
                let plans = Self::load_plans(&conn, &user.device_token);
                (user, plans)
            })
            .collect()
    }

    pub async fn get_all_telegram_bound_users(&self) -> Vec<(DeviceUser, Vec<StudyPlan>)> {
        let conn = match self.conn.lock() { Ok(conn) => conn, Err(_) => return Vec::new() };
        let Ok(mut statement) = conn.prepare(
            "SELECT device_token, feishu_open_id, feishu_user_name, telegram_chat_id, telegram_user_name, bind_code, bind_code_expires_at, created_at FROM devices WHERE telegram_chat_id IS NOT NULL",
        ) else { return Vec::new() };
        let Ok(rows) = statement.query_map([], Self::device_from_row) else { return Vec::new() };
        rows.filter_map(Result::ok)
            .map(|user| {
                let plans = Self::load_plans(&conn, &user.device_token);
                (user, plans)
            })
            .collect()
    }

    /// 客户端主控排期结构；机器人打卡和备注以 `updated_at` 最后写入者胜出。
    pub async fn sync_plans(
        &self,
        device_token: &str,
        incoming_plans: Vec<StudyPlan>,
        incoming_notes: DailyNotes,
    ) -> (Vec<StudyPlan>, DailyNotes, bool, Option<String>, bool, Option<String>) {
        let mut conn = self.conn.lock().expect("SQLite mutex poisoned");
        let tx = conn.transaction().expect("无法开始同步事务");
        Self::ensure_device_tx(&tx, device_token).expect("无法创建设备");
        let user = Self::get_device(&tx, device_token).expect("设备不存在");
        let remote_plans = Self::load_plans(&tx, device_token);
        let mut remote_map: HashMap<String, StudyPlan> = remote_plans
            .into_iter()
            .map(|plan| (plan.id.clone(), plan))
            .collect();
        let mut final_plans = Vec::new();
        for mut incoming in incoming_plans {
            if let Some(existing) = remote_map.remove(&incoming.id) {
                let server_tasks = existing
                    .schedules
                    .iter()
                    .flat_map(|schedule| schedule.tasks.iter())
                    .map(|task| (task.id.as_str(), task))
                    .collect::<HashMap<_, _>>();
                for schedule in &mut incoming.schedules {
                    for task in &mut schedule.tasks {
                        if let Some(server_task) = server_tasks.get(task.id.as_str()) {
                            if server_task.updated_at > task.updated_at {
                                task.completed = server_task.completed;
                                task.completed_at = server_task.completed_at;
                                task.updated_at = server_task.updated_at;
                            }
                        }
                    }
                }
            }
            final_plans.push(incoming);
        }
        final_plans.sort_by_key(|plan| plan.created_at);
        let mut final_notes = Self::load_notes(&tx, device_token);
        Self::merge_notes(&mut final_notes, incoming_notes);
        Self::save_plans_tx(&tx, device_token, &final_plans).expect("无法保存计划");
        Self::save_notes_tx(&tx, device_token, &final_notes).expect("无法保存备注");
        tx.commit().expect("无法提交同步事务");
        (
            final_plans,
            final_notes,
            user.feishu_open_id.is_some(),
            user.feishu_user_name,
            user.telegram_chat_id.is_some(),
            user.telegram_user_name,
        )
    }

    fn toggle_task_for_token(conn: &mut Connection, token: &str, plan_id: &str, task_id: &str) -> Result<bool, &'static str> {
        let mut plans = Self::load_plans(conn, token);
        let now = Utc::now().timestamp();
        for plan in &mut plans {
            if plan.id == plan_id {
                for schedule in &mut plan.schedules {
                    for task in &mut schedule.tasks {
                        if task.id == task_id {
                            task.completed = !task.completed;
                            task.completed_at = task.completed.then_some(now);
                            task.updated_at = now;
                            let is_completed = task.completed;
                            let tx = conn.transaction().map_err(|_| "数据库写入失败")?;
                            Self::save_plans_tx(&tx, token, &plans).map_err(|_| "数据库写入失败")?;
                            tx.commit().map_err(|_| "数据库写入失败")?;
                            return Ok(is_completed);
                        }
                    }
                }
            }
        }
        Err("未找到对应计划或任务")
    }

    pub async fn toggle_task_by_open_id(&self, open_id: &str, plan_id: &str, task_id: &str) -> Result<bool, &'static str> {
        let mut conn = self.conn.lock().map_err(|_| "SQLite 锁异常")?;
        let user = Self::get_device_by_feishu(&conn, open_id).ok_or("未找到绑定设备")?;
        Self::toggle_task_for_token(&mut conn, &user.device_token, plan_id, task_id)
    }

    pub async fn toggle_task_by_telegram_chat_id(&self, chat_id: i64, plan_id: &str, task_id: &str) -> Result<bool, &'static str> {
        let mut conn = self.conn.lock().map_err(|_| "SQLite 锁异常")?;
        let user = Self::get_device_by_telegram(&conn, chat_id)
            .ok_or("未找到绑定设备，请先发送 /bind 绑定")?;
        Self::toggle_task_for_token(&mut conn, &user.device_token, plan_id, task_id)
    }

    /// 按键去重并记录推送；容量超过 500 时删除最旧的 100 条。
    pub async fn record_pushed_date(&self, open_id: &str, push_type: &str, date: &str) -> bool {
        let conn = match self.conn.lock() { Ok(conn) => conn, Err(_) => return false };
        let key = format!("{open_id}:{push_type}:{date}");
        let inserted = conn
            .execute(
                "INSERT OR IGNORE INTO push_logs(log_key, pushed_at) VALUES (?1, ?2)",
                params![key, Local::now().to_rfc3339()],
            )
            .unwrap_or(0)
            > 0;
        if inserted {
            let _ = conn.execute(
                "DELETE FROM push_logs WHERE log_key IN (
                    SELECT log_key FROM push_logs ORDER BY pushed_at ASC LIMIT
                    (SELECT MAX(COUNT(*) - 400, 0) FROM push_logs)
                )",
                [],
            );
        }
        inserted
    }

    #[allow(dead_code)]
    pub fn database_path(&self) -> &Path {
        &self.data_path
    }
}

#[cfg(test)]
mod tests {
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
            schedules: dates.iter().enumerate().map(|(i, &date)| DailySchedule {
                day_index: i,
                date: date.to_string(),
                tasks: vec![TaskItem {
                    id: format!("{id}_{i}_0"), vid_no: i as i64 + 1,
                    title: format!("第{}讲", i + 1), portion: 500, remainder: 0,
                    from_prev: false, completed: false, completed_at: None, updated_at: 0,
                }],
                is_rest_day: false,
            }).collect(),
        }
    }

    #[tokio::test]
    async fn sqlite_sync_preserves_postponed_schedules_and_notes() {
        let dir = std::env::temp_dir().join(format!("store_test_{}", rand::random::<u64>()));
        let store = Store::new(&dir);
        let initial = make_test_plan("p1", &["2026-08-25", "2026-08-26"]);
        store.sync_plans("dev_123", vec![initial], DailyNotes::new()).await;
        let mut notes = DailyNotes::new();
        notes.insert("2026-08-30".to_string(), vec![DailyNote {
            id: "n1".to_string(), content: "复习错题".to_string(),
            created_at: 10, updated_at: 10, deleted: false,
        }]);
        let (plans, synced_notes, _, _, _, _) = store
            .sync_plans("dev_123", vec![make_test_plan("p1", &["2026-08-30", "2026-08-31"])], notes)
            .await;
        assert_eq!(plans[0].schedules[0].date, "2026-08-30");
        assert_eq!(synced_notes["2026-08-30"][0].content, "复习错题");
    }

    #[tokio::test]
    async fn sqlite_telegram_bind_and_toggle() {
        let dir = std::env::temp_dir().join(format!("store_test_tg_{}", rand::random::<u64>()));
        let store = Store::new(&dir);
        let user = store.get_or_create_device(None).await;
        let token = user.device_token;
        let code = store.generate_bind_code(&token).await.unwrap();
        store.sync_plans(&token, vec![make_test_plan("p1", &["2026-08-30"])], DailyNotes::new()).await;
        store.bind_telegram_by_code(&code, 987654321, Some("tg_user")).await.unwrap();
        assert!(store.toggle_task_by_telegram_chat_id(987654321, "p1", "p1_0_0").await.unwrap());
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
}
