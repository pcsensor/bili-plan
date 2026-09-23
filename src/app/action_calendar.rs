use super::*;

impl PlannerApp {
    /// 日期前翻一天。
    pub(super) fn prev_date_action(&mut self, cx: &mut Context<Self>) {
        self.selected_date = shift_date_str(&self.selected_date, -1);
        cx.notify();
    }

    /// 日期后翻一天。
    pub(super) fn next_date_action(&mut self, cx: &mut Context<Self>) {
        self.selected_date = shift_date_str(&self.selected_date, 1);
        cx.notify();
    }

    /// 快速回到今天。
    pub(super) fn reset_today_action(&mut self, cx: &mut Context<Self>) {
        self.selected_date = today_date_str();
        cx.notify();
    }

    /// 学习日历：前翻一个月。
    pub(super) fn prev_calendar_month_action(&mut self, cx: &mut Context<Self>) {
        if self.calendar_month == 1 {
            self.calendar_year -= 1;
            self.calendar_month = 12;
        } else {
            self.calendar_month -= 1;
        }
        cx.notify();
    }

    /// 学习日历：后翻一个月。
    pub(super) fn next_calendar_month_action(&mut self, cx: &mut Context<Self>) {
        if self.calendar_month == 12 {
            self.calendar_year += 1;
            self.calendar_month = 1;
        } else {
            self.calendar_month += 1;
        }
        cx.notify();
    }

    /// 学习日历：快速回到本月与今日。
    pub(super) fn reset_calendar_month_action(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use chrono::Datelike;
        let now = chrono::Local::now();
        self.calendar_year = now.year();
        self.calendar_month = now.month();
        let today = today_date_str();
        self.select_calendar_date_action(&today, window, cx);
    }

    /// 学习日历：选中特定日期（清空新增备注输入框并刷新右侧明细）。
    pub(super) fn select_calendar_date_action(
        &mut self,
        date_str: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.calendar_selected_date = date_str.to_string();
        self.calendar_note_input.update(cx, |state, cx| {
            state.set_value("", window, cx);
        });
        cx.notify();
    }

    /// 学习日历：追加一条当日备注。
    pub(super) fn save_calendar_note_action(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let note = self.input_value(&self.calendar_note_input, cx);
        let date = self.calendar_selected_date.clone();
        match add_daily_note(&mut self.config, &date, &note) {
            Ok(_) => {
                self.calendar_note_input
                    .update(cx, |state, cx| state.set_value("", window, cx));
                window.push_notification(
                    Notification::success(format!("已添加 {date} 的学习备注")),
                    cx,
                );
                self.trigger_auto_sync(window, cx);
            }
            Err(error) => window.push_notification(Notification::warning(error), cx),
        }
        cx.notify();
    }

    /// 删除所选日期的一条备注。
    pub(super) fn delete_calendar_note_action(
        &mut self,
        date: &str,
        note_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if delete_daily_note(&mut self.config, date, note_id) {
            window.push_notification(Notification::info("已删除该条备注"), cx);
            self.trigger_auto_sync(window, cx);
        }
        cx.notify();
    }

    /// 打开指定日期的右键新增学习计划弹窗。
    pub(super) fn open_calendar_task_modal_action(
        &mut self,
        date: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.calendar_selected_date = date.to_string();
        self.calendar_task_date = date.to_string();
        let date_for_input = date.to_string();
        self.calendar_task_target = CalendarTaskTarget::OneOff;
        self.calendar_existing_series_id = None;
        self.calendar_task_title_input
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.calendar_task_duration_input
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.calendar_task_date_input
            .update(cx, |state, cx| state.set_value(date_for_input, window, cx));
        self.calendar_series_name_input
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.calendar_task_modal_open = true;
        cx.notify();
    }

