use super::*;

impl PlannerApp {
    /// 渲染学习日历视图（月度学习看板 + 手动备忘录 + 选中日期右侧明细栏）。
    pub(super) fn render_calendar_view(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme().clone();
        let year = self.calendar_year;
        let month = self.calendar_month;
        let matrix = generate_month_calendar_matrix(year, month, &self.config.plans);
        let month_stats = compute_month_study_stats(year, month, &self.config.plans);
        let selected_date = self.calendar_selected_date.clone();
        let selected_tasks = get_tasks_for_date(&self.config.plans, &selected_date);
        let selected_notes = get_daily_notes(&self.config, &selected_date);

        // 1. 左侧：月度日历看板
        // 1.1 月份导航与本月统计条
        let month_nav_bar = bcard(cx).child(
            h_flex()
                .items_center()
                .justify_between()
                .child(
                    h_flex()
                        .items_center()
                        .gap_3()
                        .child(Button::new("cal-prev-month").label("◀ 上个月").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.prev_calendar_month_action(cx);
                            }),
                        ))
                        .child(
                            div()
                                .text_size(px(18.))
                                .font_weight(FontWeight::BLACK)
                                .child(format!("{year} 年 {month} 月")),
                        )
                        .child(Button::new("cal-next-month").label("下个月 ▶").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.next_calendar_month_action(cx);
                            }),
                        ))
                        .child(
                            Button::new("cal-today-month")
                                .small()
                                .primary()
                                .label("回到本月")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.reset_calendar_month_action(window, cx);
                                })),
                        ),
                )
                .child(
                    h_flex()
                        .items_center()
                        .gap_3()
                        .text_size(px(12.5))
                        .text_color(theme.muted_foreground)
                        .child(format!(
                            "⏱️ 规划 {} (已学 {})",
                            fmt_seconds(month_stats.total_duration as f64, true),
                            fmt_seconds(month_stats.completed_duration as f64, true)
                        ))
                        .child(format!(
                            "📚 任务 {}/{}",
                            month_stats.completed_tasks, month_stats.total_tasks
                        ))
                        .child(format!("🔥 活跃 {} 天", month_stats.active_study_days)),
                ),
        );

        // 1.2 星期表头 (周一 ~ 周日)
        // 1.2 星期表头 (周一 ~ 周日)
        let weekdays = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];
        let weekday_headers =
            h_flex()
                .w_full()
                .gap_1p5()
                .children(weekdays.iter().enumerate().map(|(idx, &w)| {
                    let is_wkend = idx >= 5;
                    div()
                        .flex_1()
                        .min_w_0()
                        .py_1p5()
                        .items_center()
                        .justify_center()
                        .flex()
                        .bg(if is_wkend {
                            theme.primary.opacity(0.12)
                        } else {
                            theme.muted.opacity(0.4)
                        })
                        .border_2()
                        .border_color(theme.border)
                        .text_size(px(12.5))
                        .font_weight(FontWeight::BOLD)
                        .text_color(if is_wkend {
                            theme.primary
                        } else {
                            theme.foreground
                        })
                        .child(w)
                }));

        // 1.3 日历网格主体 (按 7 列分行)
        let mut grid_rows = Vec::new();
        for (row_idx, chunk) in matrix.chunks(7).enumerate() {
            let mut row_cells = Vec::new();
            for (col_idx, day) in chunk.iter().enumerate() {
                let cell_index = row_idx * 7 + col_idx;

                // 若非当前所选月份，仅渲染尺寸完全一致的透明占位格，保证每列宽度严格对齐
                if !day.is_current_month {
                    let placeholder_el = div()
                        .id(("cal-ph", cell_index))
                        .flex_1()
                        .min_w_0()
                        .h(px(CALENDAR_CELL_HEIGHT))
                        .p_1p5()
                        .border_2()
                        .border_color(hsla(0.0, 0.0, 0.0, 0.0));
                    row_cells.push(placeholder_el.into_any_element());
                    continue;
                }

                let d_date = day.date.clone();
                let right_click_date = d_date.clone();
                let is_sel = d_date == selected_date;
                let is_today = day.is_today;
                let notes = get_daily_notes(&self.config, &d_date);
                let note_count = notes.len();
                let has_note = note_count > 0;
                // 课程与备注按可用容量一起排布：尽可能展示，超过格子容量时
                // 留一行明确告诉用户还有多少项可通过右侧详情查看。
                let mut summary_lines: Vec<(String, bool)> = day
                    .plan_titles
                    .iter()
                    .map(|title| (format!("• {title}"), false))
                    .collect();
                summary_lines.extend(notes.iter().map(|note| {
                    let preview: String = note.content.chars().take(18).collect();
                    (format!("📝 {preview}"), true)
                }));
                let display_count = if summary_lines.len() > CALENDAR_CELL_MAX_SUMMARY_LINES {
                    CALENDAR_CELL_MAX_SUMMARY_LINES - 1
                } else {
                    summary_lines.len()
                };
                let hidden_count = summary_lines.len().saturating_sub(display_count);
                summary_lines.truncate(display_count);
                if hidden_count > 0 {
                    summary_lines.push((format!("… 还有 {hidden_count} 项"), true));
                }

                let bg_color = if is_sel {
                    theme.primary.opacity(0.18)
                } else if is_today {
                    theme.primary.opacity(0.08)
                } else {
                    theme.background.opacity(0.35)
                };

                let border_color = if is_sel {
                    theme.primary
                } else if is_today {
                    theme.primary.opacity(0.6)
                } else {
                    theme.border.opacity(0.6)
                };

                let is_all_done = day.completed_tasks > 0 && day.completed_tasks == day.total_tasks;
                let status_bg = if is_all_done {
                    theme.primary.opacity(0.9)
                } else if day.completed_tasks > 0 {
                    theme.primary.opacity(0.4)
                } else {
                    theme.muted.opacity(0.6)
                };
                let status_text_color = if is_all_done {
                    hsla(0.0, 0.0, 0.04, 1.0)
                } else {
                    theme.foreground
                };

                let cell_el = div()
                    .id(("cal-cell", cell_index))
                    .flex_1()
                    .min_w_0()
                    .h(px(CALENDAR_CELL_HEIGHT))
                    .p_1p5()
                    .gap_1()
                    // 内容不足时摘要区贴底；新增计划或备注会追加在底部列表末尾，
                    // 内容较多时再自然向上占用可用空间。
                    .justify_between()
                    .cursor_pointer()
                    .bg(bg_color)
                    .border_2()
                    .border_color(border_color)
                    .overflow_hidden()
                    .on_click(
                        cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                            this.select_calendar_date_action(&d_date, window, cx);
                            if event.click_count() == 2 {
                                this.selected_date = d_date.clone();
                                // 日历展示全部科目，跳转后同样展示该日期的全部任务。
                                this.filter_plan_id = None;
                                this.active_tab = AppTab::TodayCheckIn;
                                cx.notify();
                            }
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _, window, cx| {
                            this.open_calendar_task_modal_action(&right_click_date, window, cx);
                        }),
                    )
                    .child(
                        // 顶部行：左侧[日期数字 + 今日 + 备忘图标]；右上角[学习时间 + 完成进度]
                        h_flex()
                            .items_center()
                            .justify_between()
                            .w_full()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_size(px(13.5))
                                            .font_weight(if is_today || is_sel {
                                                FontWeight::BLACK
                                            } else {
                                                FontWeight::BOLD
                                            })
                                            .text_color(if is_sel {
                                                theme.primary
                                            } else {
                                                theme.foreground
                                            })
                                            .child(day.day_num.to_string()),
                                    )
                                    .children(is_today.then(|| {
                                        div()
                                            .px_1()
                                            .py_0p5()
                                            .bg(theme.primary)
                                            .text_color(hsla(0.0, 0.0, 0.04, 1.0))
                                            .text_size(px(9.5))
                                            .font_weight(FontWeight::BOLD)
                                            .child("今日")
                                    }))
                                    .children(has_note.then(|| {
                                        div().text_size(px(11.)).child(format!("📝{note_count}"))
                                    })),
                            )
                            // 右上角统一放置完成进度和学习时间
                            .children(if day.total_tasks > 0 {
                                Some(
                                    h_flex()
                                        .items_center()
                                        .gap_1()
                                        .text_size(px(10.))
                                        .child(
                                            div().text_color(theme.muted_foreground).child(
                                                fmt_seconds(day.total_duration as f64, false),
                                            ),
                                        )
                                        .child(
                                            div()
                                                .px_1()
                                                .py_0p5()
                                                .bg(status_bg)
                                                .text_color(status_text_color)
                                                .font_weight(FontWeight::BOLD)
                                                .child(format!(
                                                    "{}/{}",
                                                    day.completed_tasks, day.total_tasks
                                                )),
                                        ),
                                )
                            } else if day.is_rest_day {
                                Some(
                                    h_flex().items_center().child(
                                        div()
                                            .text_size(px(10.))
                                            .text_color(theme.muted_foreground)
                                            .child("☕ 休息日"),
                                    ),
                                )
                            } else {
                                None
                            }),
                    )
                    .child(
                        // 课程和备注区：按格子容量展示多行，超出时显示省略提示。
                        v_flex().w_full().gap_0p5().children({
                            let mut items = Vec::new();
                            for (line, is_meta) in &summary_lines {
                                items.push(
                                    div()
                                        .w_full()
                                        .text_size(px(9.5))
                                        .text_color(if *is_meta {
                                            theme.primary
                                        } else {
                                            theme.muted_foreground
                                        })
                                        .whitespace_nowrap()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .child(line.clone())
                                        .into_any_element(),
                                );
                            }
                            items
                        }),
                    );

                row_cells.push(cell_el.into_any_element());
            }
            grid_rows.push(
                h_flex()
                    .w_full()
                    .gap_1p5()
                    .h(px(CALENDAR_CELL_HEIGHT))
                    .children(row_cells),
            );
        }

        let calendar_left = v_flex()
            .id("calendar-left-pane")
            .flex_1()
            .h_full()
            .overflow_y_scroll()
            .min_w_0()
            .gap_3()
            .pr_2()
            .child(month_nav_bar)
            .child(
                bcard(cx)
                    .p_3()
                    .gap_2()
                    .child(weekday_headers)
                    .child(v_flex().w_full().gap_1p5().children(grid_rows)),
            );

        // 2. 右侧：选中单日明细与备忘录侧边栏
        let total_day_dur: i64 = selected_tasks.iter().map(|t| t.task.portion).sum();
        let done_day_dur: i64 = selected_tasks
            .iter()
            .filter(|t| t.task.completed)
            .map(|t| t.task.portion)
            .sum();
        let total_day_tasks = selected_tasks.len();
        let done_day_tasks = selected_tasks.iter().filter(|t| t.task.completed).count();
        let calendar_is_future = selected_date > today_date_str();
        let calendar_advance_date = selected_date.clone();

        let day_detail_card = bcard(cx)
            .gap_2()
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(FontWeight::BOLD)
                            .child(format_date_with_weekday(&selected_date)),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .children((selected_date == today_date_str()).then(|| {
                                div()
                                    .px_2()
                                    .py_0p5()
                                    .bg(theme.primary)
                                    .text_color(hsla(0.0, 0.0, 0.04, 1.0))
                                    .text_size(px(11.))
                                    .font_weight(FontWeight::BOLD)
                                    .child("🔥 今日")
                            }))
                            .children(calendar_is_future.then(|| {
                                Button::new("calendar-advance-completed")
                                    .small()
                                    .primary()
                                    .label("提前已完成任务")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.advance_completed_tasks_action(
                                            &calendar_advance_date,
                                            None,
                                            window,
                                            cx,
                                        );
                                    }))
                            })),
                    ),
            )
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(12.5))
                    .text_color(theme.muted_foreground)
                    .child(format!("任务：{done_day_tasks} / {total_day_tasks} 项"))
                    .child(format!(
                        "已学 {} / 共 {}",
                        fmt_seconds(done_day_dur as f64, true),
                        fmt_seconds(total_day_dur as f64, true)
                    )),
            );

        // 备注新增区
        let note_edit_card = bcard(cx)
            .gap_2p5()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Icon::empty()
                            .path("icons/square-pen.svg")
                            .size_4()
                            .text_color(theme.primary),
                    )
                    .child(
                        div()
                            .text_size(px(13.5))
                            .font_weight(FontWeight::BOLD)
                            .child("添加当日学习备注"),
                    ),
            )
            .child(div().w_full().child(Input::new(&self.calendar_note_input)))
            .child(
                h_flex().items_center().justify_end().gap_2().child(
                    Button::new("cal-save-note-btn")
                        .small()
                        .primary()
                        .label("➕ 添加备注")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.save_calendar_note_action(window, cx);
                        })),
                ),
            );

        // 已有备注单独列出，支持同一天多条并可逐条删除。
        let notes_list_card = bcard(cx)
            .gap_2p5()
            .child(
                h_flex().items_center().justify_between().child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Icon::empty()
                                .path("icons/square-pen.svg")
                                .size_4()
                                .text_color(theme.primary),
                        )
                        .child(
                            div()
                                .text_size(px(13.5))
                                .font_weight(FontWeight::BOLD)
                                .child(format!("当日备注（{} 条）", selected_notes.len())),
                        ),
                ),
            )
            .child(if selected_notes.is_empty() {
                div()
                    .py_3()
                    .text_size(px(12.))
                    .text_color(theme.muted_foreground)
                    .child("暂未添加备注")
                    .into_any_element()
            } else {
                let mut note_items = Vec::new();
                for (index, note) in selected_notes.iter().enumerate() {
                    let note_id = note.id.clone();
                    let note_date = selected_date.clone();
                    note_items.push(
                        div()
                            .id(("calendar-note", index))
                            .w_full()
                            .p_2()
                            .gap_2()
                            .flex()
                            .items_start()
                            .justify_between()
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.background.opacity(0.3))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(12.))
                                    .child(note.content.clone()),
                            )
                            .child(
                                Button::new(("delete-calendar-note", index))
                                    .small()
                                    .ghost()
                                    .label("删除")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.delete_calendar_note_action(
                                            &note_date, &note_id, window, cx,
                                        );
                                    })),
                            )
                            .into_any_element(),
                    );
                }
                v_flex()
                    .w_full()
                    .gap_2()
                    .children(note_items)
                    .into_any_element()
            });

        // 当日具体任务列表
        let tasks_list_card = bcard(cx)
            .gap_2p5()
            .child(
                h_flex().items_center().justify_between().child(
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
                            div()
                                .text_size(px(13.5))
                                .font_weight(FontWeight::BOLD)
                                .child(format!("当日学习项目 (共 {total_day_tasks} 项)")),
                        ),
                ),
            )
            .child(if selected_tasks.is_empty() {
                v_flex()
                    .w_full()
                    .py_8()
                    .items_center()
                    .justify_center()
                    .gap_1p5()
                    .text_color(theme.muted_foreground)
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .child("✨ 当日无学习任务安排"),
                    )
                    .child(div().text_size(px(11.5)).child("可自由复习、预习或休息"))
                    .into_any_element()
            } else {
                let mut task_items = Vec::new();
                for (i, t_item) in selected_tasks.iter().enumerate() {
                    let pid = t_item.plan_id.clone();
                    let tid = t_item.task.id.clone();
                    let is_done = t_item.task.completed;
                    let st = t_item.source_type.clone();
                    let su = t_item.source_url.clone();
                    let has_source_url = !su.trim().is_empty();
                    let is_calendar_task = st == "calendar";
                    let vno = t_item.task.vid_no;
                    let edit_pid = pid.clone();
                    let edit_tid = tid.clone();
                    let delete_pid = pid.clone();
                    let delete_tid = tid.clone();
                    let edit_title = t_item.task.title.clone();
                    let edit_portion = t_item.task.portion;
                    let edit_date = selected_date.clone();
                    let move_seed = CalendarTaskEditSeed {
                        plan_id: pid.clone(),
                        task_id: tid.clone(),
                        title: edit_title.clone(),
                        date: selected_date.clone(),
                        portion: edit_portion,
                    };

                    task_items.push(
                        div()
                            .id(("cal-task", i))
                            .w_full()
                            .p_2()
                            .border_1()
                            .border_color(theme.border)
                            .bg(if is_done {
                                theme.primary.opacity(0.08)
                            } else {
                                theme.background.opacity(0.3)
                            })
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .text_size(px(12.5))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(if is_done {
                                                theme.muted_foreground
                                            } else {
                                                theme.foreground
                                            })
                                            .whitespace_nowrap()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .child(format!(
                                                "《{}》 P{}: {}",
                                                t_item.plan_title,
                                                t_item.task.vid_no,
                                                t_item.task.title
                                            )),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.5))
                                            .text_color(theme.muted_foreground)
                                            .child(fmt_seconds(t_item.task.portion as f64, true)),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_1p5()
                                    .children(has_source_url.then(|| {
                                        Button::new(("cal-play", i))
                                            .small()
                                            .ghost()
                                            .label("🔗 直达")
                                            .on_click(move |_, _, _| {
                                                open_video_link(&st, &su, vno);
                                            })
                                    }))
                                    .child(
                                        Button::new(("move-calendar-task", i))
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
                                    .children(is_calendar_task.then(|| {
                                        Button::new(("edit-calendar-task", i))
                                            .small()
                                            .ghost()
                                            .label("编辑")
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.open_calendar_task_edit_modal_action(
                                                    CalendarTaskEditSeed {
                                                        plan_id: edit_pid.clone(),
                                                        task_id: edit_tid.clone(),
                                                        title: edit_title.clone(),
                                                        date: edit_date.clone(),
                                                        portion: edit_portion,
                                                    },
                                                    window,
                                                    cx,
                                                );
                                            }))
                                    }))
                                    .children(is_calendar_task.then(|| {
                                        Button::new(("delete-calendar-task", i))
                                            .small()
                                            .ghost()
                                            .label("删除")
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.delete_calendar_task_action(
                                                    &delete_pid,
                                                    &delete_tid,
                                                    window,
                                                    cx,
                                                );
                                            }))
                                    }))
                                    .child(
                                        Button::new(("cal-chk", i))
                                            .small()
                                            .primary()
                                            .label(if is_done {
                                                "已完成 ✅"
                                            } else {
                                                "打卡 ⬜"
                                            })
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.toggle_task_checkin_action(
                                                    &pid, &tid, window, cx,
                                                );
                                            })),
                                    ),
                            )
                            .into_any_element(),
                    );
                }
                v_flex()
                    .w_full()
                    .gap_2()
                    .children(task_items)
                    .into_any_element()
            });

        let calendar_right = v_flex()
            .id("calendar-right-pane")
            .w(px(400.))
            .min_w(px(400.))
            .h_full()
            .overflow_y_scroll()
            .gap_3()
            .pr_1()
            .child(day_detail_card)
            .child(note_edit_card)
            .child(notes_list_card)
            .child(tasks_list_card);

        h_flex()
            .id("calendar-split-view")
            .size_full()
            .px_8()
            .py_6()
            .gap_5()
            .child(calendar_left)
            .child(calendar_right)
            .into_any_element()
    }
}
