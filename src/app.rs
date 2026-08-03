//! fenestra 桌面应用：状态机、视图与消息处理。
//!
//! 与 Python 脚本功能一一对应；核心算法在 api / parse / plan / export
//! 模块中，本模块只做状态编排与 UI。
//!
//! ## UI：Apple Human Interface Guidelines（1:1 复刻）
//!
//! - **极简布局**：纯色银灰底（亮色 `#F5F5F7` / 暗色深空灰 `#1B1B1D`），
//!   大留白（SP8/SP6 内边距、SP5 卡片间距），内容纵向卡片流。
//! - **字体**：内嵌 Inter（SF Pro 的度量兼容开源替代），CJK 由系统
//!   PingFang / Microsoft YaHei 回退；字号字重走 kit 光学字号梯度。
//! - **圆角**：`corner_smoothing 0.6` 的 Apple squircle 连续曲率，
//!   半径梯度 6/10/14/20；按钮为 Apple 标志性全圆角胶囊（pill）。
//! - **配色**：System Blue（亮 `#007AFF` / 暗 `#0A84FF`）、深空灰文字
//!   `#1D1D1F`、次要文字 `#86868B`、发丝线 `#D2D2D7`、iOS 系统状态色。
//! - **阴影与毛玻璃**：内容卡片为实心面板 + 发丝描边 + 细腻分层投影
//!   （`Surface::Card`）；浮动工具栏保留 `Surface::Glass` vibrancy
//!   （高光边缘 + Sheen + AdaptiveTint），与 macOS 工具栏质感一致。
//! - **动效**：统一 `cubic-bezier(0.25, 0.1, 0.25, 1.0)` Apple ease-out、
//!   250ms；布局变化走 `animate_layout` FLIP 弹簧；segmented 拇指弹簧。
//! - **图标**：lucide 线性图标（2px 圆头描边，最接近 SF Symbols 风格）。
//! - **响应式**：输入框全宽自适应，窗口缩放时卡片流自然伸缩。

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
// Apple HIG 设计系统
// ---------------------------------------------------------------------------

/// Apple 标志性动画曲线 `cubic-bezier(0.25, 0.1, 0.25, 1.0)`——iOS/macOS
/// 视图动画的标准 ease-out：起段迅速响应、尾段长衰减，体感"快而不突兀"。
const APPLE_EASE: CubicBezier = CubicBezier {
    x1: 0.25,
    y1: 0.1,
    x2: 0.25,
    y2: 1.0,
};

/// Apple 交互过渡：颜色/阴影统一 250ms + [`APPLE_EASE`]。
fn apple_transition() -> Transition {
    Transition::colors().easing(APPLE_EASE).duration_ms(250.0)
}

/// sRGB hex 便捷构造（`0xRRGGBB`）。
const fn hex(rgb: u32) -> Color {
    Color::from_rgb8(
        ((rgb >> 16) & 0xFF) as u8,
        ((rgb >> 8) & 0xFF) as u8,
        (rgb & 0xFF) as u8,
    )
}

