//! fenestra 桌面应用：状态机、视图与消息处理。
//!
//! 与 Python 脚本功能一一对应；核心算法在 api / parse / plan / export
//! 模块中，本模块只做状态编排与 UI。
//!
//! ## UI：macOS Liquid Glass
//!
//! 整体使用 fenestra 内置的 [`Surface::Glass`](fenestra_core::Surface::Glass)
//! 材质——半透明 vibrancy tint 加 CPU 双通道背景模糊（`Material::popover`，
//! `blur_radius` 18），加上高光边缘（`SpecularEdge`）、定向主体光泽（`Sheen`）
//! 与背景自适应色温（`AdaptiveTint`）。页面背景用主题色 `accent_gradient`
//! 加少量装饰色斑，让模糊层采样到彩色内容；交互元素在 hover/press 时由
//! kit 自带的 state_layer + Fast color transition 保持玻璃质感一致。

use std::path::PathBuf;
use std::time::Duration;

use crate::api;
use crate::export;
use crate::parse::{self, EpisodeItem, Group};
use crate::plan::{build_plan, fmt_human, fmt_seconds, note_for, trunc, Mode, PlanEntry};
use crate::{extract_sid, Error};

use fenestra::prelude::*;
// fenestra 顶层重导出 fenestra_core::theme::Mode (Light/Dark)，与
// crate::plan::Mode (split/whole) 同名，这里用别名区分。
use fenestra::Mode as ThemeMode;

// ---------------------------------------------------------------------------
// 消息与状态
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub enum Msg {
    EditInput(String),
    EditCookie(String),
    EditDays(String),
    ModeChanged(usize),
    DarkToggled,
    Fetch,
    Fetched(Result<ReadyState, String>),
    SelectAll,
    SelectGroup(usize),
    Generate,
    Export,
    Exported(Result<PathBuf, String>),
    DismissToast(usize),
    ExpireToast(u64),
}

/// 科目统计范围选择。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    All,
    Single(usize),
}

/// 已获取的合集/视频数据。
#[derive(Debug, Clone)]
pub struct ReadyState {
    pub season_title: String,
    pub structure: String,
    pub groups: Vec<Group>,
    pub selection: Selection,
    pub plan: Option<PlanData>,
}

/// 已生成的观看计划。
#[derive(Debug, Clone)]
pub struct PlanData {
    pub plan: Vec<Vec<PlanEntry>>,
    pub capacities: Vec<i64>,
    pub total: i64,
    pub days: i64,
    pub avg: f64,
    pub scope_desc: String,
}

pub enum Phase {
    Input,
    Loading,
    Ready(ReadyState),
}

pub struct PlannerApp {
    pub input: String,
    pub cookie: String,
    pub days_text: String,
    pub mode: Mode,
    pub dark: bool,
    pub phase: Phase,
    pub last_error: Option<String>,
    pub toasts: Vec<(u64, String, Status)>,
    pub next_toast: u64,
    pub proxy: Option<Proxy<Msg>>,
}

impl PlannerApp {
    /// 创建默认状态的应用。
    pub fn new() -> Self {
        Self {
            input: String::new(),
            cookie: String::new(),
            days_text: String::new(),
            mode: Mode::Split,
            dark: false,
            phase: Phase::Input,
            last_error: None,
            toasts: Vec::new(),
            next_toast: 1,
            proxy: None,
        }
    }

