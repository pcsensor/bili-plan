//! gpui-component 桌面应用：状态机、视图与交互。
//!
//! 业务编排全部在 [`crate::core`]（无 GUI 依赖）；本模块只负责把状态
//! 渲染为 gpui-component 组件树，并在事件回调中驱动 core 的纯函数。
//!
//! ## 视图结构
//!
//! - `TitleBar`：应用标题 + 亮/暗主题切换（macOS 下与系统红绿灯融合）。
//! - 「来源」卡片：来源切换、链接输入、来源专属凭证字段、天数与计划模式、
//!   获取按钮、错误横幅。
//! - 「结果」区：合集信息卡、科目选择（多科目时）、生成/导出操作行、
//!   虚拟滚动计划表（`Table`，O(1) 渲染可视行）。
//! - 反馈走 `Notification`（自动消失），替代旧版 toast。

use std::path::PathBuf;

use gpui::{
    div, prelude::*, px, App, Context, Entity, FontWeight, InteractiveElement, IntoElement, Render,
    Styled, Window,
};
use gpui_component::{
    alert::Alert,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputEvent, InputState},
    label::Label,
    notification::Notification,
    radio::{Radio, RadioGroup},
    table::{Column, Table, TableDelegate, TableState},
    v_flex, ActiveTheme, Disableable, Icon, IconName, Sizable, Theme, ThemeMode, TitleBar,
    WindowExt,
};

use crate::core::{
    export_payload, generate_plan, load_config, parse_days, save_config, FetchSource,
    JellyfinConfig, ReadyState, Selection, SourceMode,
};
use crate::plan::{fmt_human, fmt_seconds, note_for, trunc, Mode};

// ---------------------------------------------------------------------------
// 状态
// ---------------------------------------------------------------------------

pub enum Phase {
    Input,
    Loading,
    Ready(ReadyState),
}

pub struct PlannerApp {
    /// gpui-component 输入框为独立 `Entity<InputState>`，这里持有引用并
    /// 在渲染时绑定；取值通过 `read(cx).value()` 按需读取。
    link_input: Entity<InputState>,
    cookie_input: Entity<InputState>,
    jf_server_input: Entity<InputState>,
    jf_token_input: Entity<InputState>,
    days_input: Entity<InputState>,

    source: SourceMode,
    mode: Mode,
    phase: Phase,
    last_error: Option<String>,

    /// 计划表状态；生成/切换科目/修改天数时重建。
    plan_table: Option<Entity<TableState<PlanTableDelegate>>>,
}

