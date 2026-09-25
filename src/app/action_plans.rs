use super::*;

impl PlannerApp {
    /// 把当前生成的计划加入打卡计划库。
    pub(super) fn enroll_current_plan(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Phase::Ready(rd) = &self.phase else {
            window.push_notification(Notification::warning("请先获取视频并生成观看计划。"), cx);
            return;
        };
        let input = self.input_value(&self.link_input, cx);
        let manual_start_date = self.input_value(&self.start_date_input, cx);
        let today_plan_day = self.input_value(&self.today_plan_day_input, cx);
        let start_date = if today_plan_day.trim().is_empty() {
            if manual_start_date.trim().is_empty() {
                today_date_str()
            } else {
                manual_start_date.trim().to_string()
            }
        } else {
            let day_number = match today_plan_day.trim().parse::<usize>() {
                Ok(day_number) if day_number > 0 => day_number,
                _ => {
                    window.push_notification(
                        Notification::warning("“今天是第几天”必须是正整数。"),
                        cx,
                    );
                    return;
                }
            };
            let planned_days = rd.plan.as_ref().map(|plan| plan.plan.len()).unwrap_or(0);
            if day_number > planned_days {
                window.push_notification(
                    Notification::warning(format!(
                        "“今天是第几天”不能超过计划总天数（{planned_days} 天）。"
                    )),
                    cx,
                );
                return;
            }
            match infer_plan_start_date(&today_date_str(), day_number, self.skip_weekends_toggle) {
                Ok(date) => date,
                Err(error) => {
                    window.push_notification(Notification::warning(error), cx);
                    return;
                }
            }
        };
        let source_tag = crate::core::source_tag(self.source);

        match enroll_study_plan(
            &mut self.config,
            rd,
            &input,
            source_tag,
            &start_date,
            self.skip_weekends_toggle,
        ) {
            Ok(plan) => {
                window.push_notification(
                    Notification::success(format!("已成功开启《{}》每日打卡计划！", plan.title)),
                    cx,
                );
                self.active_tab = AppTab::TodayCheckIn;
                self.selected_date = today_date_str();
                self.trigger_auto_sync(window, cx);
            }
            Err(e) => {
                window.push_notification(Notification::error(e), cx);
            }
        }
        cx.notify();
    }

    /// 从「我的计划库」创建一个按日期与每日时长安排的自定义任务。
    pub(super) fn create_custom_task_action(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = self.input_value(&self.custom_title_input, cx);
        let start_date = self.input_value(&self.custom_start_date_input, cx);
        let days_text = self.input_value(&self.custom_days_input, cx);
        let duration_text = self.input_value(&self.custom_duration_input, cx);
        let days = match parse_days(&days_text) {
            Ok(days) => days,
            Err(error) => {
                window.push_notification(Notification::warning(format!("执行天数：{error}")), cx);
                return;
            }
        };
        let daily_minutes = match duration_text.trim().parse::<i64>() {
            Ok(minutes) if minutes > 0 => minutes,
            _ => {
                window.push_notification(Notification::warning("每日时长必须是正整数分钟。"), cx);
                return;
            }
        };
        let date = if start_date.trim().is_empty() {
            today_date_str()
        } else {
            start_date.trim().to_string()
        };

        match add_custom_study_plan(
            &mut self.config,
            &title,
            &date,
            days,
            daily_minutes,
            self.custom_skip_weekends_toggle,
        ) {
            Ok(plan) => {
                self.custom_title_input
                    .update(cx, |state, cx| state.set_value("", window, cx));
                self.custom_days_input
                    .update(cx, |state, cx| state.set_value("", window, cx));
                self.custom_duration_input
                    .update(cx, |state, cx| state.set_value("", window, cx));
                self.custom_task_form_open = false;
                window.push_notification(
                    Notification::success(format!("已添加自定义任务《{}》", plan.title)),
                    cx,
                );
                self.trigger_auto_sync(window, cx);
            }
            Err(error) => window.push_notification(Notification::error(error), cx),
        }
        cx.notify();
    }

    /// 切换单个学习任务的打卡状态。
    pub(super) fn toggle_task_checkin_action(
        &mut self,
        plan_id: &str,
        task_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match checkin_study_task(&mut self.config, plan_id, task_id) {
            Ok(completed) => {
                if completed {
                    window.push_notification(
                        Notification::success("已完成打卡！保持专注与节奏 🔥"),
                        cx,
                    );
                } else {
                    window.push_notification(Notification::info("已撤回该项打卡"), cx);
                }
                self.trigger_auto_sync(window, cx);
            }
            Err(e) => {
                window.push_notification(Notification::error(e), cx);
            }
        }
        cx.notify();
    }