    fn toast(&mut self, message: impl Into<String>, status: Status) {
        let id = self.next_toast;
        self.next_toast += 1;
        self.toasts.push((id, message.into(), status));
        if let Some(proxy) = self.proxy.clone() {
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(4));
                proxy.send(Msg::ExpireToast(id));
            });
        }
    }

    fn parse_days(&self) -> std::result::Result<i64, String> {
        let t = self.days_text.trim();
        if t.is_empty() {
            return Err("请先填写目标观看天数（正整数）。".to_string());
        }
        let days: i64 = t.parse().map_err(|_| "天数必须是正整数。".to_string())?;
        if days <= 0 {
            return Err("天数必须是正整数。".to_string());
        }
        Ok(days)
    }

    /// 用当前选择与天数生成计划；错误以 toast 提示。
    fn apply_generate(&mut self, days: i64) {
        let result = match &mut self.phase {
            Phase::Ready(rd) => generate_plan(rd, days, self.mode),
            _ => return,
        };
        match result {
            Ok(()) => {
                let warn = match &self.phase {
                    Phase::Ready(rd) => rd.plan.as_ref().map(|p| p.total).filter(|t| days > *t),
                    _ => None,
                };
                if let Some(total) = warn {
                    self.toast(
                        format!(
                            "提示：目标天数（{days}）大于总时长秒数（{total}），部分日期将为空闲/休息日。"
                        ),
                        Status::Warning,
                    );
                }
            }
            Err(e) => self.toast(format!("错误：{e}"), Status::Danger),
        }
    }

    fn set_selection(&mut self, sel: Selection) {
        if let Phase::Ready(rd) = &mut self.phase {
            rd.selection = sel;
            rd.plan = None;
        }
    }

    fn export_text(&self) -> Option<(String, String)> {
        match &self.phase {
            Phase::Ready(rd) => rd.plan.as_ref().map(|p| {
                let text = export::full_text(
                    &rd.season_title,
                    &rd.structure,
                    &p.scope_desc,
                    p.total,
                    p.days,
                    p.avg,
                    &rd.groups,
                    &p.plan,
                    &p.capacities,
                    p.total,
                    self.mode,
                );
                let file = format!("观看计划_{}.txt", sanitize(&rd.season_title));
                (text, file)
            }),
            _ => None,
        }
    }

    fn loading(&self) -> bool {
        matches!(self.phase, Phase::Loading)
    }

    // -----------------------------------------------------------------------
    // 视图
    // -----------------------------------------------------------------------

    fn view(&self) -> Element<Msg> {
        let theme = self.theme();

        // 顶栏玻璃面板：承载标题与亮/暗切换。
        let header = self.glass_header(&theme);

        // 装饰色斑层（绝对定位，全屏），位于玻璃卡片之下。accent_gradient
        // 已经提供主色变化，这里再叠几块低饱和度色斑，强化"毛玻璃背后有
        // 真实彩色内容"的层次感。
        let atmospheric = self.atmospheric_layer(&theme);

        let mut children: Vec<Element<Msg>> = vec![atmospheric, header];

        // 表单玻璃卡片。
        children.push(self.form_card());

        if let Some(err) = &self.last_error {
            children.push(self.error_callout(err));
        }

        match &self.phase {
            Phase::Loading => children.push(self.loading_row()),
            Phase::Ready(rd) => children.extend(self.ready_children(rd)),
            Phase::Input => {}
        }

        children.push(Element::from(
            toast_stack(self.toasts.iter().map(|(_, m, s)| (m.clone(), *s)))
                .on_dismiss(Msg::DismissToast),
        ));

        // 根容器：accent_gradient 作为页面底色（玻璃层要从中采样模糊内容），
        // 并启用 animate_layout 让合集状态切换时各卡片位置平滑过渡。
        col()
            .w_full()
            .h_full()
            .bg(theme.accent_gradient(135.0))
            .p(SP6)
            .gap(SP4)
            .scroll_y()
            .animate_layout()
            .children(children)
    }

    /// Liquid Glass 风格的顶栏：玻璃面板承载标题与亮/暗切换按钮。
    fn glass_header(&self, theme: &Theme) -> Element<Msg> {
        // 顶栏右侧的二级按钮：叠一层自定义玻璃 fill + highlight top sheen，
        // 让 Secondary 按钮在玻璃面板中保持"半透明胶囊"质感；hover/press
        // 时由 kit state_layer 平滑叠加文字色 veil，玻璃底纹不破碎。
        let theme_toggle = Element::from(
            button(if self.dark { "亮色" } else { "暗色" })
                .variant(ButtonVariant::Secondary)
                .on_click(Msg::DarkToggled),
        )
        .themed(move |t: &Theme, s| {
            s.bg(t.surface_raised.with_alpha(0.45))
                .border(1.0, t.border_subtle)
                .highlight_top(t.on_accent.with_alpha(0.10))
        });

        let _ = theme; // 当前未直接使用，预留给后续 per-element 主题
        row()
            .w_full()
            .items_center()
            .justify_between()
            .px(SP4)
            .py(SP3)
            .rounded(R_LG)
            .surface(Surface::Glass)
            .overflow_hidden()
            .transition(Transition::colors())
            .children([
                text("Bilibili 合集观看计划")
                    .size(TextSize::Xl)
                    .weight(Weight::Semibold),
                theme_toggle,
            ])
    }

    /// 装饰性低饱和度色斑，给毛玻璃模糊层提供采样内容；同时让暗色 / 亮色
    /// 模式下都有彩色基调。三个绝对定位的大圆斑，按 z-order 在玻璃卡片之下。
    fn atmospheric_layer(&self, theme: &Theme) -> Element<Msg> {
        let _ = theme; // 颜色由 themed() 闭包内部解析
                       // 左上：品牌色高饱和斑（accent step 10）
        let blob_a = div()
            .absolute()
            .top(-180.0)
            .left(-140.0)
            .w(560.0)
            .h(560.0)
            .rounded_full()
            .themed(|t: &Theme, s| s.bg(t.accents.step(10).with_alpha(0.42)));
        // 右下：品牌色中饱和斑（accent step 8）做色彩呼应
        let blob_b = div()
            .absolute()
            .bottom(-220.0)
            .right(-180.0)
            .w(640.0)
            .h(640.0)
            .rounded_full()
            .themed(|t: &Theme, s| s.bg(t.accents.step(8).with_alpha(0.36)));
        // 中右：浅色 step 7 斑，让背景略偏冷蓝
        let blob_c = div()
            .absolute()
            .top(80.0)
            .right(120.0)
            .w(380.0)
            .h(380.0)
            .rounded_full()
            .themed(|t: &Theme, s| s.bg(t.accents.step(7).with_alpha(0.28)));
        // 左下：浅色 step 6 斑，进一步丰富背景
        let blob_d = div()
            .absolute()
            .bottom(40.0)
            .left(60.0)
            .w(420.0)
            .h(420.0)
            .rounded_full()
            .themed(|t: &Theme, s| s.bg(t.accents.step(6).with_alpha(0.30)));

        div()
            .absolute()
            .top(0.0)
            .left(0.0)
            .w_full()
            .h_full()
            .overflow_hidden()
            .children([blob_a, blob_b, blob_c, blob_d])
    }

    /// 玻璃材质卡片（替代 kit `card()`）：padding/gap 与 `card()` 一致，
    /// 但渲染为 `Surface::Glass`（毛玻璃 + 边缘高光 + 深阴影）。
    /// `overflow_hidden` 让玻璃面板里的 sheen 高光和子元素不会突破圆角。
    fn glass_card<Msg: 'static>() -> Element<Msg> {
        col()
            .p(SP6)
            .gap(SP3)
            .surface(Surface::Glass)
            .overflow_hidden()
            .transition(Transition::colors())
    }

    fn form_card(&self) -> Element<Msg> {
        let loading = self.loading();

        // 玻璃化输入框：在 kit 输入控件上直接应用 Surface::Glass 材质——
        // kit 的 text_input 自身有 bg/border，surface() 会用玻璃 fill + 高光
        // 边缘覆盖它们，但保留 hover（border_strong）/ focus（accent）两条
        // themed 路径，因此输入框在玻璃面板中既透出底色又有清晰的聚焦环。
        let link_input = Element::from(
            text_input(&self.input)
                .placeholder("https://www.bilibili.com/video/BV1ps4y1d73V 或 BV 号 或 sid=6789")
                .width(560.0)
                .on_input(Msg::EditInput)
                .id("input"),
        )
        .surface(Surface::Glass);

        let days_input = Element::from(
            text_input(&self.days_text)
                .placeholder("如 30")
                .width(120.0)
                .on_input(Msg::EditDays)
                .id("days"),
        )
        .surface(Surface::Glass);

        let cookie_input = Element::from(
            text_input(&self.cookie)
                .placeholder("SESSDATA=xxx")
                .width(560.0)
                .on_input(Msg::EditCookie)
                .id("cookie"),
        )
        .surface(Surface::Glass);

        // 主按钮（玻璃上的实色强调）：保留 Primary 实色填充，但在玻璃表面
        // 上加 highlight_top sheen 让按钮本身也呈现高光，与 Liquid Glass
        // 调性一致；press 时由 kit 默认的 active_themed 切换到 accent_active。
        let primary_btn = Element::from(
            button("获取视频信息")
                .on_click(Msg::Fetch)
                .disabled(loading),
        )
        .themed(|t: &Theme, s| s.highlight_top(t.on_accent.with_alpha(0.18)));

        let days_field = field("目标观看天数").child(days_input);
        let mode_field = field("计划模式").child(segmented(
            self.mode.index(),
            ["split 精确切分", "whole 完整不拆"],
            Msg::ModeChanged,
        ));
        Self::glass_card().children([
            Element::from(
                field("链接 / BV 号 / 合集 sid")
                    .help("支持 https://www.bilibili.com/video/BVxxxx、BV 号或合集 sid=xxxx 链接")
                    .child(link_input),
            ),
            row()
                .gap(SP4)
                .items_end()
                .children([Element::from(days_field), Element::from(mode_field)]),
            Element::from(
                field("Cookie（可选，风控时使用）")
                    .help("例如 SESSDATA=xxx；留空则匿名请求")
                    .child(cookie_input),
            ),
            row().gap(SP3).items_center().children([
                primary_btn,
                if loading {
                    text("获取中…请稍候").size(TextSize::Sm)
                } else {
                    text("提示：B 站接口可能触发风控，失败时可添加 Cookie 重试")
                        .size(TextSize::Sm)
                        .themed(|t: &Theme, s| s.color(t.text_muted))
                },
            ]),
        ])
    }

    fn error_callout(&self, err: &str) -> Element<Msg> {
        // 错误提示玻璃面板：kit `callout()` 自身已有 status 色调；包一层玻璃
        // 让错误信息在半透明面板上更醒目，outer rounded + inner rounded 同
        // 心（Surface::Glass 的半径减去 SP1）。
        callout(Status::Danger, err.to_string())
            .surface(Surface::Glass)
            .overflow_hidden()
    }

    fn loading_row(&self) -> Element<Msg> {
        row()
            .gap(SP2)
            .items_center()
            .surface(Surface::Glass)
            .overflow_hidden()
            .p(SP4)
            .rounded(R_LG)
            .children([spinner(), text("正在获取视频信息…")])
    }

    fn ready_children(&self, rd: &ReadyState) -> Vec<Element<Msg>> {
        let mut out: Vec<Element<Msg>> = Vec::new();

        // 结构卡片
        let mut info: Vec<Element<Msg>> = vec![
            text(format!("合集：《{}》", rd.season_title)).weight(Weight::Semibold),
            text(format!("结构识别：{}", rd.structure))
                .size(TextSize::Sm)
                .themed(|t: &Theme, s| s.color(t.text_muted)),
            text(format!("识别科目数：{}", rd.groups.len()))
                .size(TextSize::Sm)
                .themed(|t: &Theme, s| s.color(t.text_muted)),
        ];
        if let Some(p) = &rd.plan {
            info.extend([
                text(format!("统计范围：{}", p.scope_desc)).size(TextSize::Sm),
                text(format!(
                    "总时长：{}（{}）",
                    fmt_seconds(p.total as f64, true),
                    fmt_human(p.total as f64)
                ))
                .size(TextSize::Sm),
                text(format!("目标天数：{} 天", p.days)).size(TextSize::Sm),
                text(format!(
                    "日均观看：{}（约 {:.1} 分钟/天）",
                    fmt_seconds(p.avg, true),
                    p.avg / 60.0
                ))
                .size(TextSize::Sm),
            ]);
        }
        out.push(Self::glass_card().children(info));

        // 多科目选择
        if rd.groups.len() > 1 {
            let mut sel: Vec<Element<Msg>> = vec![
                text("科目选择（统计范围）").weight(Weight::Semibold),
                Element::from(
                    radio(rd.selection == Selection::All)
                        .label("整个合集（全部科目）")
                        .on_select(Msg::SelectAll),
                ),
            ];
            for (i, g) in rd.groups.iter().enumerate() {
                let total: i64 = g.episodes.iter().map(|e| e.duration).sum();
                sel.push(Element::from(
                    radio(matches!(&rd.selection, Selection::Single(si) if *si == i))
                        .label(format!(
                            "{}. {}（{} 个视频，共 {}）",
                            i + 1,
                            g.name,
                            g.episodes.len(),
                            fmt_seconds(total as f64, true)
                        ))
                        .on_select(Msg::SelectGroup(i)),
                ));
            }
            out.push(Self::glass_card().children(sel));
        }

        // 操作按钮：主按钮（生成）保留 Primary 实色，副按钮（导出）用玻璃
        // 玻璃的 Secondary 半透明胶囊，与整体调性统一。
        let has_plan = rd.plan.is_some();
        let generate_btn = Element::from(
            button("生成观看计划")
                .variant(ButtonVariant::Secondary)
                .on_click(Msg::Generate),
        )
        .themed(|t: &Theme, s| {
            s.bg(t.surface_raised.with_alpha(0.45))
                .border(1.0, t.border_subtle)
                .highlight_top(t.on_accent.with_alpha(0.10))
        });
        let export_btn = Element::from(
            button("导出计划文本（UTF-8）")
                .on_click(Msg::Export)
                .disabled(!has_plan),
        )
        .themed(|t: &Theme, s| s.highlight_top(t.on_accent.with_alpha(0.18)));
        out.push(row().gap(SP3).children([generate_btn, export_btn]));

        // 计划表格：包裹玻璃面板，让表格内容悬浮在毛玻璃上。
        if let Some(p) = &rd.plan {
            out.push(self.plan_table_glass(p));
        } else {
            out.push(
                text("填写目标天数后点击「生成观看计划」。")
                    .size(TextSize::Sm)
                    .themed(|t: &Theme, s| s.color(t.text_muted)),
            );
        }

        out
    }

    fn plan_table_glass(&self, p: &PlanData) -> Element<Msg> {
        let mut rows: Vec<Vec<String>> = Vec::new();
        let mut cumulative: i64 = 0;
        for (di, entries) in p.plan.iter().enumerate() {
            let day_total: i64 = entries.iter().map(|e| e.portion).sum();
            cumulative += day_total;
            let remaining = p.total - cumulative;
            let summary = format!(
                "【第 {} 天】目标 {} ｜ 当日累计 {} ｜ 进度 {:.1}% ｜ 剩余总时长 {}",
                di + 1,
                fmt_seconds(p.capacities[di] as f64, true),
                fmt_seconds(day_total as f64, true),
                cumulative as f64 / p.total as f64 * 100.0,
                fmt_seconds(remaining as f64, true),
            );
            if entries.is_empty() {
                rows.push(vec![
                    (di + 1).to_string(),
                    String::new(),
                    "（本日无安排 / 休息）".to_string(),
                    String::new(),
                    summary,
                ]);
                continue;
            }
            rows.push(vec![
                (di + 1).to_string(),
                String::new(),
                summary,
                String::new(),
                String::new(),
            ]);
            for e in entries {
                rows.push(vec![
                    String::new(),
                    format!("#{}", e.vid_no),
                    trunc(&e.title, 36),
                    fmt_seconds(e.portion as f64, true),
                    trunc(&note_for(e, di), 56),
                ]);
            }
        }

        // 玻璃面板包住 data_table；面板的 SP1 padding 让表格内容与玻璃边缘
        // 留呼吸空间，overflow_hidden 让表格内部可能溢出的横线不出破圆角。
        col()
            .p(SP1)
            .rounded(R_LG)
            .surface(Surface::Glass)
            .overflow_hidden()
            .child(Element::from(
                data_table(["天", "视频#", "标题", "本日时长", "备注"], rows)
                    .id("plan-table")
                    .column_widths([64.0, 84.0, 320.0, 116.0, 400.0]),
            ))
    }
}