/// 1:1 Apple Human Interface Guidelines 主题。
///
/// 以 System Blue 的 OKLCH 色相（≈259°）生成完整色板后，再用 Apple 官方
/// hex 精确覆写中性色阶、语义色与 iOS 状态色；圆角取 squircle 连续曲率。
fn apple_theme(mode: ThemeMode) -> Theme {
    let mut t = Theme::from_accent(259.0, mode).with_corner_smoothing(0.6);
    match mode {
        ThemeMode::Light => {
            // Apple 官网系灰阶：银灰底、深空灰文字、发丝线。
            t.neutrals = Ramp([
                hex(0xF5F5F7), // N1  页面底（Apple 标志性银灰）
                hex(0xF2F2F4), // N2  表头等次级底
                hex(0xE5E5EA), // N3  控件底（iOS systemGray5）
                hex(0xD9D9DE), // N4  控件 hover
                hex(0xD2D2D7), // N5  发丝线（Apple hairline）
                hex(0xC7C7CC), // N6  描边（iOS systemGray4）
                hex(0xAEAEB2), // N7  强描边（iOS systemGray2）
                hex(0x8E8E93), // N8  禁用文字（iOS systemGray）
                hex(0x86868B), // N9  次要文字（apple.com secondary）
                hex(0x6E6E73), // N10 弱化正文
                hex(0x48484A), // N11 强调正文
                hex(0x1D1D1F), // N12 主文字（深空灰黑）
            ]);
            t.bg = hex(0xF5F5F7);
            t.surface = hex(0xFFFFFF);
            t.surface_raised = hex(0xFFFFFF);
            t.element = hex(0xE5E5EA);
            t.element_hover = hex(0xD9D9DE);
            t.element_active = hex(0xD2D2D7);
            t.border_subtle = hex(0xD2D2D7);
            t.border = hex(0xC7C7CC);
            t.border_strong = hex(0xAEAEB2);
            t.text = hex(0x1D1D1F);
            t.text_muted = hex(0x6E6E73);
            t.text_subtle = hex(0x86868B);
            t.text_disabled = hex(0x8E8E93);
            // System Blue + apple.com 按钮蓝梯度。
            t.accent = hex(0x007AFF);
            t.accent_hover = hex(0x0071E3);
            t.accent_active = hex(0x0062C4);
            t.accent_bg = hex(0xE5F1FF);
            t.accent_border = hex(0x4095FF);
            t.accent_text = hex(0x0060C9);
            t.on_accent = hex(0xFFFFFF);
            // iOS 系统状态色。
            t.danger.bg = hex(0xFFECEB);
            t.danger.border = hex(0xFFB3AE);
            t.danger.solid = hex(0xFF3B30);
            t.danger.solid_hover = hex(0xE0342B);
            t.danger.solid_active = hex(0xC22E26);
            t.danger.text = hex(0xD70015);
            t.warning.bg = hex(0xFFF4E5);
            t.warning.border = hex(0xFFD9A3);
            t.warning.solid = hex(0xFF9500);
            t.warning.solid_hover = hex(0xF08C00);
            t.warning.solid_active = hex(0xD97F00);
            t.warning.text = hex(0xB25000);
            t.success.bg = hex(0xE9F8EE);
            t.success.border = hex(0xA9E3BC);
            t.success.solid = hex(0x34C759);
            t.success.solid_hover = hex(0x2EB350);
            t.success.solid_active = hex(0x289A46);
            t.success.text = hex(0x1F7A38);
        }
        ThemeMode::Dark => {
            // 深空灰阶：近黑底、逐级抬升的灰面板、冷白文字。
            t.neutrals = Ramp([
                hex(0x1B1B1D), // N1  页面底（深空灰）
                hex(0x242426), // N2  表头等次级底
                hex(0x2C2C2E), // N3  控件底（iOS dark secondary）
                hex(0x3A3A3C), // N4  控件 hover（iOS systemGray5 dark）
                hex(0x48484A), // N5  发丝线（iOS systemGray4 dark）
                hex(0x545456), // N6  描边
                hex(0x636366), // N7  强描边（iOS systemGray3 dark）
                hex(0x8E8E93), // N8  禁用文字（iOS systemGray）
                hex(0x98989D), // N9  次要文字
                hex(0xAEAEB2), // N10 弱化正文（iOS systemGray2）
                hex(0xD1D1D6), // N11 强调正文
                hex(0xF5F5F7), // N12 主文字（冷白）
            ]);
            t.bg = hex(0x1B1B1D);
            t.surface = hex(0x242426);
            t.surface_raised = hex(0x2C2C2E);
            t.element = hex(0x2C2C2E);
            t.element_hover = hex(0x3A3A3C);
            t.element_active = hex(0x48484A);
            t.border_subtle = hex(0x38383A);
            t.border = hex(0x48484A);
            t.border_strong = hex(0x636366);
            t.text = hex(0xF5F5F7);
            t.text_muted = hex(0xAEAEB2);
            t.text_subtle = hex(0x98989D);
            t.text_disabled = hex(0x636366);
            // 暗色 System Blue（更亮以保证对比度）。
            t.accent = hex(0x0A84FF);
            t.accent_hover = hex(0x409CFF);
            t.accent_active = hex(0x007AFF);
            t.accent_bg = hex(0x0F2A4A);
            t.accent_border = hex(0x0A84FF);
            t.accent_text = hex(0x409CFF);
            t.on_accent = hex(0xFFFFFF);
            // iOS 暗色状态色。
            t.danger.bg = hex(0x3B2422);
            t.danger.border = hex(0x7A3532);
            t.danger.solid = hex(0xFF453A);
            t.danger.solid_hover = hex(0xFF5A50);
            t.danger.solid_active = hex(0xE03A30);
            t.danger.text = hex(0xFF6961);
            t.warning.bg = hex(0x3A2E1C);
            t.warning.border = hex(0x8A6116);
            t.warning.solid = hex(0xFF9F0A);
            t.warning.solid_hover = hex(0xFFAA1F);
            t.warning.solid_active = hex(0xE08E09);
            t.warning.text = hex(0xFFB340);
            t.success.bg = hex(0x1D3626);
            t.success.border = hex(0x2C6B40);
            t.success.solid = hex(0x30D158);
            t.success.solid_hover = hex(0x45DE6B);
            t.success.solid_active = hex(0x2AB94E);
            t.success.text = hex(0x32D74B);
        }
    }
    t
}

