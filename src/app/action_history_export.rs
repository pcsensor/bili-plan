use super::*;

impl PlannerApp {
    /// 点击历史条目：切到对应来源并回填链接输入框（不自动发起请求）。
    pub(super) fn use_history_entry(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.config.history.get(index) else {
            return;
        };
        let entry = entry.clone();
        let source = source_mode_of(&entry.source);
        if source != self.source {
            self.switch_source(source, window, cx);
        }
        self.link_input.update(cx, |state, cx| {
            state.set_value(entry.input.clone(), window, cx)
        });
        cx.notify();
    }

    /// 删除单条历史。
    pub(super) fn remove_history_entry(&mut self, index: usize, cx: &mut Context<Self>) {
        remove_history(&mut self.config, index);
        save_config(&self.config);
        cx.notify();
    }

    /// 一键清空历史。
    pub(super) fn clear_all_history(&mut self, cx: &mut Context<Self>) {
        clear_history(&mut self.config);
        save_config(&self.config);
        cx.notify();
    }

    /// 历史记录卡片（无历史时不渲染）。
    pub(super) fn render_history_card(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if self.config.history.is_empty() {
            return None;
        }
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;

        // 条带右侧的「清空」动作：直角描边小块，野兽风按钮语言。
        let clear = div()
            .id("history-clear")
            .px_2()
            .py_0p5()
            .text_size(px(11.))
            .font_weight(FontWeight::BOLD)
            .text_color(hsla(0.0, 0.0, 0.04, 1.0))
            .border_2()
            .border_color(hsla(0.0, 0.0, 0.04, 1.0))
            .hover(|s| s.bg(hsla(0.0, 0.0, 0.04, 0.12)))
            .child("清空")
            .on_click(cx.listener(|this, _, _, cx| this.clear_all_history(cx)));

        let rows: Vec<gpui::AnyElement> = self
            .config
            .history
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let (icon_path, source_label) = source_badge(&h.source);
                let display = if h.title.is_empty() {
                    h.input.clone()
                } else {
                    h.title.clone()
                };
                // 行内不挂 on_click：点击区（图标+文本）与删除按钮做兄弟节点，
                // 避免 gpui hitbox 不遮挡导致父子双触发。
                h_flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .child(
                        div()
                            .id(("history-item", i))
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_1()
                            .py_1()
                            .rounded_none()
                            .hover(|s| s.bg(theme.list_hover))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.use_history_entry(i, window, cx);
                            }))
                            .child(
                                Icon::empty()
                                    .path(icon_path.to_string())
                                    .size_3p5()
                                    .flex_shrink_0()
                                    .text_color(muted),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(10.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(muted)
                                    .child(source_label),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(12.5))
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .child(display),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(10.5))
                                    .text_color(muted)
                                    .child(ago_label(h.at)),
                            ),
                    )
                    .child(
                        div()
                            .id(("history-del", i))
                            .flex_shrink_0()
                            .p_1()
                            .rounded_none()
                            .hover(|s| s.bg(theme.danger.opacity(0.15)))
                            .child(
                                Icon::empty()
                                    .path("icons/close.svg")
                                    .size_3p5()
                                    .text_color(muted),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remove_history_entry(i, cx);
                            })),
                    )
                    .into_any_element()
            })
            .collect();

        Some(
            entrance(
                "anim-history",
                0.06,
                bcard(cx)
                    .child(
                        section_band("历史记录", "icons/clock.svg", cx)
                            .child(div().flex_1())
                            .child(clear),
                    )
                    .children(rows),
            )
            .into_any_element(),
        )
    }

    /// 点击「导出计划文本」：文件对话框与写盘在后台线程，完成后通知。
    pub(super) fn start_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let payload = match &self.phase {
            Phase::Ready(rd) => export_payload(rd, self.mode),
            _ => None,
        };
        let Some((text, suggested)) = payload else {
            window.push_notification(Notification::warning("请先生成观看计划。"), cx);
            return;
        };

        cx.spawn_in(window, async move |this, cx| {
            let saved = cx
                .background_executor()
                .spawn(async move {
                    let picked = pollster::block_on(
                        rfd::AsyncFileDialog::new()
                            .set_file_name(&suggested)
                            .save_file(),
                    );
                    match picked {
                        Some(handle) => {
                            let path: PathBuf = handle.path().to_path_buf();
                            match std::fs::write(&path, &text) {
                                Ok(()) => Ok(path),
                                Err(e) => Err(format!("无法保存计划文件：{e}")),
                            }
                        }
                        None => Err(String::new()), // 用户取消，不打扰
                    }
                })
                .await;

            this.update_in(cx, |_this, window, cx| match saved {
                Ok(path) => {
                    window.push_notification(
                        Notification::success(format!("已导出：{}", path.display())),
                        cx,
                    );
                }
                Err(e) if !e.is_empty() => {
                    window.push_notification(Notification::error(e), cx);
                }
                _ => {}
            })
            .ok();
        })
        .detach();
    }
}