// ---------------------------------------------------------------------------
// 业务逻辑（无 GUI 依赖）
// ---------------------------------------------------------------------------

/// 后台线程执行：获取视频信息并识别结构。
pub fn fetch_and_parse(input: &str, cookie: Option<&str>) -> Result<ReadyState, String> {
    let r = (|| -> crate::Result<ReadyState> {
        let (season_title, groups, structure) = if let Some(sid) = extract_sid(input) {
            let flat = api::fetch_season_archives(sid, cookie)?;
            if flat.is_empty() {
                return Err(Error::data("未获取到任何视频，请检查链接是否正确。"));
            }
            let groups = vec![Group {
                name: format!("合集{sid}"),
                episodes: flat
                    .into_iter()
                    .map(|it| EpisodeItem {
                        title: it.title,
                        duration: it.duration,
                    })
                    .collect(),
            }];
            (
                format!("合集{sid}"),
                groups,
                "合集归档接口（sid 链接）".to_string(),
            )
        } else {
            let bvid = parse::extract_bvid(input)?;
            let view = api::fetch_view(&bvid, cookie)?;
            let r = parse::parse_groups(&view, cookie, &parse::default_fallback)?;
            (r.season_title, r.groups, r.structure)
        };
        let selection = if groups.len() > 1 {
            Selection::All
        } else {
            Selection::Single(0)
        };
        Ok(ReadyState {
            season_title,
            structure,
            groups,
            selection,
            plan: None,
        })
    })();
    r.map_err(|e| e.message().to_string())
}