    /// 一键打卡指定计划在某一天的全部任务。
    pub(super) fn checkin_entire_day_action(
        &mut self,
        plan_id: &str,
        date: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(e) = crate::study::checkin_entire_day(&mut self.config.plans, plan_id, date) {
            window.push_notification(Notification::error(e), cx);
        } else {
            save_config(&self.config);
            window.push_notification(
                Notification::success("🎉 今日该科目任务已全部完成打卡！"),
                cx,
            );
            self.trigger_auto_sync(window, cx);
        }
        cx.notify();
    }

    /// 将所有进行中计划在过去未完成的任务分批顺延到今天起的学习日。
    pub(super) fn push_forward_all_behind_action(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let today = today_date_str();
        let plan_ids: Vec<String> = self
            .config
            .plans
            .iter()
            // 一次性日历任务固定在用户指定日期，不应被“落后顺延”自动改期。
            .filter(|p| p.status == PlanStatus::Active && p.show_in_library)
            .map(|p| p.id.clone())
            .collect();

        let mut count = 0;
        for id in plan_ids {
            if push_forward_study_plan(&mut self.config, &id, &today).unwrap_or(false) {
                count += 1;
            }
        }
        if count > 0 {
            window.push_notification(
                Notification::success(format!(
                    "已将 {count} 门科目过去未完成的任务分批顺延到今天起的学习日！"
                )),
                cx,
            );
            self.trigger_auto_sync(window, cx);
        } else {
            window.push_notification(Notification::info("暂无需要顺延到今天的未完成计划"), cx);
        }
        cx.notify();
    }

    /// 将当前选中的未来日期里已打卡的条目划归今天。
    pub(super) fn advance_completed_tasks_action(
        &mut self,
        future_date: &str,
        only_plan_id: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let today = today_date_str();
        if future_date <= today.as_str() {
            window.push_notification(Notification::info("请选择今天之后的日期。"), cx);
            return;
        }
        let plan_ids: Vec<String> = self
            .config
            .plans
            .iter()
            .filter(|plan| matches!(plan.status, PlanStatus::Active | PlanStatus::Completed))
            .filter(|plan| only_plan_id.is_none_or(|id| plan.id == id))
            .map(|plan| plan.id.clone())
            .collect();
        let mut moved_tasks = 0usize;
        let mut affected_plans = 0usize;
        for plan_id in plan_ids {
            let moved =
                advance_completed_study_tasks(&mut self.config, &plan_id, future_date, &today)
                    .unwrap_or(0);
            if moved > 0 {
                moved_tasks += moved;
                affected_plans += 1;
            }
        }
        if moved_tasks > 0 {
            window.push_notification(
                Notification::success(format!(
                    "已将 {affected_plans} 个计划中的 {moved_tasks} 项已完成任务提前到今天。"
                )),
                cx,
            );
            self.selected_date = today.clone();
            self.calendar_selected_date = today;
            self.trigger_auto_sync(window, cx);
        } else {
            window.push_notification(Notification::info("所选未来日期没有已打卡任务。"), cx);
        }
        cx.notify();
    }

    /// 切换计划状态（暂停/继续）。
    pub(super) fn toggle_plan_status_action(
        &mut self,
        plan_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        toggle_study_plan_status(&mut self.config, plan_id);
        self.trigger_auto_sync(window, cx);
        cx.notify();
    }

    /// 删除指定计划。
    pub(super) fn delete_plan_action(
        &mut self,
        plan_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        remove_study_plan(&mut self.config, plan_id);
        window.push_notification(Notification::info("已移除该学习计划"), cx);
        self.trigger_auto_sync(window, cx);
        cx.notify();
    }

    /// 顺延单门计划。
    pub(super) fn push_forward_single_plan_action(
        &mut self,
        plan_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let today = today_date_str();
        match push_forward_study_plan(&mut self.config, plan_id, &today) {
            Ok(true) => {
                window.push_notification(
                    Notification::success("已将本科目过去未完成任务分批顺延到今天起的学习日。"),
                    cx,
                );
                self.trigger_auto_sync(window, cx);
            }
            Ok(false) => window.push_notification(
                Notification::info("本科目没有需要顺延的过去未完成任务。"),
                cx,
            ),
            Err(e) => {
                window.push_notification(Notification::error(e), cx);
            }
        }
        cx.notify();
    }