impl PlannerApp {
    /// 创建应用视图（在 `open_window` 的构建回调内调用）。
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let link_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("https://www.bilibili.com/video/BV1ps4y1d73V 或 BV 号 或 sid=6789")
        });
        let cookie_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("SESSDATA=xxx")
                .masked(true)
        });
        let jf_server_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("https://media.example.com:8096"));
        let jf_token_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Jellyfin 后台「控制台 → 高级 → API 密钥」生成")
                .masked(true)
        });
        let days_input = cx.new(|cx| InputState::new(window, cx).placeholder("如 30"));

        // 链接变化即清除上一次的错误横幅。
        cx.subscribe(&link_input, |this, _, event, cx| {
            if matches!(event, InputEvent::Change) && this.last_error.is_some() {
                this.last_error = None;
                cx.notify();
            }
        })
        .detach();
        // 天数变化后，旧计划不再适用。
        cx.subscribe(&days_input, |this, _, event, cx| {
            if matches!(event, InputEvent::Change) && this.plan_table.is_some() {
                this.plan_table = None;
                if let Phase::Ready(rd) = &mut this.phase {
                    rd.plan = None;
                }
                cx.notify();
            }
        })
        .detach();

        // 启动时尝试加载本机 Jellyfin 凭证，预热字段——UI 上无需重新填写。
        if let Some(cfg) = load_config() {
            jf_server_input.update(cx, |state, cx| state.set_value(cfg.server_url, window, cx));
            jf_token_input.update(cx, |state, cx| state.set_value(cfg.token, window, cx));
        }

        Self {
            link_input,
            cookie_input,
            jf_server_input,
            jf_token_input,
            days_input,
            source: SourceMode::Bilibili,
            mode: Mode::Split,
            phase: Phase::Input,
            last_error: None,
            plan_table: None,
        }
    }

    // -----------------------------------------------------------------------
    // 交互动作
    // -----------------------------------------------------------------------

    fn input_value(&self, state: &Entity<InputState>, cx: &App) -> String {
        state.read(cx).value().to_string()
    }

    fn days(&self, cx: &App) -> Result<i64, String> {
        parse_days(&self.input_value(&self.days_input, cx))
    }

    /// 点击「获取视频信息」：前台做最小校验，网络请求放到后台执行器。
    fn start_fetch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.input_value(&self.link_input, cx);
        let cookie = {
            let v = self.input_value(&self.cookie_input, cx);
            if v.trim().is_empty() {
                None
            } else {
                Some(v)
            }
        };
        let source = match self.source {
            SourceMode::Bilibili => FetchSource::Bilibili { cookie },
            SourceMode::Jellyfin => {
                let server_url = self.input_value(&self.jf_server_input, cx);
                let token = self.input_value(&self.jf_token_input, cx);
                // 前端先做最小校验，避免起任务后才报错；core 会再 trim 检查。
                if server_url.trim().is_empty() || token.trim().is_empty() {
                    window.push_notification(
                        Notification::warning("请填写 Jellyfin 服务器地址与 API Token。"),
                        cx,
                    );
                    return;
                }
                FetchSource::Jellyfin { server_url, token }
            }
        };

        self.phase = Phase::Loading;
        self.last_error = None;
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { crate::core::fetch_and_parse(&input, &source) })
                .await;
            this.update_in(cx, |this, window, cx| this.on_fetched(result, window, cx))
                .ok();
        })
        .detach();
    }

    fn on_fetched(
        &mut self,
        result: Result<ReadyState, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(mut rd) => {
                // Jellyfin 来源且本次成功：默认写盘记住凭证，下次启动免重填。
                if matches!(self.source, SourceMode::Jellyfin) {
                    save_config(&JellyfinConfig {
                        server_url: self
                            .input_value(&self.jf_server_input, cx)
                            .trim()
                            .to_string(),
                        token: self
                            .input_value(&self.jf_token_input, cx)
                            .trim()
                            .to_string(),
                    });
                }
                // 获取成功后自动按当前天数与模式生成一次计划。
                match self.days(cx) {
                    Ok(days) => Self::run_generate(
                        &mut rd,
                        self.mode,
                        days,
                        &mut self.plan_table,
                        window,
                        cx,
                    ),
                    Err(e) => window.push_notification(Notification::warning(e), cx),
                }
                self.phase = Phase::Ready(rd);
                window.push_notification(Notification::success("已获取视频信息。"), cx);
            }
            Err(e) => {
                self.phase = Phase::Input;
                self.last_error = Some(e);
            }
        }
        cx.notify();
    }

    /// 在就绪状态上生成计划，并给出"天数 > 总时长"的提示。
    ///
    /// 以字段级参数规避借用冲突：调用方可能正持有 `self.phase` 的
    /// `&mut ReadyState` 借用。
    fn run_generate(
        rd: &mut ReadyState,
        mode: Mode,
        days: i64,
        plan_table: &mut Option<Entity<TableState<PlanTableDelegate>>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match generate_plan(rd, days, mode) {
            Ok(()) => {
                if let Some(p) = &rd.plan {
                    if days > p.total {
                        window.push_notification(
                            Notification::warning(format!(
                                "目标天数（{days}）大于总时长秒数（{}），部分日期将为空闲/休息日。",
                                p.total
                            )),
                            cx,
                        );
                    }
                }
                if let Some(plan) = &rd.plan {
                    let delegate = PlanTableDelegate::new(plan);
                    *plan_table = Some(cx.new(|cx| {
                        let mut state = TableState::new(delegate, window, cx);
                        // 只读展示表：关闭行/列选择、排序与拖拽。
                        state.col_selectable = false;
                        state.row_selectable = false;
                        state.col_movable = false;
                        state.col_resizable = false;
                        state.sortable = false;
                        state
                    }));
                }
            }
            Err(e) => window.push_notification(Notification::error(format!("错误：{e}")), cx),
        }
    }

    /// 点击「生成观看计划」。
    fn regenerate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let days = match self.days(cx) {
            Ok(days) => days,
            Err(e) => {
                window.push_notification(Notification::warning(e), cx);
                return;
            }
        };
        if let Phase::Ready(rd) = &mut self.phase {
            Self::run_generate(rd, self.mode, days, &mut self.plan_table, window, cx);
        }
        cx.notify();
    }

    /// 切换科目统计范围；沿用旧行为：天数有效时立即重新生成。
    fn set_selection(&mut self, sel: Selection, window: &mut Window, cx: &mut Context<Self>) {
        if let Phase::Ready(rd) = &mut self.phase {
            rd.selection = sel;
            rd.plan = None;
            self.plan_table = None;
        }
        if let Ok(days) = self.days(cx) {
            if let Phase::Ready(rd) = &mut self.phase {
                Self::run_generate(rd, self.mode, days, &mut self.plan_table, window, cx);
            }
        }
        cx.notify();
    }

    fn switch_source(&mut self, source: SourceMode, window: &mut Window, cx: &mut Context<Self>) {
        self.source = source;
        self.last_error = None;
        // 链接输入的 placeholder 随来源切换，避免误粘贴场景提示错位。
        let placeholder = match source {
            SourceMode::Bilibili => {
                "https://www.bilibili.com/video/BV1ps4y1d73V 或 BV 号 或 sid=6789"
            }
            SourceMode::Jellyfin => "https://host/web/#!/details?id=xxx 或直接粘贴 item ID",
        };
        self.link_input.update(cx, |state, cx| {
            state.set_placeholder(placeholder, window, cx)
        });
        // 来源切换：已识别的合集结构不再适用，丢弃旧的 Ready 状态。
        if matches!(self.phase, Phase::Ready(_)) {
            self.phase = Phase::Input;
            self.plan_table = None;
        }
        cx.notify();
    }

    fn switch_mode(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        self.mode = mode;
        // 先取天数再进入 phase 借用，避免可变借用冲突。
        let days = self.days(cx).ok();
        // 模式变化后，旧计划不再适用；天数有效时立即按新模式重建。
        if let Phase::Ready(rd) = &mut self.phase {
            rd.plan = None;
            self.plan_table = None;
            if let Some(days) = days {
                Self::run_generate(rd, mode, days, &mut self.plan_table, window, cx);
            }
        }
        cx.notify();
    }

    /// 点击「导出计划文本」：文件对话框与写盘在后台线程，完成后通知。
    fn start_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                Ok(path) => window.push_notification(
                    Notification::success(format!("计划已保存至：{}", path.display())),
                    cx,
                ),
                Err(e) if !e.is_empty() => {
                    window.push_notification(Notification::warning(e), cx);
                }
                Err(_) => {}
            })
            .ok();
        })
        .detach();
    }

    // -----------------------------------------------------------------------
    // 视图
    // -----------------------------------------------------------------------

    fn render_title_bar(&self, dark: bool, cx: &mut Context<Self>) -> impl IntoElement + use<> {
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
                    .child(Label::new("bili-planner — Bilibili & Jellyfin 观看计划")),
            )
            .child(div().flex_1())
            .child(toggle)
    }

    /// 来源切换用的成组按钮（gpui-component 无 segmented 控件）。
    fn source_toggle(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        h_flex()
            .gap_1()
            .child(
                Button::new("source-bilibili")
                    .small()
                    .label("B 站")
                    .when(self.source == SourceMode::Bilibili, |b| b.primary())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.switch_source(SourceMode::Bilibili, window, cx);
                    })),
            )
            .child(
                Button::new("source-jellyfin")
                    .small()
                    .label("Jellyfin")
                    .when(self.source == SourceMode::Jellyfin, |b| b.primary())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.switch_source(SourceMode::Jellyfin, window, cx);
                    })),
            )
    }

    /// 计划模式切换用的成组按钮。
    fn mode_toggle(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        h_flex()
            .gap_1()
            .child(
                Button::new("mode-split")
                    .small()
                    .label("split 精确切分")
                    .when(self.mode == Mode::Split, |b| b.primary())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.switch_mode(Mode::Split, window, cx);
                    })),
            )
            .child(
                Button::new("mode-whole")
                    .small()
                    .label("whole 完整不拆")
                    .when(self.mode == Mode::Whole, |b| b.primary())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.switch_mode(Mode::Whole, window, cx);
                    })),
            )
    }

    fn field_label(label: &str, help: &str, cx: &App) -> impl IntoElement + use<> {
        v_flex()
            .gap_1()
            .child(Label::new(label.to_string()).text_size(px(13.)))
            .child(
                Label::new(help.to_string())
                    .text_size(px(12.))
                    .text_color(cx.theme().muted_foreground),
            )
    }

    fn render_form_card(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
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
            }
        };

        v_flex()
            .gap_3()
            .p_5()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
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
                            .child(Label::new("计划模式").text_size(px(13.)))
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
                    .child(Input::new(&self.cookie_input).cleanable(true))]
                .into_iter()
                .map(|el| el.into_any_element())
                .collect::<Vec<_>>(),
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

    fn render_ready(&self, rd: &ReadyState, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
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
                .child(Label::new(format!("合集：《{}》", rd.season_title)).text_size(px(15.)))
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
        out.push(card(&theme).children(info).into_any_element());

        // 多科目选择
        if rd.groups.len() > 1 {
            let mut group = RadioGroup::vertical("group-sel")
                .selected_index(match rd.selection {
                    Selection::All => Some(0),
                    Selection::Single(i) => Some(i + 1),
                })
                .on_click(cx.listener(|this, ix: &usize, window, cx| {
                    let sel = if *ix == 0 {
                        Selection::All
                    } else {
                        Selection::Single(ix - 1)
                    };
                    this.set_selection(sel, window, cx);
                }))
                .child(Radio::new("sel-all").label("整个合集（全部科目）"));
            for (i, g) in rd.groups.iter().enumerate() {
                let total: i64 = g.episodes.iter().map(|e| e.duration).sum();
                group = group.child(Radio::new(("sel-group", i)).label(format!(
                    "{}. {}（{} 个视频，共 {}）",
                    i + 1,
                    g.name,
                    g.episodes.len(),
                    fmt_seconds(total as f64, true)
                )));
            }
            out.push(
                card(&theme)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                Icon::empty()
                                    .path("icons/filter.svg")
                                    .size_4()
                                    .text_color(theme.primary),
                            )
                            .child(Label::new("科目选择（统计范围）").text_size(px(15.))),
                    )
                    .child(group)
                    .into_any_element(),
            );
        }

        // 操作行
        let has_plan = rd.plan.is_some();
        out.push(
            h_flex()
                .gap_3()
                .child(
                    Button::new("generate")
                        .primary()
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
                )
                .into_any_element(),
        );

        // 计划表 / 占位提示
        match (&self.plan_table, &rd.plan) {
            (Some(table), _) => out.push(
                div()
                    .h(px(520.))
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.popover)
                    .overflow_hidden()
                    .child(Table::new(table).stripe(true))
                    .into_any_element(),
            ),
            (None, Some(_)) => out.push(
                Label::new("计划已失效，请点击「生成观看计划」重新生成。")
                    .text_size(px(13.))
                    .text_color(theme.muted_foreground)
                    .into_any_element(),
            ),
            (None, None) => out.push(
                Label::new("填写目标天数后点击「生成观看计划」。")
                    .text_size(px(13.))
                    .text_color(theme.muted_foreground)
                    .into_any_element(),
            ),
        }

        out
    }

    fn render_loading(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        h_flex()
            .gap_2()
            .items_center()
            .p_4()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .child(gpui_component::spinner::Spinner::new().small())
            .child(Label::new("正在获取视频信息…").text_size(px(13.)))
    }

    fn meta_line(label: &str, value: &str, theme: &gpui_component::ThemeColor) -> impl IntoElement {
        h_flex()
            .gap_2()
            .child(
                Label::new(format!("{label}："))
                    .text_size(px(13.))
                    .text_color(theme.muted_foreground),
            )
            .child(Label::new(value.to_string()).text_size(px(13.)))
    }
}