/// 用当前科目选择与天数生成计划。
pub fn generate_plan(rd: &mut ReadyState, days: i64, mode: Mode) -> Result<(), String> {
    let (items, scope_desc) = match &rd.selection {
        Selection::All => {
            let mut items: Vec<EpisodeItem> = Vec::new();
            for (i, g) in rd.groups.iter().enumerate() {
                for ep in &g.episodes {
                    items.push(EpisodeItem {
                        title: format!("[科目{}] {}", i + 1, ep.title),
                        duration: ep.duration,
                    });
                }
            }
            (items, "整个合集（全部科目）".to_string())
        }
        Selection::Single(gi) => {
            let g = rd
                .groups
                .get(*gi)
                .ok_or_else(|| "科目编号超出范围".to_string())?;
            (
                g.episodes.clone(),
                format!("{}（{} 个视频）", g.name, g.episodes.len()),
            )
        }
    };
    let total: i64 = items.iter().map(|i| i.duration).sum();
    if total <= 0 {
        return Err("统计范围内视频总时长为 0。".to_string());
    }
    let out = build_plan(&items, days, mode)?;
    let avg = total as f64 / days as f64;
    rd.plan = Some(PlanData {
        plan: out.plan,
        capacities: out.capacities,
        total,
        days,
        avg,
        scope_desc,
    });
    Ok(())
}