    /// 按选中的归属创建或追加日历任务。
    pub(super) fn save_calendar_task_action(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = self.input_value(&self.calendar_task_title_input, cx);
        let minutes_text = self.input_value(&self.calendar_task_duration_input, cx);
        let minutes = match minutes_text.trim().parse::<i64>() {
            Ok(minutes) if minutes > 0 => minutes,
            _ => {
                window.push_notification(Notification::warning("任务时长必须是正整数分钟。"), cx);
                return;
            }
        };
        let date = self.input_value(&self.calendar_task_date_input, cx);
        let date = date.trim().to_string();
        if chrono::NaiveDate::parse_from_str(&date, "%Y-%m-%d").is_err() {
            window.push_notification(Notification::warning("计划日期格式应为 YYYY-MM-DD。"), cx);
            return;
        }
        self.calendar_task_date = date.clone();
        let result = match self.calendar_task_target {
            CalendarTaskTarget::OneOff => {
                add_one_off_calendar_task(&mut self.config, &title, &date, minutes)
                    .map(|plan| format!("已添加 {date} 的单日任务《{}》", plan.title))
            }
            CalendarTaskTarget::NewSeries => {
                let series_name = self.input_value(&self.calendar_series_name_input, cx);
                create_calendar_series_plan(&mut self.config, &series_name, &title, &date, minutes)
                    .map(|plan| format!("已创建系列计划《{}》并加入计划库", plan.title))
            }
            CalendarTaskTarget::ExistingSeries => {
                let Some(plan_id) = self.calendar_existing_series_id.clone() else {
                    window.push_notification(Notification::warning("请选择一个已有系列计划。"), cx);
                    return;
                };
                let series_name = self
                    .config
                    .plans
                    .iter()
                    .find(|plan| plan.id == plan_id)
                    .map(|plan| plan.title.clone())
                    .unwrap_or_else(|| "系列计划".to_string());
                append_calendar_series_task(&mut self.config, &plan_id, &title, &date, minutes)
                    .map(|_| format!("已将任务追加到系列计划《{series_name}》"))
            }
        };

        match result {
            Ok(message) => {
                self.calendar_task_modal_open = false;
                window.push_notification(Notification::success(message), cx);
                self.trigger_auto_sync(window, cx);
            }
            Err(error) => window.push_notification(Notification::error(error), cx),
        }
        cx.notify();
    }

    /// 打开右侧日任务的编辑弹窗（仅日历创建的任务可编辑）。
    pub(super) fn open_calendar_task_edit_modal_action(
        &mut self,
        seed: CalendarTaskEditSeed,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.task_date_only = false;
        self.calendar_task_edit_plan_id = Some(seed.plan_id);
        self.calendar_task_edit_task_id = Some(seed.task_id);
        self.calendar_task_edit_title_input
            .update(cx, |state, cx| state.set_value(seed.title, window, cx));
        self.calendar_task_edit_duration_input
            .update(cx, |state, cx| {
                state.set_value((seed.portion / 60).to_string(), window, cx)
            });
        self.calendar_task_edit_date_input
            .update(cx, |state, cx| state.set_value(seed.date, window, cx));
        self.calendar_task_edit_modal_open = true;
        cx.notify();
    }

    /// 保存已编辑的日历任务。
    pub(super) fn save_calendar_task_edit_action(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(plan_id) = self.calendar_task_edit_plan_id.clone() else {
            return;
        };
        let Some(task_id) = self.calendar_task_edit_task_id.clone() else {
            return;
        };
        if self.task_date_only {
            let date = self.input_value(&self.calendar_task_edit_date_input, cx);
            match crate::core::move_study_task_to_date(&mut self.config, &plan_id, &task_id, &date)
            {
                Ok(()) => {
                    self.calendar_task_edit_modal_open = false;
                    window.push_notification(Notification::success("已调整任务日期"), cx);
                    self.trigger_auto_sync(window, cx);
                }
                Err(error) => window.push_notification(Notification::error(error), cx),
            }
            cx.notify();
            return;
        }
        let title = self.input_value(&self.calendar_task_edit_title_input, cx);
        let minutes_text = self.input_value(&self.calendar_task_edit_duration_input, cx);
        let date = self.input_value(&self.calendar_task_edit_date_input, cx);
        let date = date.trim().to_string();
        let minutes = match minutes_text.trim().parse::<i64>() {
            Ok(minutes) if minutes > 0 => minutes,
            _ => {
                window.push_notification(Notification::warning("任务时长必须是正整数分钟。"), cx);
                return;
            }
        };
        if chrono::NaiveDate::parse_from_str(&date, "%Y-%m-%d").is_err() {
            window.push_notification(Notification::warning("计划日期格式应为 YYYY-MM-DD。"), cx);
            return;
        }
        match update_calendar_task(&mut self.config, &plan_id, &task_id, &title, &date, minutes) {
            Ok(()) => {
                self.calendar_task_edit_modal_open = false;
                window.push_notification(Notification::success("已更新日历任务"), cx);
                self.trigger_auto_sync(window, cx);
            }
            Err(error) => window.push_notification(Notification::error(error), cx),
        }
        cx.notify();
    }

    /// 删除右侧的一项日历任务。若该任务是计划最后一项，空计划会一并移除。
    pub(super) fn delete_calendar_task_action(
        &mut self,
        plan_id: &str,
        task_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match delete_calendar_task(&mut self.config, plan_id, task_id) {
            Ok(()) => {
                window.push_notification(Notification::info("已删除日历任务"), cx);
                self.trigger_auto_sync(window, cx);
            }
            Err(error) => window.push_notification(Notification::error(error), cx),
        }
        cx.notify();
    }
}