/// 卡片容器：面板底 + 发丝描边（shadcn Card 语言）。
fn card(theme: &gpui_component::ThemeColor) -> gpui::Div {
    v_flex()
        .gap_3()
        .p_5()
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
}

impl Render for PlannerApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dark = cx.theme().is_dark();
        let theme = cx.theme().clone();

        let mut content = v_flex()
            .id("content-scroll")
            .flex_1()
            .overflow_y_scroll()
            .px_6()
            .py_5()
            .gap_4()
            .child(self.render_form_card(cx));

        if let Some(err) = &self.last_error {
            content = content.child(
                Alert::error("fetch-error", err.clone())
                    .title("获取失败")
                    .flex_shrink_0(),
            );
        }

        match &self.phase {
            Phase::Loading => content = content.child(self.render_loading(cx)),
            Phase::Ready(rd) => {
                content = content.children(self.render_ready(rd, cx));
            }
            Phase::Input => {}
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(self.render_title_bar(dark, cx))
            .child(content)
    }
}

// ---------------------------------------------------------------------------
// 计划表（Table 委托）
// ---------------------------------------------------------------------------

/// 计划表的一行：日汇总行或视频行。
struct PlanRow {
    cells: [String; 5],
    is_day_head: bool,
}

/// `Table` 委托：静态 5 列 + 预构建行数据，虚拟滚动由 Table 内部处理。
struct PlanTableDelegate {
    columns: Vec<Column>,
    rows: Vec<PlanRow>,
}

