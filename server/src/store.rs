use crate::models::{DailyNotes, DeviceUser, StudyPlan, SyncError, SyncOutcome};
use chrono::{Local, Utc};
use rand::Rng;
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tracing::{info, warn};

/// 签发令牌时主键冲突的重试次数（128 位随机令牌实际不会冲突）。
const TOKEN_ISSUE_ATTEMPTS: usize = 3;
const BIND_CODE_ISSUE_ATTEMPTS: usize = 8;

/// 绑定验证码连续失败上限。6 位码只有 90 万种取值、有效期 10 分钟，
/// 不限次数的话能私聊机器人的人可以在窗口内把码撞出来。
const BIND_MAX_FAILURES: i64 = 5;

/// 触发上限后的锁定时长（秒）。
const BIND_LOCKOUT_SECS: i64 = 900;
/// 只有此时间窗内的失败才算连续失败。
const BIND_FAILURE_WINDOW_SECS: i64 = 600;
/// 失败记录最多保留一天，并设置硬容量避免公开机器人被用来撑大数据库。
const BIND_ATTEMPT_RETENTION_SECS: i64 = 86_400;
const BIND_ATTEMPT_MAX_ROWS: i64 = 10_000;
const BIND_ATTEMPT_EVICT_ROWS: i64 = 1_000;

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
            CREATE TABLE IF NOT EXISTS bind_attempts (
                identity TEXT PRIMARY KEY,
                failures INTEGER NOT NULL DEFAULT 0,
                locked_until INTEGER NOT NULL DEFAULT 0,
                last_failed_at INTEGER NOT NULL DEFAULT 0
            );
            ",
        )
        .expect("无法初始化 SQLite 表结构");
        Self::ensure_bind_attempt_schema(&conn).expect("无法迁移绑定失败记录表");
        Self::migrate_legacy_json(&mut conn, &dir);
        Self::ensure_unique_feishu_binding(&mut conn).expect("无法迁移飞书唯一绑定约束");
        Self::ensure_unique_bind_codes(&mut conn).expect("无法迁移绑定码唯一约束");
        Self {
            data_path,
            conn: Arc::new(Mutex::new(conn)),
        }
    }

    fn ensure_bind_attempt_schema(conn: &Connection) -> rusqlite::Result<()> {
        let mut statement = conn.prepare("PRAGMA table_info(bind_attempts)")?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if !columns.iter().any(|name| name == "last_failed_at") {
            conn.execute(
                "ALTER TABLE bind_attempts ADD COLUMN last_failed_at INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_bind_attempts_last_failed_at
                 ON bind_attempts(last_failed_at);
             CREATE INDEX IF NOT EXISTS idx_bind_attempts_locked_until
                 ON bind_attempts(locked_until);",
        )
    }

    /// 兼容旧库的重复绑定：保留创建时间最新的设备，计划与其他平台绑定不变。
    fn ensure_unique_feishu_binding(conn: &mut Connection) -> rusqlite::Result<()> {
        let tx = conn.transaction()?;
        tx.execute_batch(
            "UPDATE devices SET feishu_open_id = NULL, feishu_user_name = NULL
             WHERE device_token IN (
                 SELECT device_token FROM (
                     SELECT device_token, ROW_NUMBER() OVER (
                         PARTITION BY feishu_open_id ORDER BY created_at DESC, rowid DESC
                     ) AS binding_rank
                     FROM devices WHERE feishu_open_id IS NOT NULL
                 ) WHERE binding_rank > 1
             );
             CREATE UNIQUE INDEX IF NOT EXISTS idx_devices_feishu_unique
                 ON devices(feishu_open_id) WHERE feishu_open_id IS NOT NULL;",
        )?;
        tx.commit()
    }

    /// 清除旧库中潜在的重复绑定码，然后用数据库约束保证并发签发也不会撞码。
    fn ensure_unique_bind_codes(conn: &mut Connection) -> rusqlite::Result<()> {
        let tx = conn.transaction()?;
        tx.execute_batch(
            "UPDATE devices SET bind_code = NULL, bind_code_expires_at = 0
             WHERE bind_code IS NOT NULL AND bind_code_expires_at <= unixepoch();
             UPDATE devices SET bind_code = NULL, bind_code_expires_at = 0
             WHERE rowid IN (
                 SELECT rowid FROM (
                     SELECT rowid, ROW_NUMBER() OVER (
                         PARTITION BY bind_code
                         ORDER BY bind_code_expires_at DESC, created_at DESC, rowid DESC
                     ) AS code_rank
                     FROM devices WHERE bind_code IS NOT NULL
                 ) WHERE code_rank > 1
             );
             CREATE UNIQUE INDEX IF NOT EXISTS idx_devices_bind_code_unique
                 ON devices(bind_code) WHERE bind_code IS NOT NULL;",
        )?;
        tx.commit()
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
        let Ok(mut statement) = conn
            .prepare("SELECT payload_json FROM plans WHERE device_token = ?1 ORDER BY rowid ASC")
        else {
            return Vec::new();
        };
        let Ok(rows) = statement.query_map([token], |row| row.get::<_, String>(0)) else {
            return Vec::new();
        };
        rows.filter_map(Result::ok)
            .filter_map(|json| serde_json::from_str(&json).ok())
            .collect()
    }

    fn save_plans_tx(
        tx: &Transaction<'_>,
        token: &str,
        plans: &[StudyPlan],
    ) -> rusqlite::Result<()> {
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

    fn save_notes_tx(
        tx: &Transaction<'_>,
        token: &str,
        notes: &DailyNotes,
    ) -> rusqlite::Result<()> {
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
                match server_items
                    .iter()
                    .position(|note| note.id == incoming_note.id)
                {
                    Some(index) if incoming_note.updated_at > server_items[index].updated_at => {
                        server_items[index] = incoming_note;
                    }
                    Some(_) => {}
                    None => server_items.push(incoming_note),
                }
            }
        }
    }

    /// 注册新设备并签发服务端生成的令牌。
    ///
    /// 令牌必须由服务端用 CSPRNG 发牌：若允许客户端自带令牌，`/api/sync` 就退化成一个
    /// 匿名的设备注册入口，任何人都能无限写入 devices 行。
    /// 主键冲突概率可忽略，但仍重试而非 upsert，避免覆盖已存在的设备。
    pub async fn register_device(&self) -> Result<DeviceUser, &'static str> {
        let mut conn = self.conn.lock().map_err(|_| "SQLite 锁异常")?;
        for _ in 0..TOKEN_ISSUE_ATTEMPTS {
            let user = Self::new_device(Self::generate_token());
            let tx = conn.transaction().map_err(|_| "数据库写入失败")?;
            let inserted = tx
                .execute(
                    "INSERT OR IGNORE INTO devices (device_token, feishu_open_id, feishu_user_name, telegram_chat_id, telegram_user_name, bind_code, bind_code_expires_at, created_at)
                     VALUES (?1, NULL, NULL, NULL, NULL, NULL, 0, ?2)",
                    params![user.device_token, user.created_at],
                )
                .map_err(|_| "数据库写入失败")?;
            if inserted == 1 {
                tx.commit().map_err(|_| "无法提交设备")?;
                return Ok(user);
            }
        }
        Err("无法签发设备令牌，请稍后重试")
    }

    fn generate_token() -> String {
        format!(
            "{:016x}{:016x}",
            rand::random::<u64>(),
            rand::random::<u64>()
        )
    }

    /// 按令牌查设备。不存在时返回 `None`，调用方据此回 404——不再就地创建。
    pub async fn get_device_by_token(
        &self,
        device_token: &str,
    ) -> Result<Option<DeviceUser>, &'static str> {
        let conn = self.conn.lock().map_err(|_| "SQLite 锁异常")?;
        conn.query_row(
            "SELECT device_token, feishu_open_id, feishu_user_name, telegram_chat_id, telegram_user_name, bind_code, bind_code_expires_at, created_at FROM devices WHERE device_token = ?1",
            [device_token],
            Self::device_from_row,
        )
        .optional()
        .map_err(|_| "数据库查询失败")
    }

    /// 测试专用：按指定令牌注册设备。生产路径一律走 [`Self::register_device`] 由服务端发牌。
    #[cfg(test)]
    pub async fn register_device_with_token(&self, device_token: &str) -> DeviceUser {
        let mut conn = self.conn.lock().expect("SQLite mutex poisoned");
        let tx = conn.transaction().expect("无法写入设备");
        Self::ensure_device_tx(&tx, device_token).expect("无法写入设备");
        tx.commit().expect("无法提交设备");
        Self::get_device(&conn, device_token).expect("设备不存在")
    }

    /// 为设备生成 6 位绑定验证码（有效期 10 分钟）。
    pub async fn generate_bind_code(&self, device_token: &str) -> Option<String> {
        let conn = self.conn.lock().ok()?;
        Self::get_device(&conn, device_token)?;
        let now = Utc::now().timestamp();
        conn.execute(
            "UPDATE devices SET bind_code = NULL, bind_code_expires_at = 0
             WHERE bind_code IS NOT NULL AND bind_code_expires_at <= ?1",
            [now],
        )
        .ok()?;
        for _ in 0..BIND_CODE_ISSUE_ATTEMPTS {
            let code = rand::thread_rng().gen_range(100_000..=999_999).to_string();
            match conn.execute(
                "UPDATE devices
                 SET bind_code = ?1, bind_code_expires_at = ?2
                 WHERE device_token = ?3",
                params![code, now + 600, device_token],
            ) {
                Ok(1) => return Some(code),
                Ok(_) => return None,
                Err(error)
                    if error.sqlite_error_code()
                        == Some(rusqlite::ErrorCode::ConstraintViolation) =>
                {
                    continue;
                }
                Err(_) => return None,
            }
        }
        None
    }

    pub async fn bind_by_code(
        &self,
        code: &str,
        open_id: &str,
        user_name: Option<&str>,
    ) -> Result<DeviceUser, String> {
        let mut conn = self.conn.lock().map_err(|_| "SQLite 锁异常".to_string())?;
        let now = Utc::now().timestamp();
        let identity = Self::bind_identity("feishu", open_id);
        let mut updated = Self::resolve_bind_code(&conn, &identity, code, now)?;
        updated.feishu_open_id = Some(open_id.to_string());
        updated.feishu_user_name = user_name.map(str::to_string);
        updated.bind_code = None;
        let tx = conn
            .transaction()
            .map_err(|_| "数据库写入失败".to_string())?;
        tx.execute(
            "UPDATE devices SET feishu_open_id = NULL, feishu_user_name = NULL
             WHERE feishu_open_id = ?1 AND device_token != ?2",
            params![open_id, updated.device_token],
        )
        .map_err(|_| "数据库写入失败".to_string())?;
        Self::upsert_device_tx(&tx, &updated).map_err(|_| "数据库写入失败".to_string())?;
        Self::clear_bind_attempts_tx(&tx, &identity).map_err(|_| "数据库写入失败".to_string())?;
        tx.commit().map_err(|_| "数据库写入失败".to_string())?;
        Ok(updated)
    }

    pub async fn bind_telegram_by_code(
        &self,
        code: &str,
        chat_id: i64,
        user_name: Option<&str>,
    ) -> Result<DeviceUser, String> {
        let mut conn = self.conn.lock().map_err(|_| "SQLite 锁异常".to_string())?;
        let now = Utc::now().timestamp();
        let identity = Self::bind_identity("telegram", &chat_id.to_string());
        let mut updated = Self::resolve_bind_code(&conn, &identity, code, now)?;
        updated.telegram_chat_id = Some(chat_id);
        updated.telegram_user_name = user_name.map(str::to_string);
        updated.bind_code = None;
        let tx = conn
            .transaction()
            .map_err(|_| "数据库写入失败".to_string())?;
        Self::upsert_device_tx(&tx, &updated).map_err(|_| "数据库写入失败".to_string())?;
        Self::clear_bind_attempts_tx(&tx, &identity).map_err(|_| "数据库写入失败".to_string())?;
        tx.commit().map_err(|_| "数据库写入失败".to_string())?;
        Ok(updated)
    }

    /// 失败计数的键是机器人身份（飞书 open_id / Telegram chat_id）而不是验证码本身，
    /// 这样攻击者每换一个码重试也照样累计到锁定。
    fn bind_identity(platform: &str, id: &str) -> String {
        format!("{platform}:{id}")
    }

    /// 校验绑定码并施加失败锁定，成功时返回待写入的设备。
    ///
    /// 过期码与不存在的码走同一条错误消息：区分两者会留下"这个码曾经签发过"的探测
    /// oracle，而攻击者撞码时本就无需知道具体原因。
    fn resolve_bind_code(
        conn: &Connection,
        identity: &str,
        code: &str,
        now: i64,
    ) -> Result<DeviceUser, String> {
        if let Some(remaining) = Self::bind_lockout_remaining(conn, identity, now)? {
            return Err(Self::lockout_message(remaining));
        }
        if code.len() != 6 || !code.bytes().all(|byte| byte.is_ascii_digit()) {
            let lockout = Self::record_bind_failure(conn, identity, now)?;
            return Err(Self::bind_failure_message(lockout));
        }
        let found = conn
            .query_row(
                "SELECT device_token, feishu_open_id, feishu_user_name, telegram_chat_id, telegram_user_name, bind_code, bind_code_expires_at, created_at FROM devices WHERE bind_code = ?1",
                [code],
                Self::device_from_row,
            )
            .optional()
            .map_err(|_| "数据库查询失败".to_string())?;
        match found {
            Some(device) if device.bind_code_expires_at > now => Ok(device),
            _ => {
                let lockout = Self::record_bind_failure(conn, identity, now)?;
                Err(Self::bind_failure_message(lockout))
            }
        }
    }

    fn bind_lockout_remaining(
        conn: &Connection,
        identity: &str,
        now: i64,
    ) -> Result<Option<i64>, String> {
        let locked_until = conn
            .query_row(
                "SELECT locked_until FROM bind_attempts WHERE identity = ?1",
                [identity],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| "数据库查询失败".to_string())?
            .unwrap_or(0);
        Ok((locked_until > now).then(|| locked_until - now))
    }

    /// 累计一次绑定失败，达到上限即锁定并返回锁定时长（秒）；未触发返回 0。
    fn record_bind_failure(conn: &Connection, identity: &str, now: i64) -> Result<i64, String> {
        conn.execute(
            "DELETE FROM bind_attempts
             WHERE locked_until <= ?1 AND last_failed_at < ?2",
            params![now, now - BIND_ATTEMPT_RETENTION_SECS],
        )
        .map_err(|_| "无法清理绑定失败记录".to_string())?;

        let exists = conn
            .query_row(
                "SELECT 1 FROM bind_attempts WHERE identity = ?1",
                [identity],
                |_| Ok(()),
            )
            .optional()
            .map_err(|_| "数据库查询失败".to_string())?
            .is_some();
        if !exists {
            let row_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM bind_attempts", [], |row| row.get(0))
                .map_err(|_| "数据库查询失败".to_string())?;
            if row_count >= BIND_ATTEMPT_MAX_ROWS {
                conn.execute(
                    "DELETE FROM bind_attempts WHERE identity IN (
                         SELECT identity FROM bind_attempts
                         WHERE locked_until <= ?1
                         ORDER BY last_failed_at ASC
                         LIMIT ?2
                     )",
                    params![now, BIND_ATTEMPT_EVICT_ROWS],
                )
                .map_err(|_| "无法清理绑定失败记录".to_string())?;
                let remaining: i64 = conn
                    .query_row("SELECT COUNT(*) FROM bind_attempts", [], |row| row.get(0))
                    .map_err(|_| "数据库查询失败".to_string())?;
                if remaining >= BIND_ATTEMPT_MAX_ROWS {
                    return Err("绑定校验繁忙，请稍后重试".to_string());
                }
            }
        }

        conn.execute(
            "INSERT INTO bind_attempts
                 (identity, failures, locked_until, last_failed_at)
             VALUES (?1, 1, 0, ?2)
             ON CONFLICT(identity) DO UPDATE SET
                 failures = CASE
                     WHEN bind_attempts.last_failed_at < ?3 THEN 1
                     ELSE bind_attempts.failures + 1
                 END,
                 last_failed_at = ?2",
            params![identity, now, now - BIND_FAILURE_WINDOW_SECS],
        )
        .map_err(|_| "无法记录绑定失败次数".to_string())?;
        let failures: i64 = conn
            .query_row(
                "SELECT failures FROM bind_attempts WHERE identity = ?1",
                [identity],
                |row| row.get(0),
            )
            .map_err(|_| "数据库查询失败".to_string())?;
        if failures < BIND_MAX_FAILURES {
            return Ok(0);
        }
        // 锁定的同时清零计数，让锁定期一过就重新获得完整的尝试次数。
        let _ = conn
            .execute(
                "UPDATE bind_attempts
             SET failures = 0, locked_until = ?2, last_failed_at = ?3
             WHERE identity = ?1",
                params![identity, now + BIND_LOCKOUT_SECS, now],
            )
            .map_err(|_| "无法记录绑定锁定".to_string())?;
        Ok(BIND_LOCKOUT_SECS)
    }

    fn clear_bind_attempts_tx(tx: &Transaction<'_>, identity: &str) -> rusqlite::Result<()> {
        tx.execute("DELETE FROM bind_attempts WHERE identity = ?1", [identity])?;
        Ok(())
    }

    fn bind_failure_message(lockout_secs: i64) -> String {
        if lockout_secs > 0 {
            Self::lockout_message(lockout_secs)
        } else {
            "验证码无效或已过期，请在电脑端重新生成".to_string()
        }
    }

    fn lockout_message(remaining_secs: i64) -> String {
        format!(
            "绑定失败次数过多，请在 {} 分钟后重试",
            (remaining_secs + 59) / 60
        )
    }

    pub async fn get_plans_by_open_id(
        &self,
        open_id: &str,
    ) -> Option<(DeviceUser, Vec<StudyPlan>)> {
        let conn = self.conn.lock().ok()?;
        let user = Self::get_device_by_feishu(&conn, open_id)?;
        Some((user.clone(), Self::load_plans(&conn, &user.device_token)))
    }

    pub async fn get_plans_by_telegram_chat_id(
        &self,
        chat_id: i64,
    ) -> Option<(DeviceUser, Vec<StudyPlan>)> {
        let conn = self.conn.lock().ok()?;
        let user = Self::get_device_by_telegram(&conn, chat_id)?;
        Some((user.clone(), Self::load_plans(&conn, &user.device_token)))
    }

    pub async fn get_all_bound_users(&self) -> Vec<(DeviceUser, Vec<StudyPlan>)> {
        let conn = match self.conn.lock() {
            Ok(conn) => conn,
            Err(_) => return Vec::new(),
        };
        let Ok(mut statement) = conn.prepare(
            "SELECT device_token, feishu_open_id, feishu_user_name, telegram_chat_id, telegram_user_name, bind_code, bind_code_expires_at, created_at FROM devices WHERE feishu_open_id IS NOT NULL",
        ) else { return Vec::new() };
        let Ok(rows) = statement.query_map([], Self::device_from_row) else {
            return Vec::new();
        };
        rows.filter_map(Result::ok)
            .map(|user| {
                let plans = Self::load_plans(&conn, &user.device_token);
                (user, plans)
            })
            .collect()
    }

    pub async fn get_all_telegram_bound_users(&self) -> Vec<(DeviceUser, Vec<StudyPlan>)> {
        let conn = match self.conn.lock() {
            Ok(conn) => conn,
            Err(_) => return Vec::new(),
        };
        let Ok(mut statement) = conn.prepare(
            "SELECT device_token, feishu_open_id, feishu_user_name, telegram_chat_id, telegram_user_name, bind_code, bind_code_expires_at, created_at FROM devices WHERE telegram_chat_id IS NOT NULL",
        ) else { return Vec::new() };
        let Ok(rows) = statement.query_map([], Self::device_from_row) else {
            return Vec::new();
        };
        rows.filter_map(Result::ok)
            .map(|user| {
                let plans = Self::load_plans(&conn, &user.device_token);
                (user, plans)
            })
            .collect()
    }

    /// 客户端主控排期结构；机器人打卡和备注以 `updated_at` 最后写入者胜出。
    ///
    /// 未知令牌返回 [`SyncError::UnknownDevice`]，绝不就地创建设备：否则 `/api/sync`
    /// 会退化成一个匿名可写的注册入口，任何人都能凭空灌入 devices 行。设备只能经
    /// [`Self::register_device`] 由服务端发牌产生。
    pub async fn sync_plans(
        &self,
        device_token: &str,
        incoming_plans: Vec<StudyPlan>,
        incoming_notes: DailyNotes,
    ) -> Result<SyncOutcome, SyncError> {
        let mut conn = self.conn.lock().map_err(|_| SyncError::Storage)?;
        let tx = conn.transaction().map_err(|_| SyncError::Storage)?;
        let Some(user) = Self::get_device(&tx, device_token) else {
            return Err(SyncError::UnknownDevice);
        };
        let remote_plans = Self::load_plans(&tx, device_token);
        let mut remote_map: HashMap<String, StudyPlan> = remote_plans
            .into_iter()
            .map(|plan| (plan.id.clone(), plan))
            .collect();
        let mut final_plans = Vec::new();
        for mut incoming in incoming_plans {
            if let Some(existing) = remote_map.remove(&incoming.id) {
                crate::schedule_recovery::merge_checkins(&mut incoming, &existing, true);
            }
            final_plans.push(incoming);
        }
        final_plans.sort_by_key(|plan| plan.created_at);
        let mut final_notes = Self::load_notes(&tx, device_token);
        Self::merge_notes(&mut final_notes, incoming_notes);
        Self::save_plans_tx(&tx, device_token, &final_plans).map_err(|_| SyncError::Storage)?;
        Self::save_notes_tx(&tx, device_token, &final_notes).map_err(|_| SyncError::Storage)?;
        tx.commit().map_err(|_| SyncError::Storage)?;
        Ok(SyncOutcome {
            plans: final_plans,
            daily_notes: final_notes,
            feishu_bound: user.feishu_open_id.is_some(),
            feishu_user_name: user.feishu_user_name,
            telegram_bound: user.telegram_chat_id.is_some(),
            telegram_user_name: user.telegram_user_name,
        })
    }

    fn toggle_task_for_token(
        conn: &mut Connection,
        token: &str,
        plan_id: &str,
        task_id: &str,
    ) -> Result<bool, &'static str> {
        let mut plans = Self::load_plans(conn, token);
        let now = Utc::now().timestamp();
        for plan in plans.iter_mut().filter(|plan| plan.id == plan_id) {
            let Some(is_completed) = crate::schedule_recovery::toggle(plan, task_id, now, true)
            else {
                continue;
            };
            let tx = conn.transaction().map_err(|_| "数据库写入失败")?;
            Self::save_plans_tx(&tx, token, &plans).map_err(|_| "数据库写入失败")?;
            tx.commit().map_err(|_| "数据库写入失败")?;
            return Ok(is_completed);
        }
        Err("未找到对应计划或任务")
    }

    pub async fn toggle_task_by_open_id(
        &self,
        open_id: &str,
        plan_id: &str,
        task_id: &str,
    ) -> Result<bool, &'static str> {
        let mut conn = self.conn.lock().map_err(|_| "SQLite 锁异常")?;
        let user = Self::get_device_by_feishu(&conn, open_id).ok_or("未找到绑定设备")?;
        Self::toggle_task_for_token(&mut conn, &user.device_token, plan_id, task_id)
    }

    pub async fn toggle_task_by_telegram_chat_id(
        &self,
        chat_id: i64,
        plan_id: &str,
        task_id: &str,
    ) -> Result<bool, &'static str> {
        let mut conn = self.conn.lock().map_err(|_| "SQLite 锁异常")?;
        let user = Self::get_device_by_telegram(&conn, chat_id)
            .ok_or("未找到绑定设备，请先发送 /bind 绑定")?;
        Self::toggle_task_for_token(&mut conn, &user.device_token, plan_id, task_id)
    }

    /// 按键去重并记录推送；容量超过 500 时删除最旧的 100 条。
    pub async fn record_pushed_date(&self, open_id: &str, push_type: &str, date: &str) -> bool {
        let conn = match self.conn.lock() {
            Ok(conn) => conn,
            Err(_) => return false,
        };
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
        let dir =
            std::env::temp_dir().join(format!("store_unregistered_{}", rand::random::<u64>()));
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
        let dir =
            std::env::temp_dir().join(format!("store_full_restore_{}", rand::random::<u64>()));
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
}
