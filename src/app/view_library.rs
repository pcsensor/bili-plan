use super::*;

impl PlannerApp {
    /// 渲染我的计划库标签页。
    pub(super) fn render_my_plans_view(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme().clone();
        let library_plans: Vec<&StudyPlan> = self
            .config
            .plans
            .iter()
            .filter(|plan| plan.show_in_library)
            .collect();
        let total_plans = library_plans.len();
        let active_plans = library_plans
            .iter()
            .filter(|plan| plan.status == PlanStatus::Active)
            .count();

        let header = bcard(cx).child(
            h_flex()
                .items_center()
                .justify_between()
                .child(
                    v_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(20.))
                                .font_weight(FontWeight::BLACK)
                                .child("📚 我的学习计划库"),
                        )
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(theme.muted_foreground)
                                .child(format!(
                                    "共 {total_plans} 门科目 · 进行中 {active_plans} 门"
                                )),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("new-custom-task-btn")
                                .label("➕ 自定义任务")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.custom_task_form_open = !this.custom_task_form_open;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("new-plan-btn")
                                .primary()
                                .label("➕ 设立视频计划")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.active_tab = AppTab::PlanGenerator;
                                    cx.notify();
                                })),
                        ),
                ),
        );

        let custom_task_form = self.custom_task_form_open.then(|| {
            bcard(cx)
                .gap_3()
                .child(section_band(
                    "自定义每日任务",
                    "icons/calendar-days.svg",
                    cx,
                ))
                .child(
                    v_flex()
                        .gap_3()
                        .child(
                            h_flex()
                                .gap_3()
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .gap_1()
                                        .child(Self::field_label(
                                            "任务名称",
                                            "将显示在日历、打卡与机器人中",
                                            cx,
                                        ))
                                        .child(Input::new(&self.custom_title_input)),
                                )
                                .child(
                                    v_flex()
                                        .w(px(180.))
                                        .gap_1()
                                        .child(Self::field_label("开始日期", "YYYY-MM-DD", cx))
                                        .child(Input::new(&self.custom_start_date_input)),
                                ),
                        )
                        .child(
                            h_flex()
                                .gap_3()
                                .items_end()
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .gap_1()
                                        .child(Self::field_label(
                                            "执行天数",
                                            "连续安排多少个学习日",
                                            cx,
                                        ))
                                        .child(Input::new(&self.custom_days_input)),
                                )
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .gap_1()
                                        .child(Self::field_label(
                                            "每日时长（分钟）",
                                            "每一天自动生成一项任务",
                                            cx,
                                        ))
                                        .child(Input::new(&self.custom_duration_input)),
                                )
                                .child(
                                    Button::new("toggle-custom-weekend")
                                        .label(if self.custom_skip_weekends_toggle {
                                            "跳过周末：是 ✅"
                                        } else {
                                            "跳过周末：否 ⬜"
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.custom_skip_weekends_toggle =
                                                !this.custom_skip_weekends_toggle;
                                            cx.notify();
                                        })),
                                ),
                        )
                        .child(
                            h_flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    Button::new("cancel-custom-task")
                                        .ghost()
                                        .label("取消")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.custom_task_form_open = false;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Button::new("create-custom-task")
                                        .primary()
                                        .label("创建并加入打卡")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.create_custom_task_action(window, cx);
                                        })),
                                ),
                        ),
                )
                .into_any_element()
        });

        let plan_cards: gpui::Div = if library_plans.is_empty() {
            bcard(cx)
                .items_center()
                .py_12()
                .gap_3()
                .child(
                    Icon::empty()
                        .path("icons/table.svg")
                        .size_10()
                        .text_color(theme.muted_foreground),
                )
                .child(
                    div()
                        .text_size(px(16.))
                        .font_weight(FontWeight::BOLD)
                        .child("暂无任何学习计划"),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(theme.muted_foreground)
                        .child("点击下方按钮立即创建您的第一个多科目学习打卡计划。"),
                )
                .child(
                    Button::new("empty-create")
                        .primary()
                        .label("🚀 前往计划生成器")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.active_tab = AppTab::PlanGenerator;
                            cx.notify();
                        })),
                )
        } else {
            let mut cards = Vec::new();
            for (p_idx, plan) in library_plans.iter().enumerate() {
                let (done_cnt, total_cnt, done_dur, total_dur, ratio) = compute_plan_progress(plan);
                let pid = plan.id.clone();
                let pid_del = plan.id.clone();
                let pid_push = plan.id.clone();
                let pid_reschedule = plan.id.clone();
                let pid_redistribute = plan.id.clone();

                let status_badge_bg = match plan.status {
                    PlanStatus::Active => theme.primary,
                    PlanStatus::Paused => theme.muted,
                    PlanStatus::Completed => theme.success,
                    PlanStatus::Archived => theme.muted,
                };
                let status_badge_text = match plan.status {
                    PlanStatus::Active => hsla(0.0, 0.0, 0.04, 1.0),
                    PlanStatus::Paused => theme.muted_foreground,
                    PlanStatus::Completed => hsla(0.0, 0.0, 1.0, 1.0),
                    PlanStatus::Archived => theme.muted_foreground,
                };

                let card = bcard(cx)
                    .gap_3()
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_size(px(16.))
                                            .font_weight(FontWeight::BOLD)
                                            .child(format!("《{}》", plan.title)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.5))
                                            .font_weight(FontWeight::BOLD)
                                            .px_1p5()
                                            .py_0p5()
                                            .bg(theme.muted)
                                            .child(plan.source_type.to_uppercase()),
                                    )
                                    .children(plan.is_series.then(|| {
                                        div()
                                            .text_size(px(10.5))
                                            .font_weight(FontWeight::BOLD)
                                            .px_1p5()
                                            .py_0p5()
                                            .bg(theme.primary.opacity(0.2))
                                            .child("系列计划")
                                    }))
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .font_weight(FontWeight::BOLD)
                                            .px_2()
                                            .py_0p5()
                                            .bg(status_badge_bg)
                                            .text_color(status_badge_text)
                                            .child(plan.status.label()),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        Button::new(("pause-plan", p_idx))
                                            .small()
                                            .label(if plan.status == PlanStatus::Paused {
                                                "▶️ 继续"
                                            } else {
                                                "⏸️ 暂停"
                                            })
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.toggle_plan_status_action(&pid, window, cx);
                                            })),
                                    )
                                    .child(
                                        Button::new(("reschedule-plan", p_idx))
                                            .small()
                                            .label("📅 调整未完成排期")
                                            .disabled(done_cnt >= total_cnt)
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.open_plan_reschedule_action(
                                                    &pid_reschedule,
                                                    window,
                                                    cx,
                                                );
                                            })),
                                    )
                                    .child(
                                        Button::new(("redistribute-plan", p_idx))
                                            .small()
                                            .label("📆 调整结束日期")
                                            .disabled(done_cnt >= total_cnt)
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.open_plan_redistribute_action(
                                                    &pid_redistribute,
                                                    window,
                                                    cx,
                                                );
                                            })),
                                    )
                                    .child(
                                        Button::new(("push-single", p_idx))
                                            .small()
                                            .label("🔄 顺延上日未完成")
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.push_forward_single_plan_action(
                                                    &pid_push, window, cx,
                                                );
                                            })),
                                    )
                                    .child(
                                        Button::new(("del-plan", p_idx))
                                            .small()
                                            .label("🗑️ 删除")
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.delete_plan_action(&pid_del, window, cx);
                                            })),
                                    ),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_1p5()
                            .child(
                                h_flex()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_size(px(12.5))
                                            .text_color(theme.muted_foreground)
                                            .child(format!(
                                                "范围：{} · 排期：{} 至 {}（{} 天 · {}）",
                                                plan.scope_desc,
                                                plan.start_date,
                                                plan.end_date,
                                                plan.planned_days,
                                                if plan.skip_weekends {
                                                    "跳过周末"
                                                } else {
                                                    "连续每日"
                                                }
                                            )),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.5))
                                            .font_weight(FontWeight::BOLD)
                                            .child(format!(
                                            "进度：{done_cnt}/{total_cnt} 视频 · {} / {} ({:.1}%)",
                                            fmt_seconds(done_dur as f64, true),
                                            fmt_seconds(total_dur as f64, true),
                                            ratio * 100.0
                                        )),
                                    ),
                            )
                            // 野兽风进度条：墨色边框 + 明黄填充
                            .child(
                                div()
                                    .w_full()
                                    .h(px(12.))
                                    .border_2()
                                    .border_color(theme.foreground)
                                    .bg(theme.background)
                                    .child(
                                        div()
                                            .h_full()
                                            .w(gpui::DefiniteLength::Fraction(ratio as f32))
                                            .bg(theme.primary),
                                    ),
                            ),
                    );

                cards.push(card.into_any_element());
            }

            v_flex().w_full().gap_4().children(cards)
        };

        v_flex()
            .id("myplans-scroll")
            .size_full()
            .overflow_y_scroll()
            .px_8()
            .py_6()
            .gap_5()
            .child(entrance("anim-myplans-head", 0.0, header))
            .children(custom_task_form)
            .child(entrance("anim-myplans-cards", 0.1, plan_cards))
            .into_any_element()
    }
}