impl PlanTableDelegate {
    fn new(plan: &crate::core::PlanData) -> Self {
        let columns = vec![
            Column::new("day", "天").width(px(64.)),
            Column::new("vid", "视频#").width(px(84.)),
            Column::new("title", "标题").width(px(360.)),
            Column::new("duration", "本日时长")
                .width(px(116.))
                .text_right(),
            Column::new("note", "备注").width(px(380.)),
        ];

        let mut rows: Vec<PlanRow> = Vec::new();
        let mut cumulative: i64 = 0;
        for (di, entries) in plan.plan.iter().enumerate() {
            let day_total: i64 = entries.iter().map(|e| e.portion).sum();
            cumulative += day_total;
            let remaining = plan.total - cumulative;
            // 每日汇总拆到「标题 / 备注」两列：表格行高固定，单列长文本会
            // 溢出；拆分后每行均单行显示，保持整洁单行排版。
            let day_head = format!(
                "【第 {} 天】目标 {} ｜ 累计 {}",
                di + 1,
                fmt_seconds(plan.capacities[di] as f64, true),
                fmt_seconds(day_total as f64, true),
            );
            let day_note = format!(
                "进度 {:.1}% ｜ 剩余总时长 {}",
                cumulative as f64 / plan.total as f64 * 100.0,
                fmt_seconds(remaining as f64, true),
            );
            if entries.is_empty() {
                rows.push(PlanRow {
                    cells: [
                        (di + 1).to_string(),
                        String::new(),
                        "（本日无安排 / 休息）".to_string(),
                        String::new(),
                        day_note,
                    ],
                    is_day_head: false,
                });
                continue;
            }
            rows.push(PlanRow {
                cells: [
                    (di + 1).to_string(),
                    String::new(),
                    day_head,
                    String::new(),
                    day_note,
                ],
                is_day_head: true,
            });
            for e in entries {
                rows.push(PlanRow {
                    cells: [
                        String::new(),
                        format!("#{}", e.vid_no),
                        trunc(&e.title, 22),
                        fmt_seconds(e.portion as f64, true),
                        trunc(&note_for(e, di), 28),
                    ],
                    is_day_head: false,
                });
            }
        }

        Self { columns, rows }
    }
}

