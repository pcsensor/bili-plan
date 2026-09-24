use super::*;

impl PlannerApp {
    pub(super) fn render_title_bar(
        &self,
        dark: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let (icon, label) = if dark {
            (IconName::Sun, "切换亮色")
        } else {
            (IconName::Moon, "切换暗色")
        };
        let toggle = Button::new("theme-toggle")
            .ghost()
            .small()
            .icon(icon)
            .label(label)
            .on_click(|_, window, cx| {
                let mode = if cx.theme().is_dark() {
                    ThemeMode::Light
                } else {
                    ThemeMode::Dark
                };
                Theme::change(mode, Some(window), cx);
            });

        let today_tasks = get_tasks_for_date(&self.config.plans, &today_date_str());
        let uncompleted_today = today_tasks.iter().filter(|t| !t.task.completed).count();
        let active_plans_count = self
            .config
            .plans
            .iter()
            .filter(|p| p.status == PlanStatus::Active)
            .count();

        let tab_btn = |tab: AppTab,
                       label_text: &'static str,
                       icon_path: &'static str,
                       badge: Option<usize>,
                       cx: &mut Context<Self>| {
            let active = self.active_tab == tab;
            let theme = cx.theme();
            let dark = theme.is_dark();
            let active_bg = if dark {
                theme.primary
            } else {
                hsla(0.135, 1.0, 0.5, 1.0)
            };
            let text_color = if active {
                hsla(0.0, 0.0, 0.04, 1.0)
            } else {
                theme.foreground
            };

            h_flex()
                .id(("tab-btn", tab as usize))
                .items_center()
                .gap_1p5()
                .px_3()
                .py_1()
                .cursor_pointer()
                .rounded_none()
                .border_2()
                .border_color(if active {
                    theme.foreground
                } else {
                    theme.border.opacity(0.5)
                })
                .bg(if active {
                    active_bg
                } else {
                    theme.background.opacity(0.3)
                })
                .hover(move |s| {
                    if !active {
                        s.bg(theme.accent.opacity(0.15))
                    } else {
                        s
                    }
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.active_tab = tab;
                    cx.notify();
                }))
                .child(
                    Icon::empty()
                        .path(icon_path.to_string())
                        .size_3p5()
                        .text_color(text_color),
                )
                .child(
                    div()
                        .text_size(px(12.5))
                        .font_weight(if active {
                            FontWeight::BOLD
                        } else {
                            FontWeight::MEDIUM
                        })
                        .text_color(text_color)
                        .child(label_text),
                )
                .children(badge.and_then(|count| {
                    if count > 0 {
                        Some(
                            div()
                                .px_1p5()
                                .py_0p5()
                                .text_size(px(10.))
                                .font_weight(FontWeight::BOLD)
                                .rounded_full()
                                .bg(if active {
                                    hsla(0.0, 0.0, 0.04, 0.8)
                                } else {
                                    theme.danger
                                })
                                .text_color(hsla(0.0, 0.0, 1.0, 1.0))
                                .child(count.to_string()),
                        )
                    } else {
                        None
                    }
                }))
        };

        let tabs = h_flex()
            .gap_2()
            .items_center()
            .child(tab_btn(
                AppTab::TodayCheckIn,
                "今日打卡",
                "icons/square-check.svg",
                (uncompleted_today > 0).then_some(uncompleted_today),
                cx,
            ))
            .child(tab_btn(
                AppTab::Calendar,
                "学习日历",
                "icons/calendar-days.svg",
                None,
                cx,
            ))
            .child(tab_btn(
                AppTab::PlanGenerator,
                "计划生成器",
                "icons/film.svg",
                None,
                cx,
            ))
            .child(tab_btn(
                AppTab::MyPlans,
                "我的计划库",
                "icons/table.svg",
                Some(active_plans_count),
                cx,
            ));

        let auto_sync_btn = Button::new("auto-sync-btn")
            .small()
            .ghost()
            .label(if self.config.auto_sync {
                "⚡ 自动同步: 开"
            } else {
                "⚡ 自动同步: 关"
            })
            .on_click(cx.listener(|this, _, window, cx| {
                this.config.auto_sync = !this.config.auto_sync;
                save_config(&this.config);
                let state_str = if this.config.auto_sync {
                    "开启"
                } else {
                    "关闭"
                };
                window.push_notification(
                    Notification::info(format!("已{}检测到修改自动同步功能", state_str)),
                    cx,
                );
                if this.config.auto_sync {
                    this.trigger_auto_sync(window, cx);
                }
                cx.notify();
            }));