/// 文件名安全化。
fn sanitize(s: &str) -> String {
    s.chars()
        .filter(|c| {
            !matches!(
                c,
                '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\n' | '\r'
            )
        })
        .take(40)
        .collect()
}

// ---------------------------------------------------------------------------
// App 实现
// ---------------------------------------------------------------------------

impl Default for PlannerApp {
    fn default() -> Self {
        Self::new()
    }
}

impl App for PlannerApp {
    type Msg = Msg;

    fn init(&mut self, proxy: Proxy<Msg>) {
        self.proxy = Some(proxy);
    }

    fn update(&mut self, msg: Msg) {
        match msg {
            Msg::EditInput(s) => {
                self.input = s;
                self.last_error = None;
            }
            Msg::EditCookie(s) => self.cookie = s,
            Msg::EditDays(s) => {
                self.days_text = s;
                // 天数变化后，旧计划可能不再有效
                if let Phase::Ready(rd) = &mut self.phase {
                    rd.plan = None;
                }
            }
            Msg::ModeChanged(i) => {
                self.mode = Mode::from_index(i);
                if let Phase::Ready(rd) = &mut self.phase {
                    rd.plan = None;
                }
            }
            Msg::DarkToggled => self.dark = !self.dark,
            Msg::Fetch => {
                self.phase = Phase::Loading;
                self.last_error = None;
                let input = self.input.clone();
                let cookie = if self.cookie.trim().is_empty() {
                    None
                } else {
                    Some(self.cookie.clone())
                };
                if let Some(proxy) = self.proxy.clone() {
                    std::thread::spawn(move || {
                        proxy.send(Msg::Fetched(fetch_and_parse(&input, cookie.as_deref())));
                    });
                }
            }
            Msg::Fetched(Ok(mut rd)) => {
                match self.parse_days() {
                    Ok(days) => {
                        let result = generate_plan(&mut rd, days, self.mode);
                        if let Err(e) = result {
                            self.toast(format!("错误：{e}"), Status::Danger);
                        } else if let Some(p) = &rd.plan {
                            if days > p.total {
                                self.toast(
                                    format!(
                                        "提示：目标天数（{days}）大于总时长秒数（{}），部分日期将为空闲/休息日。",
                                        p.total
                                    ),
                                    Status::Warning,
                                );
                            }
                        }
                    }
                    Err(e) => self.toast(e, Status::Warning),
                }
                self.phase = Phase::Ready(rd);
            }
            Msg::Fetched(Err(e)) => {
                self.phase = Phase::Input;
                self.last_error = Some(e);
            }
            Msg::SelectAll => {
                self.set_selection(Selection::All);
                if let Ok(days) = self.parse_days() {
                    self.apply_generate(days);
                }
            }
            Msg::SelectGroup(i) => {
                self.set_selection(Selection::Single(i));
                if let Ok(days) = self.parse_days() {
                    self.apply_generate(days);
                }
            }
            Msg::Generate => match self.parse_days() {
                Ok(days) => self.apply_generate(days),
                Err(e) => self.toast(e, Status::Warning),
            },
            Msg::Export => {
                let payload = self.export_text();
                match payload {
                    Some((text, suggested)) => {
                        if let Some(proxy) = self.proxy.clone() {
                            std::thread::spawn(move || {
                                let picked = pollster::block_on(
                                    rfd::AsyncFileDialog::new()
                                        .set_file_name(&suggested)
                                        .save_file(),
                                );
                                if let Some(file) = picked {
                                    let path = file.path().to_path_buf();
                                    match std::fs::write(&path, text) {
                                        Ok(()) => proxy.send(Msg::Exported(Ok(path))),
                                        Err(e) => proxy.send(Msg::Exported(Err(format!(
                                            "无法保存计划文件：{e}"
                                        )))),
                                    }
                                }
                            });
                        }
                    }
                    None => self.toast("请先生成观看计划。", Status::Warning),
                }
            }
            Msg::Exported(Ok(path)) => {
                self.toast(format!("计划已保存至：{}", path.display()), Status::Success);
            }
            Msg::Exported(Err(e)) => {
                self.toast(format!("警告：{e}"), Status::Warning);
            }
            Msg::DismissToast(i) => {
                if i < self.toasts.len() {
                    self.toasts.remove(i);
                }
            }
            Msg::ExpireToast(id) => self.toasts.retain(|(tid, ..)| *tid != id),
        }
    }

    fn theme(&self) -> Theme {
        // Liquid Glass 配色：duotone 中性场（冷色 220°，适度饱和度 4）+ 同
        // 色系重音（200° 青蓝），让页面既有彩色基调又有冷色玻璃感。提高
        // corner_smoothing 到 0.8，把所有圆角推向 Apple "fuller squircle"
        // 调性，配合 Surface::Glass 的边缘高光形成统一语言。
        let mode = if self.dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        };
        Theme::duotone(220.0, 4.0, 200.0, mode).with_corner_smoothing(0.8)
    }

    fn view(&self) -> Element<Msg> {
        PlannerApp::view(self)
    }
}
