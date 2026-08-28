//! Neo-Brutalist（新野兽风）主题：高饱和原色、纯黑硬边框、直角、无渐变阴影。
//!
//! 设计令牌：
//! - 亮色：纸面 `#FFFDF5`，墨色 `#0A0A0A`，主色电光蓝 `#2F6BFF`
//! - 暗色：墨底 `#101014`，纸墨 `#F5F1E8`，主色荧光黄 `#FFE600`
//! - 强调色：红 `#FF4911` / 绿 `#00B86B` / 粉 `#FF6FD8` / 黄 `#FFD400`
//! - 几何：radius = 0（直角），边框 2px 纯黑/纸白，硬阴影用纯偏移（blur=0）模拟
//!
//! 亮/暗两份 `ThemeConfig` 均完整覆写语义色，`Theme::change` 切换后仍生效。

use std::rc::Rc;

use gpui::App;
use gpui_component::theme::{Theme, ThemeConfig, ThemeRegistry};

// ---------------------------------------------------------------------------
// 设计令牌
// ---------------------------------------------------------------------------

/// 墨色（亮模式前景/边框，暗模式背景基调）。
pub const INK: &str = "#0A0A0A";
/// 纸色（亮模式背景）。
pub const PAPER: &str = "#FFFDF5";
/// 暗色纸墨（暗模式前景/边框）。
pub const PAPER_INK: &str = "#F5F1E8";
/// 暗色底。
pub const DARK_BG: &str = "#101014";

/// 电光蓝（亮模式主色）。
pub const BLUE: &str = "#2F6BFF";
/// 荧光黄（暗模式主色）。
pub const ACID_YELLOW: &str = "#FFE600";
/// 信号红。
pub const RED: &str = "#FF4911";
/// 翠绿。
pub const GREEN: &str = "#00B86B";
/// 亮粉。
pub const PINK: &str = "#FF6FD8";
/// 明黄（亮模式强调）。
pub const YELLOW: &str = "#FFD400";

/// 亮色卡片底（比纸面更纯的白，突出层级）。
const LIGHT_CARD: &str = "#FFFFFF";
/// 亮色次级底（浅灰纸）。
const LIGHT_MUTED: &str = "#F1EDE0";
/// 暗色卡片底（比底色略亮）。
const DARK_CARD: &str = "#1B1B22";
/// 暗色次级底。
const DARK_MUTED: &str = "#26262E";

// ---------------------------------------------------------------------------
// ThemeConfig 覆写
// ---------------------------------------------------------------------------