    /// 打开整门计划的未完成任务改期弹窗，并用当前最早未完成日期预填。
    pub(super) fn open_plan_reschedule_action(
        &mut self,
        plan_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_plan_schedule_modal(plan_id, PlanRescheduleMode::ShiftStart, window, cx);
    }

    pub(super) fn open_plan_redistribute_action(
        &mut self,
        plan_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_plan_schedule_modal(plan_id, PlanRescheduleMode::RedistributeToEnd, window, cx);
    }

    fn open_plan_schedule_modal(
        &mut self,
        plan_id: &str,
        mode: PlanRescheduleMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(plan) = self.config.plans.iter().find(|plan| plan.id == plan_id) else {
            window.push_notification(Notification::error("未找到指定计划"), cx);
            return;
        };
        let Some(current_start) = plan
            .schedules
            .iter()
            .filter(|schedule| schedule.tasks.iter().any(|task| !task.completed))
            .map(|schedule| schedule.date.as_str())
            .min()
        else {
            window.push_notification(Notification::info("该计划没有未完成任务。"), cx);
            return;
        };
        let initial_date = match mode {
            PlanRescheduleMode::ShiftStart => current_start.to_string(),
            PlanRescheduleMode::RedistributeToEnd => plan.end_date.clone(),
        };
        self.plan_reschedule_date_input
            .update(cx, |state, cx| state.set_value(initial_date, window, cx));
        self.plan_reschedule_plan_id = Some(plan_id.to_string());
        self.plan_reschedule_mode = mode;
        self.plan_reschedule_modal_open = true;
        cx.notify();
    }

    /// 保存整门计划的未完成任务排期调整。
    pub(super) fn save_plan_reschedule_action(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(plan_id) = self.plan_reschedule_plan_id.clone() else {
            self.plan_reschedule_modal_open = false;
            cx.notify();
            return;
        };
        if self.plan_reschedule_mode == PlanRescheduleMode::RedistributeToEnd {
            let target_end = self.input_value(&self.plan_reschedule_date_input, cx);
            let plan_title = self
                .config
                .plans
                .iter()
                .find(|plan| plan.id == plan_id)
                .map(|plan| plan.title.clone())
                .unwrap_or_else(|| "计划".to_string());
            match redistribute_unfinished_study_plan(
                &mut self.config,
                &plan_id,
                &today_date_str(),
                target_end.trim(),
            ) {
                Ok(true) => {
                    self.plan_reschedule_modal_open = false;
                    self.plan_reschedule_plan_id = None;
                    window.push_notification(
                        Notification::success(format!(
                            "已将《{plan_title}》按剩余时长重排至 {}，已完成记录保持原日期。",
                            target_end.trim()
                        )),
                        cx,
                    );
                    self.trigger_auto_sync(window, cx);
                }
                Ok(false) => window.push_notification(Notification::info("排期未发生变化。"), cx),
                Err(error) => window.push_notification(Notification::error(error), cx),
            }
            cx.notify();
            return;
        }
        let target_start = self.input_value(&self.plan_reschedule_date_input, cx);
        let (plan_title, current_start) = self
            .config
            .plans
            .iter()
            .find(|plan| plan.id == plan_id)
            .map(|plan| {
                let start = plan
                    .schedules
                    .iter()
                    .filter(|schedule| schedule.tasks.iter().any(|task| !task.completed))
                    .map(|schedule| schedule.date.clone())
                    .min()
                    .unwrap_or_default();
                (plan.title.clone(), start)
            })
            .unwrap_or_else(|| ("计划".to_string(), String::new()));

        match reschedule_unfinished_study_plan(&mut self.config, &plan_id, target_start.trim()) {
            Ok(true) => {
                let direction = if target_start.trim() < current_start.as_str() {
                    "前移"
                } else {
                    "后移"
                };
                self.plan_reschedule_modal_open = false;
                self.plan_reschedule_plan_id = None;
                window.push_notification(
                    Notification::success(format!(
                        "已将《{plan_title}》未完成任务整体{direction}，最早任务从 {} 开始；已完成记录保持原日期。",
                        target_start.trim()
                    )),
                    cx,
                );
                self.trigger_auto_sync(window, cx);
            }
            Ok(false) => window.push_notification(
                Notification::info("目标日期未变化，或该计划已没有未完成任务。"),
                cx,
            ),
            Err(error) => window.push_notification(Notification::error(error), cx),
        }
        cx.notify();
    }
}
