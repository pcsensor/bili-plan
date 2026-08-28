//! 应用资产源：合并官方组件图标与应用专属图标。
//!
//! gpui 通过全局 `AssetSource` 按 `icons/xxx.svg` 路径取 SVG。组件内部
//! 图标来自官方 `gpui-component-assets` crate；`assets/icons/` 下的应用
//! 专属图标（link/download/refresh-cw 等）优先命中，未命中回退官方集。

use gpui::{AssetSource, Result, SharedString};
use rust_embed::RustEmbed;
use std::borrow::Cow;

#[derive(RustEmbed)]
#[folder = "assets/icons"]
struct AppIcons;

/// 应用全局资产源：`Application::new().with_assets(Assets)` 注册。
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(file) = path.strip_prefix("icons/").and_then(AppIcons::get) {
            return Ok(Some(file.data));
        }
        gpui_component_assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        gpui_component_assets::Assets.list(path)
    }
}
