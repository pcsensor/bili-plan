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

    /// 双向增量同步合并计划。
    pub async fn sync_plans(
        &self,
        device_token: &str,
        incoming_plans: Vec<StudyPlan>,
    ) -> (Vec<StudyPlan>, bool, Option<String>) {
        let mut guard = self.inner.write().await;
        let user = guard.devices.entry(device_token.to_string()).or_insert_with(|| {
            DeviceUser {
                device_token: device_token.to_string(),
                feishu_open_id: None,
                feishu_user_name: None,
                bind_code: None,
                bind_code_expires_at: 0,
                created_at: Local::now().to_rfc3339(),
            }
        });
        let bound = user.feishu_open_id.is_some();
        let user_name = user.feishu_user_name.clone();

        let remote_plans = guard.plans.entry(device_token.to_string()).or_default();

        // 合并策略：按科目 plan.id 对应合并，未修改的保留，两端冲突以 completed_at 更晚或 true 优先
        let mut merged_map: HashMap<String, StudyPlan> = HashMap::new();
        for p in remote_plans.drain(..) {
            merged_map.insert(p.id.clone(), p);
        }

        for incoming in incoming_plans {
            if let Some(existing) = merged_map.get_mut(&incoming.id) {
                // 逐日逐任务合并打卡状态（基于 TaskItem.updated_at 遵循 Last-Write-Wins 规则）
                for in_sch in incoming.schedules {
                    if let Some(ex_sch) = existing.schedules.iter_mut().find(|s| s.date == in_sch.date) {
                        for in_t in in_sch.tasks {
                            if let Some(ex_t) = ex_sch.tasks.iter_mut().find(|t| t.id == in_t.id) {
                                if in_t.updated_at >= ex_t.updated_at {
                                    // 客户端更新时间更新（或同时为 0 时以客户端本次上传状态为准）
                                    ex_t.completed = in_t.completed;
                                    ex_t.completed_at = in_t.completed_at;
                                    ex_t.updated_at = in_t.updated_at;
                                }
                                // 若 ex_t.updated_at > in_t.updated_at，则保留服务端最新操作（如飞书端的打卡/取消打卡），
                                // 并在随后将该计划返回给客户端覆盖本地状态。
                            }
                        }
                    }
                }
                existing.status = incoming.status;
                existing.title = incoming.title;
                existing.skip_weekends = incoming.skip_weekends;
            } else {
                merged_map.insert(incoming.id.clone(), incoming);
            }
        }

        let mut final_plans: Vec<StudyPlan> = merged_map.into_values().collect();
        final_plans.sort_by(|a, b| a.created_at.cmp(&b.created_at));

        guard.plans.insert(device_token.to_string(), final_plans.clone());
        drop(guard);
        self.persist().await;

        (final_plans, bound, user_name)
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

    /// 记录最后一次推送日期。
    pub async fn record_pushed_date(&self, open_id: &str, push_type: &str, date: &str) -> bool {
        let mut guard = self.inner.write().await;
        let key = format!("{}:{}:{}", open_id, push_type, date);
        if guard.push_logs.contains_key(&key) {
            return false; // 已推送过
        }
        guard.push_logs.insert(key, Local::now().to_rfc3339());
        drop(guard);
        self.persist().await;
        true
    }
}