fn brutal_config(base: &ThemeConfig) -> ThemeConfig {
    let mut c = base.clone();
    let dark = c.mode.is_dark();

    c.name = if dark {
        "Brutalist Dark".into()
    } else {
        "Brutalist Light".into()
    };
    // 直角是 neo-brutalism 的标志性语言。
    c.radius = Some(0);
    c.radius_lg = Some(0);
    c.shadow = Some(false);

    let (fg, bg, card, muted, inv) = if dark {
        (PAPER_INK, DARK_BG, DARK_CARD, DARK_MUTED, INK)
    } else {
        (INK, PAPER, LIGHT_CARD, LIGHT_MUTED, PAPER)
    };
    let primary = if dark { ACID_YELLOW } else { BLUE };

    let colors = &mut c.colors;
    colors.background = Some(bg.into());
    colors.foreground = Some(fg.into());
    colors.border = Some(fg.into());
    colors.popover = Some(card.into());
    colors.popover_foreground = Some(fg.into());
    colors.muted = Some(muted.into());
    colors.muted_foreground = Some(if dark { "#A8A394" } else { "#6B6656" }.into());
    colors.secondary = Some(muted.into());
    colors.secondary_foreground = Some(fg.into());
    colors.secondary_hover = Some(if dark { "#32323C" } else { "#E5E0D0" }.into());
    colors.secondary_active = Some(if dark { "#3E3E4A" } else { "#DAD4C2" }.into());
    colors.accent = Some(if dark { DARK_MUTED } else { YELLOW }.into());
    colors.accent_foreground = Some(INK.into());
    // 主按钮：亮模式电光蓝白字；暗模式荧光黄黑字。
    colors.primary = Some(primary.into());
    colors.primary_hover = Some(if dark { "#FFEB3B" } else { "#1E56E8" }.into());
    colors.primary_active = Some(if dark { "#E6CF00" } else { "#1245C4" }.into());
    colors.primary_foreground = Some(inv.into());
    // 输入边框、焦点环、选区全部跟随后现代墨色/主色。
    colors.input = Some(fg.into());
    colors.ring = Some(primary.into());
    colors.caret = Some(primary.into());
    colors.selection = Some(if dark { "#FFE60066" } else { "#2F6BFF33" }.into());
    colors.link = Some(if dark { "#8FB0FF" } else { BLUE }.into());
    colors.link_hover = Some(if dark { "#B9CFFF" } else { "#1E56E8" }.into());
    colors.link_active = Some(if dark { "#6E97F0" } else { "#1245C4" }.into());
    // 列表 / 表格。
    colors.list = Some(card.into());
    colors.list_even = Some(if dark { "#17171D" } else { "#FAF7EE" }.into());
    colors.list_head = Some(if dark { "#26262E" } else { YELLOW }.into());
    colors.table_head_foreground = Some(INK.into());
    colors.list_hover = Some(if dark { "#2C2C36" } else { "#FFF3B8" }.into());
    colors.list_active = Some(if dark { "#3A3A00" } else { "#DBE6FF" }.into());
    colors.list_active_border = Some(primary.into());
    // 语义色。
    colors.danger = Some(RED.into());
    colors.danger_hover = Some("#E63E0C".into());
    colors.danger_active = Some("#C93408".into());
    colors.danger_foreground = Some("#FFFFFF".into());
    colors.info = Some(BLUE.into());
    colors.info_hover = Some("#1E56E8".into());
    colors.info_active = Some("#1245C4".into());
    colors.info_foreground = Some("#FFFFFF".into());
    colors.success = Some(GREEN.into());
    colors.success_foreground = Some("#FFFFFF".into());
    colors.warning = Some(if dark { ACID_YELLOW } else { YELLOW }.into());
    colors.warning_foreground = Some(INK.into());
    // 图表色板：原色系。
    colors.chart_1 = Some(BLUE.into());
    colors.chart_2 = Some(RED.into());
    colors.chart_3 = Some(GREEN.into());
    colors.chart_4 = Some(PINK.into());
    colors.chart_5 = Some(if dark { ACID_YELLOW } else { YELLOW }.into());
    // 分组框 / 侧栏等容器。
    colors.group_box = Some(card.into());
    colors.group_box_foreground = Some(fg.into());
    colors.sidebar = Some(card.into());
    colors.sidebar_foreground = Some(fg.into());
    colors.sidebar_border = Some(fg.into());
    colors.sidebar_accent = Some(muted.into());
    colors.sidebar_accent_foreground = Some(fg.into());
    colors.accordion = Some(card.into());
    colors.accordion_hover = Some(if dark { "#2C2C36" } else { "#FFF3B8" }.into());
    colors.progress_bar = Some(primary.into());
    colors.description_list_label = Some(muted.into());
    colors.description_list_label_foreground = Some(fg.into());
    // 滚动条用墨色块，而非默认的半透明灰。
    colors.scrollbar = Some("#00000000".into());
    colors.scrollbar_thumb = Some(if dark { "#3E3E4A" } else { "#0A0A0A" }.into());
    colors.scrollbar_thumb_hover = Some(primary.into());
    colors.drag_border = Some(primary.into());
    colors.drop_target = Some(if dark { "#3A3A00" } else { "#DBE6FF" }.into());
    c
}

/// 在 `gpui_component::init` 之后调用：替换默认亮/暗主题配置并重新套用。
pub fn init(cx: &mut App) {
    let registry = ThemeRegistry::global(cx);
    let light = brutal_config(registry.default_light_theme());
    let dark = brutal_config(registry.default_dark_theme());

    {
        let theme = Theme::global_mut(cx);
        theme.light_theme = Rc::new(light);
        theme.dark_theme = Rc::new(dark);
    }
    let mode = Theme::global(cx).mode;
    Theme::change(mode, None, cx);
}
