use super::*;

impl PlannerApp {
    /// 安全地将云端返回的最新打卡状态合并到本地正在编辑的计划中（防止覆盖本地尚未同步的最新修改）。
    pub(super) fn merge_synced_plans(&mut self, remote_plans: Vec<StudyPlan>) {
        // 只合并仍存在的计划；同步期间删除最后一项后，旧响应不能把它复活。
        let mut remote_map: std::collections::HashMap<String, StudyPlan> =
            std::collections::HashMap::new();
        for rp in remote_plans {
            remote_map.insert(rp.id.clone(), rp);
        }

        for plan in &mut self.config.plans {
            if let Some(rp) = remote_map.get(&plan.id) {
                crate::schedule_recovery::merge_checkins(plan, rp, false);
            }
        }
        crate::study::restore_cancelled_advanced_tasks(&mut self.config.plans);
    }

    /// 触发与云服务双向增量同步（手动同步，弹窗呈现详情）。
    pub(super) fn sync_cloud_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cloud_syncing {
            return;
        }
        self.cloud_syncing = true;
        cx.notify();

        let mut cfg_clone = self.config.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let res = sync_with_cloud(&mut cfg_clone);
                    (res, cfg_clone)
                })
                .await;

            this.update_in(cx, |this, window, cx| {
                this.cloud_syncing = false;
                let (res, synced_cfg) = result;
                match res {
                    Ok(_) => {
                        this.merge_synced_plans(synced_cfg.plans);
                        crate::study::merge_daily_notes(
                            &mut this.config.daily_notes,
                            synced_cfg.daily_notes,
                        );
                        this.config.feishu_bound = synced_cfg.feishu_bound;
                        this.config.feishu_user_name = synced_cfg.feishu_user_name;
                        if synced_cfg.sync_device_token.is_some() {
                            this.config.sync_device_token = synced_cfg.sync_device_token;
                        }
                        this.config.sync_revision = synced_cfg.sync_revision;
                        save_config(&this.config);

                        let feishu_status = if this.config.feishu_bound {
                            format!(
                                "已绑定 ({})",
                                this.config.feishu_user_name.as_deref().unwrap_or("学习者")
                            )
                        } else {
                            "未连接飞书机器人 (点击「绑定飞书」开始连接)".to_string()
                        };
                        let plan_count = this.config.plans.len();
                        let today = today_date_str();
                        let today_tasks = get_tasks_for_date(&this.config.plans, &today);
                        let done_count = today_tasks.iter().filter(|t| t.task.completed).count();

                        this.cloud_sync_modal_data = Some((
                            "云端同步成功".to_string(),
                            true,
                            vec![
                                format!("📚 学习科目：共 {plan_count} 门计划已完成状态对齐"),
                                format!("📱 飞书状态：{feishu_status}"),
                                format!(
                                    "🔥 今日任务：共 {} 项，已完成 {done_count} 项打卡",
                                    today_tasks.len()
                                ),
                                "✨ 本地与云端数据已保持最新一致！".to_string(),
                            ],
                        ));
                        this.cloud_sync_modal_open = true;
                    }
                    Err(e) => {
                        this.cloud_sync_modal_data = Some((
                            "云端同步失败".to_string(),
                            false,
                            vec![
                                format!("❌ 失败原因：{e}"),
                                "💡 建议：请检查本地网络连接及云服务器运行状态。".to_string(),
                            ],
                        ));
                        this.cloud_sync_modal_open = true;
                    }
                }
                if this.has_pending_auto_sync {
                    this.has_pending_auto_sync = false;
                    this.trigger_auto_sync(window, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// 触发检测到修改后的自动同步（静默异步执行，成功后右上角弹出通知并自动消失）。
    pub(super) fn trigger_auto_sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.config.auto_sync {
            return;
        }
        if self.cloud_syncing {
            self.has_pending_auto_sync = true;
            return;
        }
        self.cloud_syncing = true;
        cx.notify();

        let mut cfg_clone = self.config.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let res = sync_with_cloud(&mut cfg_clone);
                    (res, cfg_clone)
                })
                .await;

            this.update_in(cx, |this, window, cx| {
                this.cloud_syncing = false;
                let (res, synced_cfg) = result;
                match res {
                    Ok(_) => {
                        this.merge_synced_plans(synced_cfg.plans);
                        crate::study::merge_daily_notes(
                            &mut this.config.daily_notes,
                            synced_cfg.daily_notes,
                        );
                        this.config.feishu_bound = synced_cfg.feishu_bound;
                        this.config.feishu_user_name = synced_cfg.feishu_user_name;
                        if synced_cfg.sync_device_token.is_some() {
                            this.config.sync_device_token = synced_cfg.sync_device_token;
                        }
                        this.config.sync_revision = synced_cfg.sync_revision;
                        save_config(&this.config);
                        window.push_notification(
                            Notification::success("☁️ 检测到修改，已自动同步到云端"),
                            cx,
                        );
                    }
                    Err(e) => {
                        window.push_notification(
                            Notification::warning(format!("⚠️ 自动同步未成功: {e}")),
                            cx,
                        );
                    }
                }
                if this.has_pending_auto_sync {
                    this.has_pending_auto_sync = false;
                    this.trigger_auto_sync(window, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// 打开飞书绑定弹窗并生成 6 位验证码。
    pub(super) fn request_bind_code_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match request_cloud_bind_code(&mut self.config) {
            Ok((code, expires)) => {
                self.cloud_bind_code = Some(code);
                self.cloud_bind_expires = Some(expires);
                self.cloud_bind_modal_open = true;
            }
            Err(e) => {
                window.push_notification(Notification::error(e), cx);
            }
        }
        cx.notify();
    }

    /// 检查飞书绑定状态。
    pub(super) fn check_bind_status_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match check_cloud_bind_status(&mut self.config) {
            Ok(bound) => {
                if bound {
                    self.cloud_bind_modal_open = false;
                    let name = self
                        .config
                        .feishu_user_name
                        .as_deref()
                        .unwrap_or("飞书用户");
                    window.push_notification(
                        Notification::success(format!("🎉 飞书绑定成功！已连接到 {name}")),
                        cx,
                    );
                    self.trigger_auto_sync(window, cx);
                } else {
                    window.push_notification(
                        Notification::info(
                            "尚未检测到绑定消息，请先在飞书聊天框向机器人发送 /bind <验证码>",
                        ),
                        cx,
                    );
                }
            }
            Err(e) => {
                window.push_notification(Notification::error(e), cx);
            }
        }
        cx.notify();
    }

    // -----------------------------------------------------------------------
    // 搜索历史
    // -----------------------------------------------------------------------
}
