use super::*;

impl PlannerApp {
    /// 右栏：每日计划面板（独立滚动，高度撑满窗口）。
    pub(super) fn render_plan_panel(&self, rd: &ReadyState, cx: &mut Context<Self>) -> gpui::Div {
        let theme = cx.theme().clone();
        let table_card = |content: gpui::AnyElement| {
            bcard(cx)
                .flex_1()
                .min_h_0()
                .min_w_0()
                .child(section_band("05 · 每日计划", "icons/table.svg", cx))
                .child(content)
        };
        match (&self.plan_table, &rd.plan) {
            (Some(table), _) => table_card(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .rounded_none()
                    .border_2()
                    .border_color(theme.foreground)
                    .overflow_hidden()
                    .child(Table::new(table).stripe(true))
                    .into_any_element(),
            ),
            (None, Some(_)) => table_card(
                Label::new("计划已失效，请点击「生成观看计划」重新生成。")
                    .text_size(px(13.))
                    .text_color(theme.muted_foreground)
                    .into_any_element(),
            ),
            (None, None) => table_card(
                v_flex()
                    .gap_2()
                    .items_center()
                    .py_8()
                    .child(
                        Icon::empty()
                            .path("icons/calendar-days.svg")
                            .size_8()
                            .text_color(theme.muted_foreground),
                    )
                    .child(
                        Label::new("填写目标天数后点击「生成观看计划」。")
                            .text_size(px(13.))
                            .text_color(theme.muted_foreground),
                    )
                    .into_any_element(),
            ),
        }
    }

    pub(super) fn render_loading(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        bcard(cx)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(gpui_component::spinner::Spinner::new().small())
                    .child(Label::new(self.fetch_progress.clone()).text_size(px(13.))),
            )
            .child(
                Label::new(format!("已用时 {} 秒", self.fetch_elapsed_secs))
                    .text_size(px(12.))
                    .text_color(cx.theme().muted_foreground),
            )
            .when(self.source == SourceMode::FnOs, |card| {
                card.child(
                    Label::new("网盘视频首次读取时长可能需要几分钟，完成后会自动生成计划。")
                        .text_size(px(12.))
                        .text_color(cx.theme().muted_foreground),
                )
            })
    }

    pub(super) fn meta_line(
        label: &str,
        value: &str,
        theme: &gpui_component::ThemeColor,
    ) -> impl IntoElement {
        h_flex()
            .gap_2()
            .child(
                Label::new(format!("{label}："))
                    .text_size(px(13.))
                    .text_color(theme.muted_foreground),
            )
            .child(Label::new(value.to_string()).text_size(px(13.)))
    }

    /// 渲染科目选择的单选项（野兽风视觉：硬边框、高亮背景、单选指示器与右侧时长徽标）。
    pub(super) fn render_subject_item(
        id: impl Into<gpui::ElementId>,
        selected: bool,
        title: String,
        badge: Option<String>,
        theme: &gpui_component::ThemeColor,
        dark: bool,
        on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let active_bg = if dark {
            theme.primary.opacity(0.18)
        } else {
            theme.primary.opacity(0.12)
        };
        let hover_bg = if dark {
            theme.accent.opacity(0.10)
        } else {
            theme.accent.opacity(0.08)
        };

        h_flex()
            .id(id)
            .w_full()
            .min_w_0()
            .items_center()
            .gap_3()
            .px_3()
            .py_2()
            .rounded_none()
            .border_2()
            .border_color(if selected {
                theme.primary
            } else {
                theme.border
            })
            .bg(if selected {
                active_bg
            } else {
                theme.background.opacity(0.35)
            })
            .cursor_pointer()
            .hover(move |s| if !selected { s.bg(hover_bg) } else { s })
            .on_click(move |event, window, cx| on_click(event, window, cx))
            .child(
                // 单选指示器圆圈
                div()
                    .flex_shrink_0()
                    .size(px(16.))
                    .rounded_full()
                    .border_2()
                    .border_color(if selected {
                        theme.primary
                    } else {
                        theme.muted_foreground
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(selected, |d| {
                        d.child(div().size(px(8.)).rounded_full().bg(theme.primary))
                    }),
            )
            .child(
                // 科目标题
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(13.))
                    .font_weight(if selected {
                        FontWeight::BOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .text_color(theme.foreground)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(title),
            )
            .children(badge.map(|b| {
                div()
                    .flex_shrink_0()
                    .text_size(px(11.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.muted_foreground)
                    .px_2()
                    .py_0p5()
                    .bg(theme.muted.opacity(0.5))
                    .rounded_none()
                    .border_1()
                    .border_color(theme.border.opacity(0.4))
                    .child(b)
            }))
    }
    /// 渲染计划生成器标签页（左侧配置 + 右侧计划表）。
    pub(super) fn render_plan_generator_view(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme().clone();

        // 左栏：hero + 表单 + （就绪后）信息/科目/操作/打卡，独立滚动。
        let mut left = v_flex()
            .id("content-scroll")
            .h_full()
            .flex_1()
            .min_w_0()
            .overflow_y_scroll()
            .px_6()
            .py_5()
            .gap_5()
            .child(entrance("anim-hero", 0., self.render_hero(cx)))
            .child(entrance("anim-form", 0.12, self.render_form_card(cx)));

        // 加载卡片紧跟表单：点击「获取视频信息」后立刻可见进度与用时，
        // 不会被历史记录卡片挤到视口之外（网盘视频探测可能持续数分钟）。
        if matches!(self.phase, Phase::Loading) {
            left = left.child(self.render_loading(cx));
        }

        left = left.children(self.render_history_card(cx));

        if let Some(err) = &self.last_error {
            left = left.child(
                Alert::error("fetch-error", err.clone())
                    .title("获取失败")
                    .flex_shrink_0(),
            );
        }

        if let Phase::Ready(rd) = &self.phase {
            left = left.children(self.render_ready_left(rd, cx));
        }

        // 右栏：就绪后展开的计划面板，独立滚动、高度撑满窗口。
        if let Phase::Ready(rd) = &self.phase {
            h_resizable("main-splitter")
                .child(
                    resizable_panel()
                        .size_range(px(360.)..px(1800.))
                        .child(left),
                )
                .child(
                    resizable_panel()
                        .size(PLAN_PANEL_WIDTH)
                        .size_range(px(380.)..px(2400.))
                        .child(
                            v_flex()
                                .size_full()
                                .min_h_0()
                                .min_w_0()
                                .border_l_2()
                                .border_color(theme.foreground)
                                .bg(theme.background.opacity(0.72))
                                .child(v_flex().flex_1().min_h_0().min_w_0().px_5().py_4().child(
                                    entrance("anim-plan", 0.15, self.render_plan_panel(rd, cx)),
                                )),
                        ),
                )
                .into_any_element()
        } else {
            h_flex().flex_1().min_h_0().child(left).into_any_element()
        }
    }
}