impl TableDelegate for PlanTableDelegate {
    fn columns_count(&self, _cx: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _cx: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, col_ix: usize, _cx: &App) -> &Column {
        &self.columns[col_ix]
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut gpui::Context<TableState<Self>>,
    ) -> impl IntoElement {
        let row = &self.rows[row_ix];
        div()
            .text_size(px(13.))
            .when(row.is_day_head, |d| d.font_weight(FontWeight::SEMIBOLD))
            .text_color(if row.is_day_head {
                cx.theme().foreground
            } else {
                cx.theme().secondary_foreground
            })
            .whitespace_nowrap()
            .overflow_hidden()
            .child(row.cells[col_ix].clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{EpisodeItem, Group};
    use gpui::TestAppContext;

    /// 构造带两门科目、已生成 3 天计划的就绪状态。
    fn ready_with_plan() -> ReadyState {
        let groups = vec![
            Group {
                name: "第一章 基础".into(),
                episodes: vec![EpisodeItem {
                    title: "1.1 集合".into(),
                    duration: 3720,
                }],
            },
            Group {
                name: "第二章 进阶".into(),
                episodes: vec![EpisodeItem {
                    title: "2.1 图论".into(),
                    duration: 5400,
                }],
            },
        ];
        ReadyState {
            season_title: "测试合集".into(),
            structure: "多分栏合集".into(),
            groups,
            selection: Selection::All,
            plan: None,
        }
    }

    /// 无头渲染冒烟：窗口构建、计划表委托、亮/暗主题下的元素树构建
    /// 都不应 panic（替代旧 fenestra 访问树冒烟测试）。
    #[gpui::test]
    async fn render_smoke_both_themes(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::init(cx);
        });

        let window = cx.add_window(|window, cx| {
            let mut app = PlannerApp::new(window, cx);
            let mut rd = ready_with_plan();
            PlannerApp::run_generate(&mut rd, Mode::Split, 3, &mut app.plan_table, window, cx);
            assert!(rd.plan.is_some(), "计划应已生成");
            app.phase = Phase::Ready(rd);
            app
        });

        window
            .update(cx, |app, window, cx| {
                // 亮色：构建完整元素树（表单卡 + 信息卡 + 科目选择 + 表格）。
                let _ = app.render(window, cx);

                // 切换暗色后再次构建（主题配置重套用 + 渲染路径不 panic）。
                Theme::change(ThemeMode::Dark, Some(window), cx);
                let _ = app.render(window, cx);
            })
            .expect("window update should succeed");
    }
}
