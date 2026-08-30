use crate::models::{DeviceUser, StudyPlan};
use chrono::{Local, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct StoreData {
    pub devices: HashMap<String, DeviceUser>,
    pub plans: HashMap<String, Vec<StudyPlan>>, // device_token -> plans
    #[serde(default)]
    pub push_logs: HashMap<String, String>,     // feishu_open_id -> last_push_date
}

#[derive(Clone)]
pub struct Store {
    data_path: PathBuf,
    inner: Arc<RwLock<StoreData>>,
}

impl Store {
    pub fn new(data_dir: impl AsRef<Path>) -> Self {
        let dir = data_dir.as_ref().to_path_buf();
        let _ = fs::create_dir_all(&dir);
        let data_path = dir.join("store.json");

        let data = if data_path.exists() {
            match fs::read_to_string(&data_path) {
                Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
                Err(_) => StoreData::default(),
            }
        } else {
            StoreData::default()
        };

        Self {
            data_path,
            inner: Arc::new(RwLock::new(data)),
        }
    }

    async fn persist(&self) {
        let guard = self.inner.read().await;
        if let Ok(json) = serde_json::to_string_pretty(&*guard) {
            let tmp = self.data_path.with_extension("json.tmp");
            if fs::write(&tmp, json).is_ok() {
                let _ = fs::rename(tmp, &self.data_path);
            }
        }
    }

    /// 获取或创建设备 Token。
    pub async fn get_or_create_device(&self, device_token: Option<&str>) -> DeviceUser {
        let mut guard = self.inner.write().await;
        if let Some(token) = device_token {
            if let Some(user) = guard.devices.get(token) {
                return user.clone();
            }
        }

        let new_token = format!("{:x}{:x}", rand::random::<u64>(), rand::random::<u64>());
        let now = Local::now().to_rfc3339();
        let user = DeviceUser {
            device_token: new_token.clone(),
            feishu_open_id: None,
            feishu_user_name: None,
            telegram_chat_id: None,
            telegram_user_name: None,
            bind_code: None,
            bind_code_expires_at: 0,
            created_at: now,
        };
        guard.devices.insert(new_token.clone(), user.clone());
        drop(guard);
        self.persist().await;
        user
    }

    /// 为设备生成 6 位绑定验证码（有效期 10 分钟）。
    pub async fn generate_bind_code(&self, device_token: &str) -> Option<String> {
        let mut guard = self.inner.write().await;
        let user = guard.devices.get_mut(device_token)?;

        let code: u32 = rand::thread_rng().gen_range(100_000..999_999);
        let code_str = code.to_string();
        let expires = Utc::now().timestamp() + 600; // 10 minutes

        user.bind_code = Some(code_str.clone());
        user.bind_code_expires_at = expires;
        drop(guard);
        self.persist().await;
        Some(code_str)
    }

    /// 飞书用户通过验证码绑定设备。
    pub async fn bind_by_code(
        &self,
        code: &str,
        open_id: &str,
        user_name: Option<&str>,
    ) -> Result<DeviceUser, &'static str> {
        let mut guard = self.inner.write().await;
        let now = Utc::now().timestamp();

        let mut target_token = None;
        for (token, user) in guard.devices.iter() {
            if let Some(c) = &user.bind_code {
                if c == code {
                    if user.bind_code_expires_at < now {
                        return Err("验证码已过期，请在电脑客户端重新生成");
                    }
                    target_token = Some(token.clone());
                    break;
                }
            }
        }

        let token = match target_token {
            Some(t) => t,
            None => return Err("验证码无效或未找到对应设备"),
        };

        if let Some(user) = guard.devices.get_mut(&token) {
            user.feishu_open_id = Some(open_id.to_string());
            user.feishu_user_name = user_name.map(|s| s.to_string());
            user.bind_code = None;
            let result = user.clone();
            drop(guard);
            self.persist().await;
            Ok(result)
        } else {
            Err("设备不存在")
        }
    }

    /// 使用绑定码绑定 Telegram 用户。
    pub async fn bind_telegram_by_code(
        &self,
        code: &str,
        chat_id: i64,
        user_name: Option<&str>,
    ) -> Result<DeviceUser, &'static str> {
        let mut guard = self.inner.write().await;
        let now = Utc::now().timestamp();
        let mut target_token = None;

        for (token, user) in guard.devices.iter() {
            if let Some(c) = &user.bind_code {
                if c == code && user.bind_code_expires_at > now {
                    target_token = Some(token.clone());
                    break;
                }
            }
        }

        let token = match target_token {
            Some(t) => t,
            None => return Err("验证码无效或已过期，请在电脑端重新生成"),
        };

        if let Some(user) = guard.devices.get_mut(&token) {
            user.telegram_chat_id = Some(chat_id);
            user.telegram_user_name = user_name.map(|s| s.to_string());
            user.bind_code = None;
            let result = user.clone();
            drop(guard);
            self.persist().await;
            Ok(result)
        } else {
            Err("设备不存在")
        }
    }

    /// 查找飞书 OpenID 对应的所有计划。
    pub async fn get_plans_by_open_id(&self, open_id: &str) -> Option<(DeviceUser, Vec<StudyPlan>)> {
        let guard = self.inner.read().await;
        for (token, user) in guard.devices.iter() {
            if user.feishu_open_id.as_deref() == Some(open_id) {
                let plans = guard.plans.get(token).cloned().unwrap_or_default();
                return Some((user.clone(), plans));
            }
        }
        None
    }

    /// 查找 Telegram Chat ID 对应的所有计划。
    pub async fn get_plans_by_telegram_chat_id(&self, chat_id: i64) -> Option<(DeviceUser, Vec<StudyPlan>)> {
        let guard = self.inner.read().await;
        for (token, user) in guard.devices.iter() {
            if user.telegram_chat_id == Some(chat_id) {
                let plans = guard.plans.get(token).cloned().unwrap_or_default();
                return Some((user.clone(), plans));
            }
        }
        None
    }

    /// 获取所有已绑定飞书的用户及计划。
    pub async fn get_all_bound_users(&self) -> Vec<(DeviceUser, Vec<StudyPlan>)> {
        let guard = self.inner.read().await;
        let mut list = Vec::new();
        for (token, user) in guard.devices.iter() {
            if user.feishu_open_id.is_some() {
                let plans = guard.plans.get(token).cloned().unwrap_or_default();
                list.push((user.clone(), plans));
            }
        }
        list
    }

    /// 获取所有已绑定 Telegram 的用户及计划。
    pub async fn get_all_telegram_bound_users(&self) -> Vec<(DeviceUser, Vec<StudyPlan>)> {
        let guard = self.inner.read().await;
        let mut list = Vec::new();
        for (token, user) in guard.devices.iter() {
            if user.telegram_chat_id.is_some() {
                let plans = guard.plans.get(token).cloned().unwrap_or_default();
                list.push((user.clone(), plans));
            }
        }
        list
    }

    /// 双向增量同步合并计划。
    pub async fn sync_plans(
        &self,
        device_token: &str,
        incoming_plans: Vec<StudyPlan>,
    ) -> (Vec<StudyPlan>, bool, Option<String>, bool, Option<String>) {
        let mut guard = self.inner.write().await;
        let user = guard.devices.entry(device_token.to_string()).or_insert_with(|| {
            DeviceUser {
                device_token: device_token.to_string(),
                feishu_open_id: None,
                feishu_user_name: None,
                telegram_chat_id: None,
                telegram_user_name: None,
                bind_code: None,
                bind_code_expires_at: 0,
                created_at: Local::now().to_rfc3339(),
            }
        });
        let feishu_bound = user.feishu_open_id.is_some();
        let feishu_user_name = user.feishu_user_name.clone();
        let telegram_bound = user.telegram_chat_id.is_some();
        let telegram_user_name = user.telegram_user_name.clone();

        let remote_plans: Vec<StudyPlan> = guard.plans.remove(device_token).unwrap_or_default();
        let mut remote_map: HashMap<String, StudyPlan> = HashMap::new();
        for p in remote_plans {
            remote_map.insert(p.id.clone(), p);
        }

        // 合并策略：
        // 1. 客户端为主控端（计划增删、排期与顺延以客户端结构为准）；
        // 2. 服务端（如飞书/TG 打卡）若有更新的打卡记录 (ex_t.updated_at > in_t.updated_at)，则合并保留；
        // 3. 已从客户端删除的计划不再保留。
        let mut final_plans: Vec<StudyPlan> = Vec::new();

        for mut incoming in incoming_plans {
            if let Some(existing) = remote_map.get(&incoming.id) {
                // 收集服务端中该计划的所有打卡状态
                let mut server_tasks: HashMap<&str, &crate::models::TaskItem> = HashMap::new();
                for ex_sch in &existing.schedules {
                    for ex_t in &ex_sch.tasks {
                        server_tasks.insert(ex_t.id.as_str(), ex_t);
                    }
                }

                // 将服务端更新的打卡状态合并到客户端的新排期结构中
                for in_sch in &mut incoming.schedules {
                    for in_t in &mut in_sch.tasks {
                        if let Some(ex_t) = server_tasks.get(in_t.id.as_str()) {
                            if ex_t.updated_at > in_t.updated_at {
                                in_t.completed = ex_t.completed;
                                in_t.completed_at = ex_t.completed_at;
                                in_t.updated_at = ex_t.updated_at;
                            }
                        }
                    }
                }
            }
            final_plans.push(incoming);
        }

        final_plans.sort_by(|a, b| a.created_at.cmp(&b.created_at));

        guard.plans.insert(device_token.to_string(), final_plans.clone());
        drop(guard);
        self.persist().await;

        (final_plans, feishu_bound, feishu_user_name, telegram_bound, telegram_user_name)
    }

    /// 切换指定任务的打卡状态（由飞书卡片操作触发）。
    pub async fn toggle_task_by_open_id(
        &self,
        open_id: &str,
        plan_id: &str,
        task_id: &str,
    ) -> Result<bool, &'static str> {
        let mut guard = self.inner.write().await;
        let mut token_found = None;
        for (token, user) in guard.devices.iter() {
            if user.feishu_open_id.as_deref() == Some(open_id) {
                token_found = Some(token.clone());
                break;
            }
        }

        let token = match token_found {
            Some(t) => t,
            None => return Err("未找到绑定设备"),
        };

        if let Some(plans) = guard.plans.get_mut(&token) {
            let now = Utc::now().timestamp();
            for plan in plans.iter_mut() {
                if plan.id == plan_id {
                    for sch in plan.schedules.iter_mut() {
                        for task in sch.tasks.iter_mut() {
                            if task.id == task_id {
                                task.completed = !task.completed;
                                task.completed_at = if task.completed {
                                    Some(now)
                                } else {
                                    None
                                };
                                task.updated_at = now;
                                let is_completed = task.completed;
                                drop(guard);
                                self.persist().await;
                                return Ok(is_completed);
                            }
                        }
                    }
                }
            }
        }

        Err("未找到对应计划或任务")
    }

    /// 切换指定任务的打卡状态（由 Telegram 按钮操作触发）。
    pub async fn toggle_task_by_telegram_chat_id(
        &self,
        chat_id: i64,
        plan_id: &str,
        task_id: &str,
    ) -> Result<bool, &'static str> {
        let mut guard = self.inner.write().await;
        let mut token_found = None;
        for (token, user) in guard.devices.iter() {
            if user.telegram_chat_id == Some(chat_id) {
                token_found = Some(token.clone());
                break;
            }
        }

        let token = match token_found {
            Some(t) => t,
            None => return Err("未找到绑定设备，请先发送 /bind 绑定"),
        };

        if let Some(plans) = guard.plans.get_mut(&token) {
            let now = Utc::now().timestamp();
            for plan in plans.iter_mut() {
                if plan.id == plan_id {
                    for sch in plan.schedules.iter_mut() {
                        for task in sch.tasks.iter_mut() {
                            if task.id == task_id {
                                task.completed = !task.completed;
                                task.completed_at = if task.completed {
                                    Some(now)
                                } else {
                                    None
                                };
                                task.updated_at = now;
                                let is_completed = task.completed;
                                drop(guard);
                                self.persist().await;
                                return Ok(is_completed);
                            }
                        }
                    }
                }
            }
        }

        Err("未找到对应计划或任务")
    }

    /// 记录最后一次推送日期，并自动清理 14 天前的旧推送记录。
    pub async fn record_pushed_date(&self, open_id: &str, push_type: &str, date: &str) -> bool {
        let mut guard = self.inner.write().await;
        let key = format!("{}:{}:{}", open_id, push_type, date);
        if guard.push_logs.contains_key(&key) {
            return false; // 已推送过
        }
        guard.push_logs.insert(key, Local::now().to_rfc3339());

        // 仅保留最多 500 条推送记录，防止文件无限膨胀
        if guard.push_logs.len() > 500 {
            let mut keys: Vec<String> = guard.push_logs.keys().cloned().collect();
            keys.sort();
            let remove_count = guard.push_logs.len().saturating_sub(400);
            for k in keys.into_iter().take(remove_count) {
                guard.push_logs.remove(&k);
            }
        }

        drop(guard);
        self.persist().await;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{DailySchedule, PlanStatus, StudyPlan, TaskItem};

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
                .map(|(i, &d)| DailySchedule {
                    day_index: i,
                    date: d.to_string(),
                    tasks: vec![TaskItem {
                        id: format!("{}_{}_0", id, i),
                        vid_no: i as i64 + 1,
                        title: format!("第{}讲", i + 1),
                        portion: 500,
                        remainder: 0,
                        from_prev: false,
                        completed: false,
                        completed_at: None,
                        updated_at: 0,
                    }],
                    is_rest_day: false,
                })
                .collect(),
        }
    }

    #[tokio::test]
    async fn test_sync_preserves_postponed_schedules() {
        let temp_dir = std::env::temp_dir().join(format!("store_test_{}", rand::random::<u64>()));
        let store = Store::new(&temp_dir);

        let token = "dev_123";

        // 1. 初始同步：08-25, 08-26
        let initial_plan = make_test_plan("p1", &["2026-08-25", "2026-08-26"]);
        let (plans1, _, _, _, _) = store.sync_plans(token, vec![initial_plan]).await;
        assert_eq!(plans1[0].schedules[0].date, "2026-08-25");

        // 2. 客户端在本地将未完成任务顺延至 08-30, 08-31，并再次同步
        let postponed_plan = make_test_plan("p1", &["2026-08-30", "2026-08-31"]);
        let (plans2, _, _, _, _) = store.sync_plans(token, vec![postponed_plan]).await;

        // 服务端应当采纳顺延后的排期结构，而不是固守 08-25
        assert_eq!(plans2[0].schedules[0].date, "2026-08-30");
        assert_eq!(plans2[0].schedules[1].date, "2026-08-31");
    }

    #[tokio::test]
    async fn test_telegram_bind_and_toggle() {
        let temp_dir = std::env::temp_dir().join(format!("store_test_tg_{}", rand::random::<u64>()));
        let store = Store::new(&temp_dir);

        let user_dev = store.get_or_create_device(None).await;
        let token = user_dev.device_token;
        let code = store.generate_bind_code(&token).await.unwrap();

        let plan = make_test_plan("p1", &["2026-08-30"]);
        store.sync_plans(&token, vec![plan]).await;

        // 使用 Telegram 绑定
        let user = store.bind_telegram_by_code(&code, 987654321, Some("tg_user")).await.unwrap();
        assert_eq!(user.telegram_chat_id, Some(987654321));
        assert_eq!(user.telegram_user_name.as_deref(), Some("tg_user"));

        // 通过 Telegram 进行任务打卡
        let is_done = store.toggle_task_by_telegram_chat_id(987654321, "p1", "p1_0_0").await.unwrap();
        assert!(is_done);

        // 查询计划验证已打卡
        let (_, plans) = store.get_plans_by_telegram_chat_id(987654321).await.unwrap();
        assert!(plans[0].schedules[0].tasks[0].completed);
    }
}