/// Apple 卡片：实心面板 + 1px 发丝线 + 细腻分层投影（App Store 卡片语言），
/// squircle 连续圆角由主题 `corner_smoothing` 保证。`shrink0` 关键：根滚动
/// 容器是固定高度 flex 列，内容超出视口时未禁缩的卡片会被 flex-shrink
/// 压缩（children 溢出并被 overflow_hidden 裁切），必须让卡片保持固有
/// 高度、交给 scroll_y 滚动。
fn apple_card<Msg: 'static>() -> Element<Msg> {
    col()
        .p(SP6)
        .gap(SP3)
        .shrink0()
        .surface(Surface::Card)
        .overflow_hidden()
        .transition(apple_transition())
}

/// Apple 主按钮：System Blue 实色胶囊 + 顶部高光（apple.com 的 CTA 质感）。
fn apple_primary_style() -> impl Fn(&Theme, Style) -> Style {
    |t: &Theme, s| s.rounded_full().highlight_top(t.on_accent.with_alpha(0.18))
}

/// Apple 次级按钮：中性灰胶囊、无边框（iOS 灰底按钮质感），
/// hover/press 由 kit state_layer 平滑叠加。
fn apple_secondary_style() -> impl Fn(&Theme, Style) -> Style {
    |t: &Theme, s| s.rounded_full().bg(t.element).border(0.0, t.border_subtle)
}

/// Apple 分组标题：蓝色小线性图标 + Semibold 标题（iOS 设置分组风格）。
fn section_title<Msg: 'static>(icon: Element<Msg>, label: String) -> Element<Msg> {
    row().gap(SP2).items_center().children([
        icon.w(16.0)
            .h(16.0)
            .themed(|t: &Theme, s| s.color(t.accent)),
        text(label).weight(Weight::Semibold),
    ])
}

// ---------------------------------------------------------------------------
// 消息与状态
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub enum Msg {
    EditInput(String),
    EditCookie(String),
    EditJfServer(String),
    EditJfToken(String),
    EditDays(String),
    SourceChanged(usize),
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

/// 视频来源：决定 `fetch_and_parse` 走 B 站还是 Jellyfin 适配器。
///
/// UI 通过 segmented 切换；后台线程构造本枚举传给 `fetch_and_parse`，
/// 实现一处入口、两条适配器路径。
#[derive(Debug, Clone)]
pub enum FetchSource {
    /// B 站：可选 Cookie（SESSDATA），用于风控时匿名→登录升级。
    Bilibili { cookie: Option<String> },
    /// Jellyfin：服务器地址 + API Token（必填）。
    Jellyfin { server_url: String, token: String },
}

/// UI 来源 segmented 的状态枚举（与 `plan::Mode` 同样模式：`from_index`/`index`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceMode {
    #[default]
    Bilibili,
    Jellyfin,
}