        let bind_btn = if self.config.feishu_bound || self.config.telegram_bound {
            let mut labels = Vec::new();
            if self.config.feishu_bound {
                labels.push(format!(
                    "飞书:{}",
                    self.config.feishu_user_name.as_deref().unwrap_or("已连")
                ));
            }
            if self.config.telegram_bound {
                labels.push(format!(
                    "TG:{}",
                    self.config.telegram_user_name.as_deref().unwrap_or("已连")
                ));
            }
            let label = format!("📱 {}", labels.join(" | "));
            Button::new("bot-status-btn")
                .small()
                .ghost()
                .label(label)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.request_bind_code_action(window, cx);
                }))
        } else {
            Button::new("bot-bind-btn")
                .small()
                .primary()
                .label("📱 绑定机器人")
                .on_click(cx.listener(|this, _, window, cx| {
                    this.request_bind_code_action(window, cx);
                }))
        };

        let sync_btn = Button::new("sync-btn")
            .small()
            .ghost()
            .icon(Icon::empty().path("icons/refresh-cw.svg"))
            .label(if self.cloud_syncing {
                "同步中…"
            } else {
                "云端同步"
            })
            .disabled(self.cloud_syncing)
            .on_click(cx.listener(|this, _, window, cx| {
                this.sync_cloud_action(window, cx);
            }));

        let cloud_settings_btn = Button::new("cloud-settings-btn")
            .small()
            .ghost()
            .icon(Icon::empty().path("icons/server.svg"))
            .label("云端设置")
            .on_click(cx.listener(|this, _, window, cx| {
                this.open_cloud_settings_action(window, cx);
            }));

        let right_actions = h_flex()
            .gap_2()
            .items_center()
            .child(auto_sync_btn)
            .child(bind_btn)
            .child(cloud_settings_btn)
            .child(sync_btn)
            .child(toggle);

        TitleBar::new()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Icon::empty()
                            .path("icons/play.svg")
                            .size_4()
                            .text_color(cx.theme().primary),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::BOLD)
                            .child("BILI-PLANNER"),
                    ),
            )
            .child(div().flex_1().flex().justify_center().child(tabs))
            .child(right_actions)
    }

    /// Hero 区：超大标题 + 关键词高亮块（野兽风海报语言）。
    pub(super) fn render_hero(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = cx.theme();
        let dark = theme.is_dark();
        let ink = theme.foreground;

        // 高亮底块：亮=明黄 / 暗=荧光黄，文字始终用墨黑。
        let hl_bg = if dark {
            theme.primary
        } else {
            hsla(0.135, 1.0, 0.5, 1.0)
        };
        let hl = |text: &str| {
            div()
                .px_2()
                .py_0p5()
                .bg(hl_bg)
                .text_color(hsla(0.0, 0.0, 0.04, 1.0))
                .child(text.to_string())
        };

        v_flex()
            .gap_2()
            .pt_2()
            .pb_1()
            .child(
                h_flex()
                    .gap_3()
                    .items_end()
                    .flex_wrap()
                    .child(
                        div()
                            .text_size(px(40.))
                            .font_weight(FontWeight::BLACK)
                            .text_color(ink)
                            .child("合集观看"),
                    )
                    .child(
                        div()
                            .text_size(px(40.))
                            .font_weight(FontWeight::BLACK)
                            .child(hl("计划")),
                    )
                    .child(
                        div()
                            .text_size(px(40.))
                            .font_weight(FontWeight::BLACK)
                            .text_color(ink)
                            .child("生成器"),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(hl("BILIBILI"))
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.muted_foreground)
                            .child("×"),
                    )
                    .child(hl("JELLYFIN"))
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(theme.muted_foreground)
                            .child("— 粘贴链接，按目标天数自动排期"),
                    ),
            )
    }

    /// 来源切换用的成组按钮（gpui-component 无 segmented 控件）。
    pub(super) fn source_toggle(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        h_flex()
            .gap_1()
            .child(
                Button::new("source-bilibili")
                    .small()
                    .icon(Icon::empty().path("icons/tv.svg").size_4())
                    .label("B 站")
                    .when(self.source == SourceMode::Bilibili, |b| b.primary())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.switch_source(SourceMode::Bilibili, window, cx);
                    })),
            )
            .child(
                Button::new("source-jellyfin")
                    .small()
                    .icon(Icon::empty().path("icons/film.svg").size_4())
                    .label("Jellyfin")
                    .when(self.source == SourceMode::Jellyfin, |b| b.primary())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.switch_source(SourceMode::Jellyfin, window, cx);
                    })),
            )
            .child(
                Button::new("source-fnos")
                    .small()
                    .icon(Icon::empty().path("icons/server.svg").size_4())
                    .label("飞牛影视")
                    .when(self.source == SourceMode::FnOs, |b| b.primary())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.switch_source(SourceMode::FnOs, window, cx);
                    })),
            )
    }

    /// 计划模式切换用的成组按钮。
    pub(super) fn mode_toggle(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        h_flex()
            .gap_1()
            .child(
                Button::new("mode-split")
                    .small()
                    .icon(Icon::empty().path("icons/scissors.svg").size_4())
                    .label("split 精确切分")
                    .when(self.mode == Mode::Split, |b| b.primary())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.switch_mode(Mode::Split, window, cx);
                    })),
            )
            .child(
                Button::new("mode-whole")
                    .small()
                    .icon(Icon::empty().path("icons/square-check.svg").size_4())
                    .label("whole 完整不拆")
                    .when(self.mode == Mode::Whole, |b| b.primary())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.switch_mode(Mode::Whole, window, cx);
                    })),
            )
    }

    pub(super) fn field_label(label: &str, help: &str, cx: &App) -> impl IntoElement + use<> {
        v_flex()
            .gap_1()
            .child(
                Label::new(label.to_string())
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD),
            )
            .child(
                Label::new(help.to_string())
                    .text_size(px(12.))
                    .text_color(cx.theme().muted_foreground),
            )
    }

    pub(super) fn render_form_card(&self, cx: &mut Context<Self>) -> gpui::Div {
        let loading = matches!(self.phase, Phase::Loading);
        let theme = cx.theme().clone();

        // 链接输入的提示随来源切换，避免误粘贴场景提示错位。
        let (link_label, link_help) = match self.source {
            SourceMode::Bilibili => (
                "链接 / BV 号 / 合集 sid",
                "支持 https://www.bilibili.com/video/BVxxxx、BV 号或合集 sid=xxxx 链接",
            ),
            SourceMode::Jellyfin => (
                "Jellyfin 链接 / item ID",
                "粘贴 Jellyfin 网页详情页链接（取 ?id= 后部分）或直接 item ID；首次填写服务器/Token 后会自动保存到本机",
            ),
            SourceMode::FnOs => (
                "飞牛影视链接 / 条目 guid",
                "粘贴飞牛影视网页链接（自动取 ?guid= 参数或路径末段）或直接输入 guid；首次填写服务器/账号后会自动保存到本机",
            ),
        };

        let hint = if loading {
            String::new()
        } else {
            match self.source {
                SourceMode::Bilibili => {
                    "提示：B 站接口可能触发风控，失败时可添加 Cookie 重试".to_string()
                }
                SourceMode::Jellyfin => {
                    "提示：若拉取失败，请确认 Token 有效且 Jellyfin 可访问".to_string()
                }
                SourceMode::FnOs => {
                    "提示：链接需指向影视库/合集/季（有下级剧集），服务器需可从本机访问".to_string()
                }
            }
        };

        bcard(cx)
            .child(section_band("01 · 数据来源", "icons/link.svg", cx))
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(Label::new("来源").text_size(px(13.)))
                    .child(self.source_toggle(cx)),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(Self::field_label(link_label, link_help, cx))
                    .child(Input::new(&self.link_input).cleanable(true)),
            )
            .child(
                h_flex()
                    .gap_4()
                    .items_end()
                    .child(
                        v_flex()
                            .gap_1()
                            .w(px(140.))
                            .child(Self::field_label("目标观看天数", "正整数", cx))
                            .child(Input::new(&self.days_input)),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                Label::new("计划模式")
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::SEMIBOLD),
                            )
                            .child(self.mode_toggle(cx)),
                    ),
            )
            .children(match self.source {
                SourceMode::Bilibili => vec![v_flex()
                    .gap_1()
                    .child(Self::field_label(
                        "Cookie（可选，风控时使用）",
                        "例如 SESSDATA=xxx；留空则匿名请求",
                        cx,
                    ))
                    .child(Input::new(&self.cookie_input).cleanable(true))
                    .into_any_element()],
                SourceMode::Jellyfin => vec![
                    v_flex()
                        .gap_1()
                        .child(Self::field_label(
                            "Jellyfin 服务器地址",
                            "形如 https://media.example.com:8096，无尾斜杠亦可",
                            cx,
                        ))
                        .child(Input::new(&self.jf_server_input).cleanable(true))
                        .into_any_element(),
                    v_flex()
                        .gap_1()
                        .child(Self::field_label(
                            "Jellyfin API Token",
                            "获取成功后会自动保存到本机",
                            cx,
                        ))
                        .child(Input::new(&self.jf_token_input).cleanable(true))
                        .into_any_element(),
                ],
                SourceMode::FnOs => vec![
                    v_flex()
                        .gap_1()
                        .child(Self::field_label(
                            "飞牛影视服务器地址",
                            "形如 http://192.168.1.10:5666（不含 /v 等路径）",
                            cx,
                        ))
                        .child(Input::new(&self.fnos_server_input).cleanable(true))
                        .into_any_element(),
                    v_flex()
                        .gap_1()
                        .child(Self::field_label(
                            "飞牛影视账号",
                            "飞牛影视自己的账号，不是飞牛系统登录账号",
                            cx,
                        ))
                        .child(Input::new(&self.fnos_user_input).cleanable(true))
                        .into_any_element(),
                    v_flex()
                        .gap_1()
                        .child(Self::field_label(
                            "飞牛影视密码",
                            "仅保存在本机，用于换取登录 token",
                            cx,
                        ))
                        .child(Input::new(&self.fnos_password_input).cleanable(true))
                        .into_any_element(),
                ],
            })
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        Button::new("fetch")
                            .primary()
                            .icon(Icon::empty().path("icons/refresh-cw.svg"))
                            .label("获取视频信息")
                            .loading(loading)
                            .disabled(loading)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.start_fetch(window, cx);
                            })),
                    )
                    .children((!hint.is_empty()).then(|| {
                        Label::new(hint)
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                    })),
            )
    }

    /// 左栏：合集信息卡 + 科目选择 + 操作行。
    pub(super) fn render_ready_left(
        &self,
        rd: &ReadyState,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let theme = cx.theme().clone();
        let mut out: Vec<gpui::AnyElement> = Vec::new();

        // 合集信息卡
        let mut info: Vec<gpui::AnyElement> = Vec::new();
        info.push(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    Icon::empty()
                        .path("icons/info.svg")
                        .size_4()
                        .text_color(theme.primary),
                )
                .child(
                    Label::new(format!("合集：《{}》", rd.season_title))
                        .text_size(px(15.))
                        .font_weight(FontWeight::BOLD),
                )
                .into_any_element(),
        );
        info.push(Self::meta_line("结构识别", &rd.structure, &theme).into_any_element());
        info.push(
            Self::meta_line("识别科目数", &rd.groups.len().to_string(), &theme).into_any_element(),
        );
        if let Some(p) = &rd.plan {
            for line in [
                format!("统计范围：{}", p.scope_desc),
                format!(
                    "总时长：{}（{}）",
                    fmt_seconds(p.total as f64, true),
                    fmt_human(p.total as f64)
                ),
                format!("目标天数：{} 天", p.days),
                format!(
                    "日均观看：{}（约 {:.1} 分钟/天）",
                    fmt_seconds(p.avg, true),
                    p.avg / 60.0
                ),
            ] {
                info.push(
                    Label::new(line)
                        .text_size(px(13.))
                        .text_color(theme.muted_foreground)
                        .into_any_element(),
                );
            }
        }
        out.push(
            entrance(
                "anim-info",
                0.18,
                bcard(cx)
                    .child(section_band("02 · 合集信息", "icons/info.svg", cx))
                    .children(info),
            )
            .into_any_element(),
        );

        // 多科目选择
        if rd.groups.len() > 1 {
            let dark = cx.theme().is_dark();
            let is_all_selected = matches!(rd.selection, Selection::All);
            let all_total_eps: usize = rd.groups.iter().map(|g| g.episodes.len()).sum();
            let all_total_dur: i64 = rd
                .groups
                .iter()
                .flat_map(|g| g.episodes.iter())
                .map(|e| e.duration)
                .sum();

            let mut items: Vec<gpui::AnyElement> = Vec::new();

            // 1. 全部科目
            items.push(
                Self::render_subject_item(
                    "sel-all",
                    is_all_selected,
                    "整个合集（全部科目）".to_string(),
                    Some(format!(
                        "{} 个视频 · {}",
                        all_total_eps,
                        fmt_seconds(all_total_dur as f64, true)
                    )),
                    &theme,
                    dark,
                    cx.listener(|this, _, window, cx| {
                        this.set_selection(Selection::All, window, cx);
                    }),
                )
                .into_any_element(),
            );

            // 2. 各单科目
            for (i, g) in rd.groups.iter().enumerate() {
                let is_selected = matches!(rd.selection, Selection::Single(ix) if ix == i);
                let total: i64 = g.episodes.iter().map(|e| e.duration).sum();
                items.push(
                    Self::render_subject_item(
                        ("sel-group", i),
                        is_selected,
                        format!("{}. {}", i + 1, g.name),
                        Some(format!(
                            "{} 个视频 · {}",
                            g.episodes.len(),
                            fmt_seconds(total as f64, true)
                        )),
                        &theme,
                        dark,
                        cx.listener(move |this, _, window, cx| {
                            this.set_selection(Selection::Single(i), window, cx);
                        }),
                    )
                    .into_any_element(),
                );
            }

            out.push(
                entrance(
                    "anim-groups",
                    0.26,
                    bcard(cx)
                        .child(section_band("03 · 科目选择", "icons/filter.svg", cx))
                        .child(v_flex().w_full().min_w_0().gap_2().children(items)),
                )
                .into_any_element(),
            );
        }

        // 操作行
        let has_plan = rd.plan.is_some();
        out.push(
            entrance(
                "anim-actions",
                0.34,
                bcard(cx)
                    .child(section_band("04 · 计划操作", "icons/calendar-days.svg", cx))
                    .child(
                        h_flex()
                            .gap_3()
                            .child(
                                Button::new("generate")
                                    .primary()
                                    .icon(Icon::empty().path("icons/calendar-days.svg"))
                                    .label("生成观看计划")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.regenerate(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("export")
                                    .icon(Icon::empty().path("icons/download.svg"))
                                    .label("导出计划文本（UTF-8）")
                                    .disabled(!has_plan)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.start_export(window, cx);
                                    })),
                            ),
                    ),
            )
            .into_any_element(),
        );

        // 05 · 加入打卡计划（生成计划后呈现）
        if has_plan {
            let skip_weekends = self.skip_weekends_toggle;
            out.push(
                entrance(
                    "anim-enroll",
                    0.42,
                    bcard(cx)
                        .child(section_band(
                            "05 · 开启进度打卡",
                            "icons/square-check.svg",
                            cx,
                        ))
                        .child(
                            v_flex()
                                .gap_3()
                                .child(
                                    h_flex()
                                        .gap_3()
                                        .items_end()
                                        .child(
                                            v_flex()
                                                .gap_1()
                                                .flex_1()
                                                .child(Self::field_label(
                                                    "起始学习日期",
                                                    "YYYY-MM-DD，默认今天",
                                                    cx,
                                                ))
                                                .child(Input::new(&self.start_date_input)),
                                        )
                                        .child(
                                            v_flex()
                                                .gap_1()
                                                .flex_1()
                                                .child(Self::field_label(
                                                    "今天是第几天（可选）",
                                                    "填写后自动倒推，并优先于起始日期",
                                                    cx,
                                                ))
                                                .child(Input::new(&self.today_plan_day_input)),
                                        )
                                        .child(
                                            Button::new("toggle-weekend")
                                                .label(if skip_weekends {
                                                    "跳过周末：是 ✅"
                                                } else {
                                                    "跳过周末：否 ⬜"
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.skip_weekends_toggle =
                                                        !this.skip_weekends_toggle;
                                                    cx.notify();
                                                })),
                                        ),
                                )
                                .child(
                                    Button::new("enroll-btn")
                                        .primary()
                                        .icon(Icon::empty().path("icons/square-check.svg"))
                                        .label("🚀 加入每日学习打卡并持久化")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.enroll_current_plan(window, cx);
                                        })),
                                ),
                        ),
                )
                .into_any_element(),
            );
        }

        out
    }
}
