use super::*;

impl PlannerApp {
    /// 渲染今日打卡板块（多科目叠加看板与任务打卡）。
    pub(super) fn render_today_checkin_view(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme().clone();
        let stats = compute_study_stats(&self.config.plans, &self.selected_date);
        let remaining_duration = stats
            .today_total_duration
            .saturating_sub(stats.today_completed_duration)
            .max(0);

        // 1. 统计概览卡片 (Neo-Brutalist 三列硬阴影卡片)
        let stats_row = h_flex()
            .w_full()
            .gap_4()
            .child(
                bcard(cx)
                    .flex_1()
                    .gap_1()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Icon::empty()
                                    .path("icons/square-check.svg")
                                    .size_4()
                                    .text_color(theme.primary),
                            )
                            .child(
                                Label::new("今日任务")
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::BOLD),
                            ),
                    )
                    .child(
                        h_flex()
                            .items_baseline()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(26.))
                                    .font_weight(FontWeight::BLACK)
                                    .child(format!(
                                        "{}/{}",
                                        stats.today_completed_tasks, stats.today_total_tasks
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme.muted_foreground)
                                    .child(if stats.today_total_tasks > 0 {
                                        format!(
                                            "完成率 {:.0}%",
                                            (stats.today_completed_tasks as f64
                                                / stats.today_total_tasks as f64)
                                                * 100.0
                                        )
                                    } else {
                                        "无安排".to_string()
                                    }),
                            ),
                    ),
            )
            .child(
                bcard(cx)
                    .flex_1()
                    .gap_1()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Icon::empty()
                                    .path("icons/clock.svg")
                                    .size_4()
                                    .text_color(theme.primary),
                            )
                            .child(
                                Label::new("今日学习时长")
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::BOLD),
                            ),
                    )
                    .child(
                        h_flex()
                            .items_baseline()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(22.))
                                    .font_weight(FontWeight::BLACK)
                                    .child(fmt_seconds(stats.today_total_duration as f64, true)),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme.muted_foreground)
                                    .child(format!(
                                        "已学 {}",
                                        fmt_seconds(stats.today_completed_duration as f64, true)
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.primary)
                            .child(format!(
                                "今日剩余学习时长：{}",
                                fmt_seconds(remaining_duration as f64, true)
                            )),
                    ),
            )
            .child(
                bcard(cx)
                    .flex_1()
                    .gap_1()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Icon::empty()
                                    .path("icons/calendar-days.svg")
                                    .size_4()
                                    .text_color(theme.primary),
                            )
                            .child(
                                Label::new("连续学习 Streak")
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::BOLD),
                            ),
                    )
                    .child(
                        h_flex()
                            .items_baseline()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(26.))
                                    .font_weight(FontWeight::BLACK)
                                    .child(format!("🔥 {} 天", stats.current_streak)),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme.muted_foreground)
                                    .child(format!("累计打卡 {} 天", stats.total_days_checked_in)),
                            ),
                    ),
            );

        // 2. 日期选择导航条
        let today = today_date_str();
        let is_today = self.selected_date == today;
        let is_future = self.selected_date > today;
        let future_date_for_advance = self.selected_date.clone();
        let date_nav =
            bcard(cx).child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_3()
                            .child(Button::new("prev-date").label("◀ 前一天").on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.prev_date_action(cx);
                                }),
                            ))
                            .child(
                                div()
                                    .text_size(px(16.))
                                    .font_weight(FontWeight::BOLD)
                                    .child(format_date_with_weekday(&self.selected_date)),
                            )
                            .child(Button::new("next-date").label("后一天 ▶").on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.next_date_action(cx);
                                }),
                            ))
                            .children((!is_today).then(|| {
                                Button::new("back-today")
                                    .primary()
                                    .label("回到今天")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.reset_today_action(cx);
                                    }))
                            })),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .children(is_future.then(|| {
                                Button::new("advance-completed-tasks")
                                    .icon(Icon::empty().path("icons/clock.svg"))
                                    .label(if self.filter_plan_id.is_some() {
                                        "提前本科目已完成任务"
                                    } else {
                                        "提前全部科目已完成任务"
                                    })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        let selected_plan = this.filter_plan_id.clone();
                                        this.advance_completed_tasks_action(
                                            &future_date_for_advance,
                                            selected_plan.as_deref(),
                                            window,
                                            cx,
                                        );
                                    }))
                            }))
                            .child(
                                Button::new("push-forward-all")
                                    .icon(Icon::empty().path("icons/refresh-cw.svg"))
                                    .label("顺延全部科目积压任务")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.push_forward_all_behind_action(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("add-plan-quick")
                                    .primary()
                                    .label("➕ 添加新科目")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.active_tab = AppTab::PlanGenerator;
                                        cx.notify();
                                    })),
                            ),
                    ),
            );

        // 3. 多科目任务聚合
        let all_tasks = get_tasks_for_date(&self.config.plans, &self.selected_date);

        let filtered_tasks: Vec<_> = if let Some(fid) = &self.filter_plan_id {
            all_tasks
                .into_iter()
                .filter(|t| &t.plan_id == fid)
                .collect()
        } else {
            all_tasks
        };

        // 科目过滤标签
        let active_plans: Vec<_> = self
            .config
            .plans
            .iter()
            .filter(|p| p.status == PlanStatus::Active)
            .collect();
        let filter_bar = if active_plans.len() > 1 {
            let mut buttons = Vec::new();
            let is_all = self.filter_plan_id.is_none();
            buttons.push(
                Button::new("filter-all")
                    .label("全部科目")
                    .selected(is_all)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.filter_plan_id = None;
                        cx.notify();
                    }))
                    .into_any_element(),
            );
            for (idx, p) in active_plans.iter().enumerate() {
                let pid = p.id.clone();
                let is_sel = self.filter_plan_id.as_deref() == Some(&pid);
                let title = p.title.clone();
                buttons.push(
                    Button::new(("filter-p", idx))
                        .label(title)
                        .selected(is_sel)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.filter_plan_id = Some(pid.clone());
                            cx.notify();
                        }))
                        .into_any_element(),
                );
            }
            Some(h_flex().gap_2().items_center().children(buttons))
        } else {
            None
        };

        // 任务列表分组渲染
        let task_content: gpui::Div = if self.config.plans.is_empty() {
            bcard(cx)
                .items_center()
                .py_10()
                .gap_3()
                .child(
                    Icon::empty()
                        .path("icons/calendar-days.svg")
                        .size_10()
                        .text_color(theme.muted_foreground),
                )
                .child(
                    div()
                        .text_size(px(16.))
                        .font_weight(FontWeight::BOLD)
                        .child("暂无进行中的学习计划"),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(theme.muted_foreground)
                        .child(
                        "请前往「计划生成器」输入 B 站或 Jellyfin 链接创建您的第一个学习打卡计划！",
                    ),
                )
                .child(
                    Button::new("go-generator")
                        .primary()
                        .label("🚀 前往计划生成器")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.active_tab = AppTab::PlanGenerator;
                            cx.notify();
                        })),
                )
        } else if filtered_tasks.is_empty() {
            bcard(cx)
                .items_center()
                .py_10()
                .gap_3()
                .child(
                    Icon::empty()
                        .path("icons/square-check.svg")
                        .size_10()
                        .text_color(theme.primary),
                )
                .child(
                    div()
                        .text_size(px(16.))
                        .font_weight(FontWeight::BOLD)
                        .child("🎉 今日无学习任务安排或为休息日！"),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(theme.muted_foreground)
                        .child("适当休息劳逸结合，保持最佳学习状态。"),
                )
        } else {
            // 按科目分组展示
            let mut plan_map: std::collections::BTreeMap<String, Vec<crate::study::TodayTaskView>> =
                std::collections::BTreeMap::new();
            for item in filtered_tasks {
                plan_map.entry(item.plan_id.clone()).or_default().push(item);
            }

            let mut group_cards = Vec::new();
            for (grp_idx, (plan_id, items)) in plan_map.into_iter().enumerate() {
                let plan_title = items[0].plan_title.clone();
                let day_display = items[0].day_display.clone();
                let source_type = items[0].source_type.clone();
                let source_url = items[0].source_url.clone();
                let total_in_group = items.len();
                let total_group_duration: i64 = items.iter().map(|t| t.task.portion).sum();
                let remaining_group_duration: i64 = items
                    .iter()
                    .filter(|t| !t.task.completed)
                    .map(|t| t.task.portion)
                    .sum();
                let done_in_group = items.iter().filter(|t| t.task.completed).count();
                let is_all_group_done = total_in_group > 0 && done_in_group == total_in_group;

                let pid_clone = plan_id.clone();
                let sel_date_clone = self.selected_date.clone();

                let mut task_rows = Vec::new();
                for (item_idx, view) in items.into_iter().enumerate() {
                    let task = view.task;
                    let tid = task.id.clone();
                    let st = source_type.clone();
                    let su = source_url.clone();
                    let has_source_url = !su.trim().is_empty();
                    let vno = task.vid_no;
                    let is_done = task.completed;

                    let (chk_icon, chk_color) = if is_done {
                        ("icons/square-check.svg", theme.primary)
                    } else {
                        ("icons/square.svg", theme.muted_foreground)
                    };

                    let btn_play_id = ("play-task", grp_idx * 1000 + item_idx);
                    let btn_chk_id = ("check-btn", grp_idx * 1000 + item_idx);
                    let pid_click1 = plan_id.clone();
                    let pid_click2 = plan_id.clone();
                    let tid_click1 = tid.clone();
                    let tid_click2 = tid.clone();
                    let move_seed = CalendarTaskEditSeed {
                        plan_id: plan_id.clone(),
                        task_id: tid.clone(),
                        title: task.title.clone(),
                        date: self.selected_date.clone(),
                        portion: task.portion,
                    };

                    task_rows.push(
                        h_flex()
                            .id(("task-row", grp_idx * 1000 + item_idx))
                            .items_center()
                            .justify_between()
                            .px_3()
                            .py_2p5()
                            .border_1()
                            .border_color(if is_done {
                                theme.primary.opacity(0.4)
                            } else {
                                theme.border.opacity(0.6)
                            })
                            .bg(if is_done {
                                theme.primary.opacity(0.08)
                            } else {
                                theme.background.opacity(0.25)
                            })
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_3()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .id(("chk-icon", grp_idx * 1000 + item_idx))
                                            .cursor_pointer()
                                            .p_1()
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.toggle_task_checkin_action(
                                                    &pid_click1,
                                                    &tid_click1,
                                                    window,
                                                    cx,
                                                );
                                            }))
                                            .child(
                                                Icon::empty()
                                                    .path(chk_icon)
                                                    .size_5()
                                                    .text_color(chk_color),
                                            ),
                                    )
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .gap_0p5()
                                            .child(
                                                h_flex()
                                                    .items_center()
                                                    .gap_2()
                                                    .child(
                                                        div()
                                                            .text_size(px(13.5))
                                                            .font_weight(FontWeight::BOLD)
                                                            .text_color(if is_done {
                                                                theme.muted_foreground
                                                            } else {
                                                                theme.foreground
                                                            })
                                                            .child(format!(
                                                                "P{}: {}",
                                                                task.vid_no, task.title
                                                            )),
                                                    )
                                                    .children(task.from_prev.then(|| {
                                                        div()
                                                            .text_size(px(10.5))
                                                            .px_1p5()
                                                            .py_0p5()
                                                            .bg(theme.accent.opacity(0.2))
                                                            .child("接上一日")
                                                    }))
                                                    .children((task.remainder > 0).then(|| {
                                                        div()
                                                            .text_size(px(10.5))
                                                            .px_1p5()
                                                            .py_0p5()
                                                            .bg(theme.primary.opacity(0.2))
                                                            .child("顺延至次日")
                                                    })),
                                            )
                                            .child(
                                                h_flex()
                                                    .gap_3()
                                                    .items_center()
                                                    .child(
                                                        div()
                                                            .text_size(px(11.5))
                                                            .text_color(theme.muted_foreground)
                                                            .child(format!(
                                                                "⏱️ 本日任务时长：{}",
                                                                fmt_seconds(
                                                                    task.portion as f64,
                                                                    true
                                                                )
                                                            )),
                                                    )
                                                    .children((task.remainder > 0).then(|| {
                                                        div()
                                                            .text_size(px(11.5))
                                                            .text_color(theme.muted_foreground)
                                                            .child(format!(
                                                                "剩余顺延：{}",
                                                                fmt_seconds(
                                                                    task.remainder as f64,
                                                                    true
                                                                )
                                                            ))
                                                    })),
                                            ),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        Button::new(("move-task", grp_idx * 1000 + item_idx))
                                            .small()
                                            .ghost()
                                            .label("调整日期")
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.open_calendar_task_edit_modal_action(
                                                    move_seed.clone(),
                                                    window,
                                                    cx,
                                                );
                                                this.task_date_only = true;
                                            })),
                                    )
                                    .children(has_source_url.then(|| {
                                        Button::new(btn_play_id)
                                            .icon(Icon::empty().path("icons/play.svg"))
                                            .label("直达播放")
                                            .small()
                                            .on_click(move |_, _, _| {
                                                open_video_link(&st, &su, vno);
                                            })
                                    }))
                                    .child(
                                        Button::new(btn_chk_id)
                                            .small()
                                            .label(if is_done { "已打卡" } else { "打卡" })
                                            .primary()
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.toggle_task_checkin_action(
                                                    &pid_click2,
                                                    &tid_click2,
                                                    window,
                                                    cx,
                                                );
                                            })),
                                    ),
                            )
                            .into_any_element(),
                    );
                }

                group_cards.push(
                    bcard(cx)
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
                                                .path(source_badge(&source_type).0)
                                                .size_4()
                                                .text_color(theme.primary),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(15.))
                                                .font_weight(FontWeight::BOLD)
                                                .child(format!("《{plan_title}》 · {day_display}")),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(11.5))
                                                .px_2()
                                                .py_0p5()
                                                .bg(if is_all_group_done {
                                                    theme.primary
                                                } else {
                                                    theme.muted
                                                })
                                                .text_color(if is_all_group_done {
                                                    hsla(0.0, 0.0, 0.04, 1.0)
                                                } else {
                                                    theme.muted_foreground
                                                })
                                                .child(format!(
                                                    "完成 {done_in_group}/{total_in_group}"
                                                )),
                                        ),
                                )
                                .child(
                                    h_flex()
                                        .items_center()
                                        .gap_3()
                                        .flex_shrink_0()
                                        .child(
                                            v_flex()
                                                .gap_0p5()
                                                .text_size(px(12.5))
                                                .child(
                                                    div().text_color(theme.muted_foreground).child(
                                                        format!(
                                                            "⏱️ 当日总时长：{}",
                                                            fmt_seconds(
                                                                total_group_duration as f64,
                                                                true
                                                            )
                                                        ),
                                                    ),
                                                )
                                                .child(div().text_color(theme.primary).child(
                                                    format!(
                                                        "当日剩余时长：{}",
                                                        fmt_seconds(
                                                            remaining_group_duration as f64,
                                                            true
                                                        )
                                                    ),
                                                )),
                                        )
                                        .children((!is_all_group_done).then(|| {
                                            Button::new(("check-all-grp", grp_idx))
                                                .small()
                                                .label("一键打卡本科目今日")
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.checkin_entire_day_action(
                                                            &pid_clone,
                                                            &sel_date_clone,
                                                            window,
                                                            cx,
                                                        );
                                                    },
                                                ))
                                        })),
                                ),
                        )
                        .child(v_flex().w_full().gap_2().children(task_rows))
                        .into_any_element(),
                );
            }

            v_flex().w_full().gap_4().children(group_cards)
        };

        v_flex()
            .id("today-scroll")
            .size_full()
            .overflow_y_scroll()
            .px_8()
            .py_6()
            .gap_5()
            .child(entrance("anim-stats", 0.0, stats_row))
            .child(entrance("anim-nav", 0.08, date_nav))
            .children(filter_bar.map(|f| entrance("anim-filter", 0.14, f).into_any_element()))
            .child(entrance("anim-tasks", 0.18, task_content))
            .into_any_element()
    }
}