impl SourceMode {
    pub fn from_index(i: usize) -> Self {
        if i == 0 {
            Self::Bilibili
        } else {
            Self::Jellyfin
        }
    }
    pub fn index(self) -> usize {
        match self {
            Self::Bilibili => 0,
            Self::Jellyfin => 1,
        }
    }
}

/// 持久化到本机的 Jellyfin 凭证（JSON 文件，工作目录旁）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct JellyfinConfig {
    pub server_url: String,
    pub token: String,
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
    pub jf_server: String,
    pub jf_token: String,
    pub source: SourceMode,
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
            jf_server: String::new(),
            jf_token: String::new(),
            source: SourceMode::Bilibili,
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

        // 浮动玻璃工具栏：承载标题与亮/暗切换（macOS 工具栏 vibrancy）。
        let toolbar = self.apple_toolbar();

        // 极淡的装饰色斑层（绝对定位，全屏），位于卡片之下：给浮动工具栏
        // 的 vibrancy 模糊层提供可采样的彩色内容，同时不破坏极简纯色底。
        let atmospheric = self.atmospheric_layer(&theme);

        let mut children: Vec<Element<Msg>> = vec![atmospheric, toolbar];

        // 表单卡片。
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

        // 根容器：纯色 Apple 银灰/深空灰底 + 大留白；animate_layout 让
        // 合集状态切换时各卡片位置以弹簧曲线平滑过渡。
        col()
            .w_full()
            .h_full()
            .bg(theme.bg)
            .px(SP8)
            .py(SP6)
            .gap(SP5)
            .scroll_y()
            .animate_layout()
            .children(children)
    }

    /// Apple 风格浮动工具栏：玻璃 vibrancy 面板承载 Semibold 标题与
    /// 线性图标亮/暗切换（SF Symbols 风格的 sun/moon）。
    fn apple_toolbar(&self) -> Element<Msg> {
        let (glyph, name) = if self.dark {
            (icons::lucide::sun(), "亮色")
        } else {
            (icons::lucide::moon(), "暗色")
        };
        let theme_toggle = Element::from(icon_button(glyph).label(name).on_click(Msg::DarkToggled));

        row()
            .w_full()
            .items_center()
            .justify_between()
            .px(SP5)
            .py(SP3)
            .rounded(R_LG)
            .shrink0()
            .surface(Surface::Glass)
            .overflow_hidden()
            .transition(apple_transition())
            .children([
                text("Bilibili 合集观看计划")
                    .size(TextSize::Lg)
                    .weight(Weight::Semibold),
                theme_toggle,
            ])
    }

    /// 极淡的装饰色斑：两块 Apple 蓝系圆形色斑，alpha 压到几乎不可见，
    /// 只为浮动工具栏的玻璃模糊层提供细微色彩变化（纯色底下 vibrancy
    /// 退化为 tint，仍然成立，但略带层次更接近 macOS 桌面观感）。
    fn atmospheric_layer(&self, theme: &Theme) -> Element<Msg> {
        let dark = matches!(theme.mode, ThemeMode::Dark);
        let (a_strong, a_soft) = if dark { (0.22, 0.16) } else { (0.14, 0.10) };
        // 左上：System Blue 主斑
        let blob_a = div()
            .absolute()
            .top(-200.0)
            .left(-160.0)
            .w(560.0)
            .h(560.0)
            .rounded_full()
            .themed(move |t: &Theme, s| s.bg(t.accents.step(9).with_alpha(a_strong)));
        // 右下：浅蓝呼应斑
        let blob_b = div()
            .absolute()
            .bottom(-240.0)
            .right(-200.0)
            .w(640.0)
            .h(640.0)
            .rounded_full()
            .themed(move |t: &Theme, s| s.bg(t.accents.step(7).with_alpha(a_soft)));

        div()
            .absolute()
            .top(0.0)
            .left(0.0)
            .w_full()
            .h_full()
            .overflow_hidden()
            .children([blob_a, blob_b])
    }

    fn form_card(&self) -> Element<Msg> {
        let loading = self.loading();

        // 顶部「来源」segmented：决定后续字段集与 fetch 走哪条适配器路径。
        let source_field = field("来源").child(segmented(
            self.source.index(),
            ["B 站", "Jellyfin"],
            Msg::SourceChanged,
        ));

        // 链接字段：placeholder/help 随来源切换——避免误粘贴场景的提示错位。
        let (link_placeholder, link_label, link_help) = match self.source {
            SourceMode::Bilibili => (
                "https://www.bilibili.com/video/BV1ps4y1d73V 或 BV 号 或 sid=6789",
                "链接 / BV 号 / 合集 sid",
                "支持 https://www.bilibili.com/video/BVxxxx、BV 号或合集 sid=xxxx 链接",
            ),
            SourceMode::Jellyfin => (
                "https://host/web/#!/details?id=xxx 或直接粘贴 item ID",
                "Jellyfin 链接 / item ID",
                "粘贴 Jellyfin 网页详情页链接（取 ?id= 后部分）或直接 item ID；首次填写服务器/Token 后会自动保存到本机",
            ),
        };
        let link_input = Element::from(
            text_input(&self.input)
                .placeholder(link_placeholder)
                .on_input(Msg::EditInput)
                .id("input"),
        )
        .w_full();

        let days_input = Element::from(
            text_input(&self.days_text)
                .placeholder("如 30")
                .width(120.0)
                .on_input(Msg::EditDays)
                .id("days"),
        );

        // 来源相关字段组：B 站 → Cookie；Jellyfin → 服务器地址 + Token。
        let source_specific: Vec<Element<Msg>> = match self.source {
            SourceMode::Bilibili => {
                let cookie_input = Element::from(
                    text_input(&self.cookie)
                        .placeholder("SESSDATA=xxx")
                        .on_input(Msg::EditCookie)
                        .id("cookie"),
                )
                .w_full();
                vec![Element::from(
                    field("Cookie（可选，风控时使用）")
                        .help("例如 SESSDATA=xxx；留空则匿名请求")
                        .child(cookie_input),
                )]
            }
            SourceMode::Jellyfin => {
                let server_input = Element::from(
                    text_input(&self.jf_server)
                        .placeholder("https://media.example.com:8096")
                        .on_input(Msg::EditJfServer)
                        .id("jf_server"),
                )
                .w_full();
                let token_input = Element::from(
                    text_input(&self.jf_token)
                        .placeholder("API Token（Jellyfin 后台「控制台 → 高级 → API 密钥」生成）")
                        .on_input(Msg::EditJfToken)
                        .id("jf_token"),
                )
                .w_full();
                vec![
                    Element::from(
                        field("Jellyfin 服务器地址")
                            .help("形如 https://media.example.com:8096，无尾斜杠亦可")
                            .child(server_input),
                    ),
                    Element::from(
                        field("Jellyfin API Token")
                            .help("在 Jellyfin 后台「控制台 → 高级 → API 密钥」生成；获取成功后会自动保存")
                            .child(token_input),
                    ),
                ]
            }
        };

        // 主按钮：System Blue 实色胶囊 + 顶部高光（apple.com CTA）。
        let primary_btn = Element::from(
            button("获取视频信息")
                .on_click(Msg::Fetch)
                .disabled(loading),
        )
        .themed(apple_primary_style());

        let days_field = field("目标观看天数").child(days_input);
        let mode_field = field("计划模式").child(segmented(
            self.mode.index(),
            ["split 精确切分", "whole 完整不拆"],
            Msg::ModeChanged,
        ));

        // 提示文字：按来源切换，给到用户当前模式的常见故障提示。
        let hint_text: Element<Msg> = if loading {
            text("获取中…请稍候").size(TextSize::Sm)
        } else {
            match self.source {
                SourceMode::Bilibili => {
                    text("提示：B 站接口可能触发风控，失败时可添加 Cookie 重试")
                        .size(TextSize::Sm)
                        .themed(|t: &Theme, s| s.color(t.text_muted))
                }
                SourceMode::Jellyfin => {
                    text("提示：若拉取失败，请确认 Token 有效且 Jellyfin 可访问")
                        .size(TextSize::Sm)
                        .themed(|t: &Theme, s| s.color(t.text_muted))
                }
            }
        };

        // 动态拼装表单卡片子项：来源 segmented → 链接 → (days + mode) → 来源相关字段 → 主按钮行。
        let mut children: Vec<Element<Msg>> = Vec::new();
        children.push(Element::from(source_field));
        children.push(Element::from(
            field(link_label).help(link_help).child(link_input),
        ));
        children.push(
            row()
                .gap(SP4)
                .items_end()
                .children([Element::from(days_field), Element::from(mode_field)]),
        );
        children.extend(source_specific);
        children.push(
            row()
                .gap(SP3)
                .items_center()
                .children([primary_btn, hint_text]),
        );

        apple_card().children(children)
    }

    fn error_callout(&self, err: &str) -> Element<Msg> {
        // Apple 风格错误横幅：iOS 系统红浅底 + 状态色文字与图标，
        // squircle 圆角由主题统一。
        callout(Status::Danger, err.to_string())
            .shrink0()
            .overflow_hidden()
    }

    fn loading_row(&self) -> Element<Msg> {
        // Apple 活动指示条：实心卡片胶囊 + spinner。
        row()
            .gap(SP2)
            .items_center()
            .shrink0()
            .surface(Surface::Card)
            .overflow_hidden()
            .px(SP4)
            .py(SP3)
            .rounded_full()
            .children([spinner(), text("正在获取视频信息…")])
    }

    fn ready_children(&self, rd: &ReadyState) -> Vec<Element<Msg>> {
        let mut out: Vec<Element<Msg>> = Vec::new();

        // 合集信息卡片：Apple 分组标题 + 次级文字统计行。
        let mut info: Vec<Element<Msg>> = vec![
            section_title(
                icons::lucide::info(),
                format!("合集：《{}》", rd.season_title),
            ),
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
        out.push(apple_card().children(info));

        // 多科目选择
        if rd.groups.len() > 1 {
            let mut sel: Vec<Element<Msg>> = vec![
                section_title(icons::lucide::filter(), "科目选择（统计范围）".to_string()),
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
            out.push(apple_card().children(sel));
        }

        // 操作按钮：主操作（生成）为 System Blue 实色胶囊，次操作（导出）
        // 为中性灰胶囊——apple.com 的 CTA 层级。
        let has_plan = rd.plan.is_some();
        let generate_btn = Element::from(button("生成观看计划").on_click(Msg::Generate))
            .themed(apple_primary_style());
        let export_btn = Element::from(
            button("导出计划文本（UTF-8）")
                .variant(ButtonVariant::Secondary)
                .on_click(Msg::Export)
                .disabled(!has_plan),
        )
        .themed(apple_secondary_style());
        out.push(
            row()
                .gap(SP3)
                .shrink0()
                .children([generate_btn, export_btn]),
        );

        // 计划表格：Apple 卡片包裹，发丝线分隔行。
        if let Some(p) = &rd.plan {
            out.push(self.plan_table_card(p));
        } else {
            out.push(
                text("填写目标天数后点击「生成观看计划」。")
                    .size(TextSize::Sm)
                    .themed(|t: &Theme, s| s.color(t.text_muted)),
            );
        }

        out
    }

    /// kit `data_table` 的自动虚拟化行数阈值（与 fenestra-kit
    /// `AUTO_SCROLL_ROWS` 保持一致）：超过该阈值表体切换为内部滚动 +
    /// 虚拟化，每帧只构建/绘制可视窗口内的行（O(1)），长计划滑动不掉帧。
    const PLAN_TABLE_VIRTUAL_ROWS: usize = 50;
    /// 虚拟化计划表的卡片高度（logical px）：约 15 行 + 表头 + 留白。
    const PLAN_TABLE_HEIGHT: f32 = 560.0;

    fn plan_table_card(&self, p: &PlanData) -> Element<Msg> {
        let mut rows: Vec<Vec<String>> = Vec::new();
        let mut cumulative: i64 = 0;
        for (di, entries) in p.plan.iter().enumerate() {
            let day_total: i64 = entries.iter().map(|e| e.portion).sum();
            cumulative += day_total;
            let remaining = p.total - cumulative;
            // 每日汇总拆到「标题 / 备注」两列：kit data_table 行高固定，
            // 单列长文本会换行溢出与下行重叠；拆分后每行均单行显示，
            // 符合 Apple 表格的整洁单行排版。
            let day_head = format!(
                "【第 {} 天】目标 {} ｜ 累计 {}",
                di + 1,
                fmt_seconds(p.capacities[di] as f64, true),
                fmt_seconds(day_total as f64, true),
            );
            let day_note = format!(
                "进度 {:.1}% ｜ 剩余总时长 {}",
                cumulative as f64 / p.total as f64 * 100.0,
                fmt_seconds(remaining as f64, true),
            );
            if entries.is_empty() {
                rows.push(vec![
                    (di + 1).to_string(),
                    String::new(),
                    "（本日无安排 / 休息）".to_string(),
                    String::new(),
                    day_note,
                ]);
                continue;
            }
            rows.push(vec![
                (di + 1).to_string(),
                String::new(),
                day_head,
                String::new(),
                day_note,
            ]);
            for e in entries {
                rows.push(vec![
                    String::new(),
                    format!("#{}", e.vid_no),
                    trunc(&e.title, 22),
                    fmt_seconds(e.portion as f64, true),
                    trunc(&note_for(e, di), 28),
                ]);
            }
        }

        let n_rows = rows.len();
        let table = Element::from(
            data_table(["天", "视频#", "标题", "本日时长", "备注"], rows)
                .id("plan-table")
                .column_widths([64.0, 84.0, 320.0, 116.0, 400.0]),
        );

        // Apple 卡片包住 data_table；面板的 SP1 padding 让表格内容与卡片
        // 边缘留呼吸空间，overflow_hidden 保证内部横线不突破 squircle 圆角。
        //
        // 行数超过 kit 的自动虚拟化阈值时，data_table 切换为虚拟化滚动
        // 表体（内部滚动 + 吸顶表头）。此前用 `sticky_header(false)` 强制
        // inline 整表随页面滚动——每帧都要重建/布局/绘制全部 70+ 行，
        // 长计划滑动明显掉帧。虚拟化后每帧只处理可视窗口内的行（O(1)），
        // 滑动恢复极致丝滑。
        //
        // 代价：虚拟化表体用 `h_full` 填满父高，必须有一个确定高度，
        // 否则在页面级滚动流里会塌缩为视口高度（2026-08-03 的"第 4 天起
        // 行消失"回归）。这里只对超过阈值的表给卡片固定高度；小表仍 inline
        // 随页面滚动，保持原有的整页滚动体验。
        let mut card = col()
            .p(SP1)
            .rounded(R_LG)
            .shrink0()
            .surface(Surface::Card)
            .overflow_hidden();
        if n_rows > Self::PLAN_TABLE_VIRTUAL_ROWS {
            card = card.h(Self::PLAN_TABLE_HEIGHT);
        }
        card.child(table)
    }
}

// ---------------------------------------------------------------------------
// 业务逻辑（无 GUI 依赖）
// ---------------------------------------------------------------------------

/// 后台线程执行：按来源分派获取视频/合集信息并识别结构。
///
/// - `FetchSource::Bilibili`：sid → 归档接口；否则 BV → view API → parse_groups
/// - `FetchSource::Jellyfin`：`JellyfinClient` → `jellyfin::fetch_groups`
///
/// 两条路径共用 `ReadyState` 构造与默认选择策略（多科目→All，单科目→Single(0)），
/// 后续计划生成、表格、导出与来源无关。
pub fn fetch_and_parse(input: &str, source: &FetchSource) -> Result<ReadyState, String> {
    let r = (|| -> crate::Result<ReadyState> {
        let (season_title, groups, structure) = match source {
            FetchSource::Bilibili { cookie } => {
                let cookie = cookie.as_deref();
                if let Some(sid) = extract_sid(input) {
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
                }
            }
            FetchSource::Jellyfin { server_url, token } => {
                if server_url.trim().is_empty() {
                    return Err(Error::input("请填写 Jellyfin 服务器地址。"));
                }
                if token.trim().is_empty() {
                    return Err(Error::input("请填写 Jellyfin API Token。"));
                }
                let client = crate::jellyfin::JellyfinClient::new(
                    server_url.trim().to_string(),
                    token.trim().to_string(),
                );
                crate::jellyfin::fetch_groups(&client, input)?
            }
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
// Jellyfin 凭证持久化（0 新依赖：家目录 + serde_json）
// ---------------------------------------------------------------------------

/// 配置文件路径：用户家目录下 `.bili-planner.json`（比工作目录稳定，
/// 不同启动目录都能读到）。Windows 取 `%USERPROFILE%`，Unix 取 `$HOME`。
fn config_path() -> Option<PathBuf> {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let home = std::env::var(key).ok()?;
    Some(PathBuf::from(home).join(".bili-planner.json"))
}

/// 启动时尝试加载本机 Jellyfin 凭证；文件不存在或损坏时静默返回 `None`。
fn load_config() -> Option<JellyfinConfig> {
    let path = config_path()?;
    let data = std::fs::read_to_string(&path).ok()?;
    let cfg: JellyfinConfig = serde_json::from_str(&data).ok()?;
    if cfg.server_url.trim().is_empty() || cfg.token.trim().is_empty() {
        return None;
    }
    Some(cfg)
}

/// 把 Jellyfin 凭证写到本机（pretty JSON）。失败静默：不阻塞主流程。
fn save_config(cfg: &JellyfinConfig) {
    let Some(path) = config_path() else { return };
    if let Ok(json) = serde_json::to_string_pretty(cfg) {
        let _ = std::fs::write(&path, json);
    }
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
        // 启动时尝试加载本机 Jellyfin 凭证，预热字段——UI 上无需重新填写。
        if let Some(cfg) = load_config() {
            self.jf_server = cfg.server_url;
            self.jf_token = cfg.token;
        }
    }

    fn update(&mut self, msg: Msg) {
        match msg {
            Msg::EditInput(s) => {
                self.input = s;
                self.last_error = None;
            }
            Msg::EditCookie(s) => self.cookie = s,
            Msg::EditJfServer(s) => self.jf_server = s,
            Msg::EditJfToken(s) => self.jf_token = s,
            Msg::EditDays(s) => {
                self.days_text = s;
                // 天数变化后，旧计划可能不再有效
                if let Phase::Ready(rd) = &mut self.phase {
                    rd.plan = None;
                }
            }
            Msg::SourceChanged(i) => {
                self.source = SourceMode::from_index(i);
                self.last_error = None;
                // 来源切换：已识别的合集结构不再适用，丢弃旧的 Ready 状态。
                if let Phase::Ready(_) = &self.phase {
                    self.phase = Phase::Input;
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
                let source = match self.source {
                    SourceMode::Bilibili => FetchSource::Bilibili { cookie },
                    SourceMode::Jellyfin => {
                        // 前端先做最小校验，避免起线程后才报错；后端会再 trim 检查。
                        if self.jf_server.trim().is_empty() || self.jf_token.trim().is_empty() {
                            self.phase = Phase::Input;
                            self.toast("请填写 Jellyfin 服务器地址与 API Token。", Status::Warning);
                            return;
                        }
                        FetchSource::Jellyfin {
                            server_url: self.jf_server.clone(),
                            token: self.jf_token.clone(),
                        }
                    }
                };
                if let Some(proxy) = self.proxy.clone() {
                    std::thread::spawn(move || {
                        proxy.send(Msg::Fetched(fetch_and_parse(&input, &source)));
                    });
                }
            }
            Msg::Fetched(Ok(mut rd)) => {
                // Jellyfin 来源且本次成功：默认写盘记住凭证，下次启动免重填。
                if matches!(self.source, SourceMode::Jellyfin) {
                    save_config(&JellyfinConfig {
                        server_url: self.jf_server.trim().to_string(),
                        token: self.jf_token.trim().to_string(),
                    });
                }
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
        // Apple HIG 主题：System Blue 强调 + 深空灰中性色 + squircle 连续
        // 圆角（0.6 corner smoothing）；亮色银灰底 / 暗色深空灰底。
        let mode = if self.dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        };
        apple_theme(mode)
    }

    fn view(&self) -> Element<Msg> {
        PlannerApp::view(self)
    }
}
