use super::*;

impl PlannerApp {
    /// 渲染机器人绑定弹窗（Neo-Brutalist 弹窗 + 醒目大字验证码，支持飞书与 Telegram）。
    pub(super) fn render_bind_modal(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme().clone();
        let code_str = self.cloud_bind_code.as_deref().unwrap_or("------");

        div()
            .absolute()
            .inset_0()
            .bg(hsla(0.0, 0.0, 0.0, 0.6))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(480.))
                    .bg(theme.background)
                    .border_2()
                    .border_color(theme.foreground)
                    .shadow_lg()
                    .p_6()
                    .gap_4()
                    .flex()
                    .flex_col()
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(Icon::empty().path("icons/tv.svg").size_5().text_color(theme.primary))
                                    .child(div().text_size(px(16.)).font_weight(FontWeight::BOLD).child("📱 绑定飞书 / Telegram 学习助手")),
                            )
                            .child(
                                div()
                                    .id("close-bind-modal")
                                    .cursor_pointer()
                                    .p_1()
                                    .child(Icon::empty().path("icons/square.svg").size_4().text_color(theme.muted_foreground))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cloud_bind_modal_open = false;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(theme.muted_foreground)
                            .child("在飞书机器人 或 Telegram 机器人聊天窗口中，发送以下指令完成绑定："),
                    )
                    .child(
                        div()
                            .py_3()
                            .px_4()
                            .bg(theme.primary.opacity(0.15))
                            .border_2()
                            .border_color(theme.primary)
                            .items_center()
                            .justify_center()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(theme.muted_foreground)
                                    .child("复制并在 飞书/TG 中发送："),
                            )
                            .child(
                                div()
                                    .text_size(px(22.))
                                    .font_weight(FontWeight::BLACK)
                                    .text_color(theme.foreground)
                                    .child(format!("/bind {code_str}")),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child("• 验证码有效期 10 分钟\n• 绑定后两端均支持每日 08:30 计划早报与 21:30 督促提醒\n• 支持在消息卡片中直接点击按钮完成打卡并双向同步"),
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .justify_end()
                            .child(
                                Button::new("cancel-bind")
                                    .label("稍后再说")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cloud_bind_modal_open = false;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("confirm-bind")
                                    .primary()
                                    .label("✅ 我已发送，完成绑定")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.check_bind_status_action(window, cx);
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// 渲染云端服务器地址设置与健康检查面板。
    pub(super) fn render_cloud_settings_modal(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme().clone();
        let test_result = self.cloud_server_test_result.clone();

        div()
            .absolute()
            .inset_0()
            .bg(hsla(0.0, 0.0, 0.0, 0.6))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(580.))
                    .bg(theme.background)
                    .border_2()
                    .border_color(theme.foreground)
                    .shadow_lg()
                    .p_6()
                    .gap_4()
                    .flex()
                    .flex_col()
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
                                            .path("icons/server.svg")
                                            .size_5()
                                            .text_color(theme.primary),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(16.))
                                            .font_weight(FontWeight::BOLD)
                                            .child("云端服务器设置"),
                                    ),
                            )
                            .child(
                                div()
                                    .id("close-cloud-settings-modal")
                                    .cursor_pointer()
                                    .p_1()
                                    .child(
                                        Icon::empty()
                                            .path("icons/square.svg")
                                            .size_4()
                                            .text_color(theme.muted_foreground),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cloud_settings_modal_open = false;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .child(Self::field_label(
                                "云端服务器地址",
                                "服务根地址，例如 https://plan.example.com",
                                cx,
                            ))
                            .child(Input::new(&self.cloud_server_input))
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(theme.muted_foreground)
                                    .child("测试连接会请求 <服务器地址>/api/health。切换到另一台服务器后需重新绑定机器人。"),
                            ),
                    )
                    .children(test_result.map(|(message, success)| {
                        div()
                            .w_full()
                            .p_3()
                            .border_1()
                            .border_color(if success { theme.success } else { theme.danger })
                            .bg(if success {
                                theme.success.opacity(0.1)
                            } else {
                                theme.danger.opacity(0.1)
                            })
                            .text_size(px(12.5))
                            .text_color(if success { theme.success } else { theme.danger })
                            .child(if success {
                                format!("✅ {message}")
                            } else {
                                format!("⚠️ {message}")
                            })
                    }))
                    .child(
                        h_flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("cancel-cloud-settings")
                                    .label("取消")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cloud_settings_modal_open = false;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("test-cloud-server")
                                    .ghost()
                                    .disabled(self.cloud_testing)
                                    .label(if self.cloud_testing {
                                        "测试中…"
                                    } else {
                                        "测试连接"
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.test_cloud_server_action(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("save-cloud-server")
                                    .primary()
                                    .label("保存地址")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.save_cloud_server_action(window, cx);
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// 渲染云端同步结果弹窗。
    pub(super) fn render_sync_result_modal(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme().clone();
        let Some((title, is_success, lines)) = &self.cloud_sync_modal_data else {
            return div().into_any_element();
        };

        let title_icon = if *is_success {
            "icons/square-check.svg"
        } else {
            "icons/refresh-cw.svg"
        };
        let icon_color = if *is_success {
            theme.primary
        } else {
            theme.danger
        };

        let content_items: Vec<gpui::AnyElement> = lines
            .iter()
            .map(|line| {
                div()
                    .text_size(px(13.))
                    .text_color(theme.foreground)
                    .child(line.clone())
                    .into_any_element()
            })
            .collect();

        div()
            .absolute()
            .inset_0()
            .bg(hsla(0.0, 0.0, 0.0, 0.6))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(480.))
                    .bg(theme.background)
                    .border_2()
                    .border_color(theme.foreground)
                    .shadow_lg()
                    .p_6()
                    .gap_4()
                    .flex()
                    .flex_col()
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
                                            .path(title_icon)
                                            .size_5()
                                            .text_color(icon_color),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(16.))
                                            .font_weight(FontWeight::BOLD)
                                            .child(title.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .id("close-sync-modal")
                                    .cursor_pointer()
                                    .p_1()
                                    .child(
                                        Icon::empty()
                                            .path("icons/square.svg")
                                            .size_4()
                                            .text_color(theme.muted_foreground),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cloud_sync_modal_open = false;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        v_flex()
                            .w_full()
                            .gap_2()
                            .p_3()
                            .bg(theme.primary.opacity(0.06))
                            .border_1()
                            .border_color(theme.border)
                            .children(content_items),
                    )
                    .child(
                        h_flex().justify_end().child(
                            Button::new("sync-modal-ok-btn")
                                .primary()
                                .label("好的")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.cloud_sync_modal_open = false;
                                    cx.notify();
                                })),
                        ),
                    ),
            )
            .into_any_element()
    }
}
