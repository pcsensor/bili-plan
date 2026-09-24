use super::*;

impl PlannerApp {
    /// 计划库中整体调整未完成任务日期的弹窗。
    pub(super) fn render_plan_reschedule_modal(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme().clone();
        let plan = self
            .plan_reschedule_plan_id
            .as_deref()
            .and_then(|plan_id| self.config.plans.iter().find(|plan| plan.id == plan_id));
        let plan_title = plan
            .map(|plan| plan.title.clone())
            .unwrap_or_else(|| "学习计划".to_string());
        let unfinished_count = plan
            .map(|plan| {
                plan.schedules
                    .iter()
                    .flat_map(|schedule| &schedule.tasks)
                    .filter(|task| !task.completed)
                    .count()
            })
            .unwrap_or(0);
        let unfinished_dates: Vec<&str> = plan
            .map(|plan| {
                plan.schedules
                    .iter()
                    .filter(|schedule| schedule.tasks.iter().any(|task| !task.completed))
                    .map(|schedule| schedule.date.as_str())
                    .collect()
            })
            .unwrap_or_default();
        let current_start = unfinished_dates.iter().copied().min().unwrap_or("-");
        let current_end = unfinished_dates.iter().copied().max().unwrap_or("-");

        div()
            .id("plan-reschedule-backdrop")
            .absolute()
            .inset_0()
            .bg(hsla(0.0, 0.0, 0.0, 0.6))
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.stop_propagation()),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|_, _, _, cx| cx.stop_propagation()),
            )
            .on_click(cx.listener(|_, _, _, cx| cx.stop_propagation()))
            .child(
                v_flex()
                    .id("plan-reschedule-modal")
                    .w(px(560.))
                    .bg(theme.background)
                    .border_2()
                    .border_color(theme.foreground)
                    .shadow_lg()
                    .p_6()
                    .gap_4()
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        Icon::empty()
                                            .path("icons/calendar-days.svg")
                                            .size_5()
                                            .text_color(theme.primary),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(16.))
                                            .font_weight(FontWeight::BOLD)
                                            .child(format!("调整《{plan_title}》未完成排期")),
                                    ),
                            )
                            .child(
                                div()
                                    .id("close-plan-reschedule")
                                    .cursor_pointer()
                                    .p_1()
                                    .child(
                                        Icon::empty()
                                            .path("icons/square.svg")
                                            .size_4()
                                            .text_color(theme.muted_foreground),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.plan_reschedule_modal_open = false;
                                        this.plan_reschedule_plan_id = None;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(theme.muted_foreground)
                            .child(format!(
                                "当前未完成排期：{current_start} 至 {current_end} · {unfinished_count} 项任务"
                            )),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(Self::field_label(
                                "新的未完成任务起始日期",
                                "YYYY-MM-DD",
                                cx,
                            ))
                            .child(Input::new(&self.plan_reschedule_date_input)),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .p_3()
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.primary.opacity(0.08))
                            .child(
                                "所有未完成日期批次会整体平移相同的自然日数，批次间隔和手动周末安排保持不变；已完成任务及其打卡日期不会移动。",
                            ),
                    )
                    .child(
                        h_flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("cancel-plan-reschedule")
                                    .label("取消")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.plan_reschedule_modal_open = false;
                                        this.plan_reschedule_plan_id = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("save-plan-reschedule")
                                    .primary()
                                    .label("保存排期")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.save_plan_reschedule_action(window, cx);
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// 编辑右侧日历任务的弹窗。
    pub(super) fn render_calendar_task_edit_modal(
        &self,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = cx.theme().clone();
        let plan_title = self
            .calendar_task_edit_plan_id
            .as_deref()
            .and_then(|plan_id| self.config.plans.iter().find(|plan| plan.id == plan_id))
            .map(|plan| plan.title.clone())
            .unwrap_or_else(|| "日历任务".to_string());

        div()
            .id("calendar-task-edit-backdrop")
            .absolute()
            .inset_0()
            .bg(hsla(0.0, 0.0, 0.0, 0.6))
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .on_click(cx.listener(|_, _, _, cx| {
                cx.stop_propagation();
            }))
            .child(
                v_flex()
                    .id("calendar-task-edit-modal")
                    .w(px(560.))
                    .bg(theme.background)
                    .border_2()
                    .border_color(theme.foreground)
                    .shadow_lg()
                    .p_6()
                    .gap_4()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _, _, cx| {
                            cx.stop_propagation();
                        }),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        Icon::empty()
                                            .path("icons/scissors.svg")
                                            .size_5()
                                            .text_color(theme.primary),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(16.))
                                            .font_weight(FontWeight::BOLD)
                                            .child(if self.task_date_only { format!("调整《{plan_title}》中的任务日期") } else { format!("编辑《{plan_title}》中的日历任务") }),
                                    ),
                            )
                            .child(
                                div()
                                    .id("close-calendar-task-edit")
                                    .cursor_pointer()
                                    .p_1()
                                    .child(
                                        Icon::empty()
                                            .path("icons/square.svg")
                                            .size_4()
                                            .text_color(theme.muted_foreground),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.calendar_task_edit_modal_open = false;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_3()
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(Self::field_label("任务名称", "", cx))
                                    .child(Input::new(&self.calendar_task_edit_title_input).disabled(self.task_date_only)),
                            )
                            .child(
                                h_flex()
                                    .gap_3()
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .gap_1()
                                            .child(Self::field_label("计划日期", "YYYY-MM-DD", cx))
                                            .child(Input::new(&self.calendar_task_edit_date_input)),
                                    )
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .gap_1()
                                            .child(Self::field_label("时长（分钟）", "正整数", cx))
                                            .child(Input::new(
                                                &self.calendar_task_edit_duration_input,
                                            ).disabled(self.task_date_only)),
                                    ),
                            ),
                    )
                    .children(self.task_date_only.then(|| div().text_sm().child(
                        "保留打卡与切片信息。向前移空原日期后，后续任务依次补位。允许指定周末；后续手动指定的日期优先于旧操作的撤销。"
                    )))
                    .child(
                        h_flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("cancel-calendar-task-edit")
                                    .label("取消")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.calendar_task_edit_modal_open = false;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("save-calendar-task-edit")
                                    .primary()
                                    .label("保存修改")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.save_calendar_task_edit_action(window, cx);
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// 日历格右键新增任务的弹窗。系列计划与普通视频计划使用同一计划库实体。
    pub(super) fn render_calendar_task_modal(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme().clone();
        let target = self.calendar_task_target;
        let existing_series: Vec<&StudyPlan> = self
            .config
            .plans
            .iter()
            .filter(|plan| plan.is_series && plan.show_in_library)
            .collect();

        let one_off_button = if target == CalendarTaskTarget::OneOff {
            Button::new("calendar-target-one-off")
                .small()
                .primary()
                .label("单日任务")
        } else {
            Button::new("calendar-target-one-off")
                .small()
                .ghost()
                .label("单日任务")
        }
        .on_click(cx.listener(|this, _, _, cx| {
            this.calendar_task_target = CalendarTaskTarget::OneOff;
            this.calendar_existing_series_id = None;
            cx.notify();
        }));
        let new_series_button = if target == CalendarTaskTarget::NewSeries {
            Button::new("calendar-target-new-series")
                .small()
                .primary()
                .label("新系列计划")
        } else {
            Button::new("calendar-target-new-series")
                .small()
                .ghost()
                .label("新系列计划")
        }
        .on_click(cx.listener(|this, _, _, cx| {
            this.calendar_task_target = CalendarTaskTarget::NewSeries;
            this.calendar_existing_series_id = None;
            cx.notify();
        }));
        let existing_series_button = if target == CalendarTaskTarget::ExistingSeries {
            Button::new("calendar-target-existing-series")
                .small()
                .primary()
                .label("加入已有系列")
        } else {
            Button::new("calendar-target-existing-series")
                .small()
                .ghost()
                .label("加入已有系列")
        }
        .on_click(cx.listener(|this, _, _, cx| {
            this.calendar_task_target = CalendarTaskTarget::ExistingSeries;
            cx.notify();
        }));

        div()
            .id("calendar-task-modal-backdrop")
            .absolute()
            .inset_0()
            .bg(hsla(0.0, 0.0, 0.0, 0.6))
            .flex()
            .items_center()
            .justify_center()
            // 遮罩层必须吞掉事件；否则关闭弹窗的鼠标抬起会点击到下方日历格。
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| {
                cx.stop_propagation();
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(|_, _, _, cx| {
                cx.stop_propagation();
            }))
            .on_click(cx.listener(|_, _, _, cx| {
                cx.stop_propagation();
            }))
            .child(
                v_flex()
                    .id("calendar-task-modal-scroll")
                    .w(px(640.))
                    .max_h(px(720.))
                    .overflow_y_scroll()
                    .bg(theme.background)
                    .border_2()
                    .border_color(theme.foreground)
                    .shadow_lg()
                    .p_6()
                    .gap_4()
                    .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| {
                        cx.stop_propagation();
                    }))
                    .on_mouse_down(MouseButton::Right, cx.listener(|_, _, _, cx| {
                        cx.stop_propagation();
                    }))
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        Icon::empty()
                                            .path("icons/calendar-days.svg")
                                            .size_5()
                                            .text_color(theme.primary),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(16.))
                                            .font_weight(FontWeight::BOLD)
                                            .child(format!(
                                                "为 {} 添加学习计划",
                                                format_date_with_weekday(&self.calendar_task_date)
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .id("close-calendar-task-modal")
                                    .cursor_pointer()
                                    .p_1()
                                    .child(
                                        Icon::empty()
                                            .path("icons/square.svg")
                                            .size_4()
                                            .text_color(theme.muted_foreground),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.calendar_task_modal_open = false;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .child(
                                v_flex()
                                    .flex_1()
                                    .gap_1()
                                    .child(Self::field_label(
                                        "当天任务名称",
                                        "会显示在日历与打卡面板中",
                                        cx,
                                    ))
                                    .child(Input::new(&self.calendar_task_title_input)),
                            )
                            .child(
                                v_flex()
                                    .w(px(150.))
                                    .gap_1()
                                    .child(Self::field_label(
                                        "计划日期",
                                        "可手动修改",
                                        cx,
                                    ))
                                    .child(Input::new(&self.calendar_task_date_input)),
                            )
                            .child(
                                v_flex()
                                    .w(px(150.))
                                    .gap_1()
                                    .child(Self::field_label("时长（分钟）", "正整数", cx))
                                    .child(Input::new(&self.calendar_task_duration_input)),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .child(Self::field_label(
                                "计划归属",
                                "系列计划会显示在“我的计划库”，后续可从任意日期继续追加任务",
                                cx,
                            ))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(one_off_button)
                                    .child(new_series_button)
                                    .child(existing_series_button),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(theme.muted_foreground)
                                    .child(match target {
                                        CalendarTaskTarget::OneOff => {
                                            "单日任务只参与指定日期的学习与打卡，不显示在计划库。"
                                        }
                                        CalendarTaskTarget::NewSeries => {
                                            "创建系列后，当前日期成为首个日程；以后添加的日期会自动延展计划时间范围。"
                                        }
                                        CalendarTaskTarget::ExistingSeries => {
                                            "任务将追加到选择的系列；可手动指定任意日期，系列起止时间会自动更新。"
                                        }
                                    }),
                            ),
                    )
                    .children((target == CalendarTaskTarget::NewSeries).then(|| {
                        v_flex()
                            .gap_1()
                            .child(Self::field_label(
                                "系列计划名称",
                                "例如：英语词汇冲刺",
                                cx,
                            ))
                            .child(Input::new(&self.calendar_series_name_input))
                    }))
                    .children((target == CalendarTaskTarget::ExistingSeries).then(|| {
                        if existing_series.is_empty() {
                            div()
                                .p_3()
                                .border_1()
                                .border_color(theme.border)
                                .text_size(px(12.5))
                                .text_color(theme.muted_foreground)
                                .child("还没有日历系列计划，请先选择“新系列计划”。")
                                .into_any_element()
                        } else {
                            let mut series_buttons = Vec::new();
                            for (index, plan) in existing_series.iter().enumerate() {
                                let plan_id = plan.id.clone();
                                let is_selected = self.calendar_existing_series_id.as_deref()
                                    == Some(plan_id.as_str());
                                let button = if is_selected {
                                    Button::new(("choose-calendar-series", index))
                                        .small()
                                        .primary()
                                        .label(format!("《{}》 · {} 至 {}", plan.title, plan.start_date, plan.end_date))
                                } else {
                                    Button::new(("choose-calendar-series", index))
                                        .small()
                                        .ghost()
                                        .label(format!("《{}》 · {} 至 {}", plan.title, plan.start_date, plan.end_date))
                                }
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.calendar_existing_series_id = Some(plan_id.clone());
                                    cx.notify();
                                }));
                                series_buttons.push(button.into_any_element());
                            }
                            v_flex()
                                .gap_1p5()
                                .child(
                                    div()
                                        .text_size(px(12.5))
                                        .font_weight(FontWeight::BOLD)
                                        .child("选择已有系列"),
                                )
                                .children(series_buttons)
                                .into_any_element()
                        }
                    }))
                    .child(
                        h_flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("cancel-calendar-task")
                                    .label("取消")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.calendar_task_modal_open = false;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("save-calendar-task")
                                    .primary()
                                    .label("添加任务")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.save_calendar_task_action(window, cx);
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }
}
