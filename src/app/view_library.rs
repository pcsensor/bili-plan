use super::*;

/// 计划库卡片的展示模型：列表回调里不再借用 `config`。
struct PlanCardModel {
    id: String,
    title: String,
    source_type: String,
    is_series: bool,
    status: PlanStatus,
    scope_desc: String,
    start_date: String,
    end_date: String,
    planned_days: usize,
    skip_weekends: bool,
    done_cnt: usize,
    total_cnt: usize,
    done_dur: i64,
    total_dur: i64,
    ratio: f64,
}

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

        let body: gpui::AnyElement = if library_plans.is_empty() {
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
                .into_any_element()
        } else {
            let cards: Vec<PlanCardModel> = library_plans
                .iter()
                .map(|plan| {
                    let (done_cnt, total_cnt, done_dur, total_dur, ratio) =
                        compute_plan_progress(plan);
                    PlanCardModel {
                        id: plan.id.clone(),
                        title: plan.title.clone(),
                        source_type: plan.source_type.clone(),
                        is_series: plan.is_series,
                        status: plan.status,
                        scope_desc: plan.scope_desc.clone(),
                        start_date: plan.start_date.clone(),
                        end_date: plan.end_date.clone(),
                        planned_days: plan.planned_days,
                        skip_weekends: plan.skip_weekends,
                        done_cnt,
                        total_cnt,
                        done_dur,
                        total_dur,
                        ratio,
                    }
                })
                .collect();

            let count = cards.len();
            if self.my_plans_list.item_count() != count {
                self.my_plans_list.reset(count);
            }
            let this = cx.entity().downgrade();
            gpui::list(self.my_plans_list.clone(), move |ix, _window, cx| {
                Self::render_library_plan_card(ix, &cards, &this, cx)
            })
            .w_full()
            .flex_1()
            .min_h_0()
            .into_any_element()
        };

        v_flex()
            .id("myplans-root")
            .size_full()
            .px_8()
            .py_6()
            .gap_5()
            .child(entrance("anim-myplans-head", 0.0, header))
            .children(custom_task_form)
            .child(body)
            .into_any_element()
    }

    /// 单张计划卡（供虚拟列表按需构建）。
    fn render_library_plan_card(
        ix: usize,
        cards: &[PlanCardModel],
        this: &gpui::WeakEntity<Self>,
        cx: &mut gpui::App,
    ) -> gpui::AnyElement {
        let Some(card) = cards.get(ix) else {
            return div().into_any_element();
        };
        let theme = cx.theme().clone();
        let idx = ix;
        let id = card.id.clone();
        let id_del = card.id.clone();
        let id_push = card.id.clone();
        let id_reschedule = card.id.clone();
        let id_redistribute = card.id.clone();
        let done_cnt = card.done_cnt;
        let total_cnt = card.total_cnt;

        let status_badge_bg = match card.status {
            PlanStatus::Active => theme.primary,
            PlanStatus::Paused => theme.muted,
            PlanStatus::Completed => theme.success,
            PlanStatus::Archived => theme.muted,
        };
        let status_badge_text = match card.status {
            PlanStatus::Active => hsla(0.0, 0.0, 0.04, 1.0),
            PlanStatus::Paused => theme.muted_foreground,
            PlanStatus::Completed => hsla(0.0, 0.0, 1.0, 1.0),
            PlanStatus::Archived => theme.muted_foreground,
        };

        bcard(cx)
            .gap_3()
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(16.))
                                    .font_weight(FontWeight::BOLD)
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(format!("《{}》", card.title)),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .font_weight(FontWeight::BOLD)
                                    .px_1p5()
                                    .py_0p5()
                                    .bg(theme.muted)
                                    .child(card.source_type.to_uppercase()),
                            )
                            .children(card.is_series.then(|| {
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
                                    .child(card.status.label()),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_shrink_0()
                            .child(
                                Button::new(("pause-plan", idx))
                                    .small()
                                    .label(if card.status == PlanStatus::Paused {
                                        "▶️ 继续"
                                    } else {
                                        "⏸️ 暂停"
                                    })
                                    .on_click(Self::list_action(this, move |view, window, cx| {
                                        view.toggle_plan_status_action(&id, window, cx);
                                    })),
                            )
                            .child(
                                Button::new(("reschedule-plan", idx))
                                    .small()
                                    .label("📅 调整未完成排期")
                                    .disabled(done_cnt >= total_cnt)
                                    .on_click(Self::list_action(this, move |view, window, cx| {
                                        view.open_plan_reschedule_action(
                                            &id_reschedule,
                                            window,
                                            cx,
                                        );
                                    })),
                            )
                            .child(
                                Button::new(("redistribute-plan", idx))
                                    .small()
                                    .label("📆 调整结束日期")
                                    .disabled(done_cnt >= total_cnt)
                                    .on_click(Self::list_action(this, move |view, window, cx| {
                                        view.open_plan_redistribute_action(
                                            &id_redistribute,
                                            window,
                                            cx,
                                        );
                                    })),
                            )
                            .child(
                                Button::new(("push-single", idx))
                                    .small()
                                    .label("🔄 顺延上日未完成")
                                    .on_click(Self::list_action(this, move |view, window, cx| {
                                        view.push_forward_single_plan_action(&id_push, window, cx);
                                    })),
                            )
                            .child(
                                Button::new(("del-plan", idx))
                                    .small()
                                    .label("🗑️ 删除")
                                    .on_click(Self::list_action(this, move |view, window, cx| {
                                        view.delete_plan_action(&id_del, window, cx);
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
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(format!(
                                        "范围：{} · 排期：{} 至 {}（{} 天 · {}）",
                                        card.scope_desc,
                                        card.start_date,
                                        card.end_date,
                                        card.planned_days,
                                        if card.skip_weekends {
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
                                        "进度：{}/{} 视频 · {} / {} ({:.1}%)",
                                        card.done_cnt,
                                        card.total_cnt,
                                        fmt_seconds(card.done_dur as f64, true),
                                        fmt_seconds(card.total_dur as f64, true),
                                        card.ratio * 100.0
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
                                    .w(gpui::DefiniteLength::Fraction(card.ratio as f32))
                                    .bg(theme.primary),
                            ),
                    ),
            )
            .into_any_element()
    }
}
