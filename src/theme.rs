//! 主题定制：以内置默认亮/暗主题为底，把主色系覆写为 Apple System Blue。
//!
//! gpui-component 的主题由 [`ThemeConfig`]（语义色令牌）驱动，`Theme::change`
//! 在每次亮/暗切换时重新套用 `light_theme`/`dark_theme` 两份配置。因此把
//! System Blue 写进这两份配置（而非直接改运行时色值），切换后覆写仍然生效。
//!
//! 色值沿用旧版 `apple_theme` 提炼的 Apple HIG 色板：
//! - 亮色主色 `#007AFF`（apple.com 按钮梯度 hover `#0071E3` / active `#0062C4`）
//! - 暗色主色 `#0A84FF`（更亮以保证深底对比度）

use std::rc::Rc;

use gpui::App;
use gpui_component::theme::{Theme, ThemeConfig, ThemeRegistry};

/// 亮色模式主色：Apple System Blue。
const BLUE_LIGHT: &str = "#007AFF";
/// 暗色模式主色：Apple System Blue（dark variant）。
const BLUE_DARK: &str = "#0A84FF";

/// 在内置默认主题基础上覆写主色系；未指定的令牌保持内置默认值。
fn system_blue_config(base: &ThemeConfig) -> ThemeConfig {
    let mut config = base.clone();
    let dark = config.mode.is_dark();
    config.name = if dark {
        "Apple Dark".into()
    } else {
        "Apple Light".into()
    };
    config.radius = Some(8);
    config.colors.primary = Some(if dark { BLUE_DARK } else { BLUE_LIGHT }.into());
    config.colors.primary_hover = Some(if dark { "#409CFF" } else { "#0071E3" }.into());
    config.colors.primary_active = Some(if dark { BLUE_LIGHT } else { "#0062C4" }.into());
    config.colors.primary_foreground = Some("#FFFFFF".into());
    // 焦点环、链接与输入选区跟随主色，保持"一处蓝到底"的 Apple 观感。
    config.colors.ring = Some(if dark { "#0A84FF99" } else { "#007AFF66" }.into());
    config.colors.link = Some(if dark { "#409CFF" } else { "#0066CC" }.into());
    config.colors.link_hover = Some(if dark { "#409CFF" } else { "#0071E3" }.into());
    config.colors.link_active = Some(if dark { BLUE_LIGHT } else { "#0062C4" }.into());
    config.colors.selection = Some(if dark { "#0A84FF59" } else { "#007AFF40" }.into());
    config
}

/// 在 `gpui_component::init` 之后调用：替换默认亮/暗主题配置并重新套用，
/// 使 System Blue 成为全局主色。
pub fn init(cx: &mut App) {
    let registry = ThemeRegistry::global(cx);
    let light = system_blue_config(registry.default_light_theme());
    let dark = system_blue_config(registry.default_dark_theme());

    {
        let theme = Theme::global_mut(cx);
        theme.light_theme = Rc::new(light);
        theme.dark_theme = Rc::new(dark);
    }
    // 以当前模式重新套用（含自定义配置），立即生效。
    let mode = Theme::global(cx).mode;
    Theme::change(mode, None, cx);
}
