//! Bilibili & Jellyfin 合集观看计划生成器（跨平台桌面应用入口）。
//!
//! 启动 GPUI 应用 → 初始化 gpui-component 与新野兽派（Neo-Brutalist）主题 →
//! 打开主窗口（`Root` 包裹 [`PlannerApp`]，TitleBar 与系统红绿灯融合）。

use bili_planner::app::PlannerApp;
use bili_planner::{assets::Assets, theme as app_theme};
use gpui::{px, size, AppContext, Application, Bounds, WindowBounds, WindowOptions};
use gpui_component::{Root, TitleBar};

const WINDOW_SIZE: (f32, f32) = (1120.0, 820.0);
const WINDOW_MIN_SIZE: (f32, f32) = (960.0, 660.0);

fn main() {
    Application::new().with_assets(Assets).run(|cx| {
        gpui_component::init(cx);
        app_theme::init(cx);

        let bounds = Bounds::centered(None, size(px(WINDOW_SIZE.0), px(WINDOW_SIZE.1)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitleBar::title_bar_options()),
            window_min_size: Some(size(px(WINDOW_MIN_SIZE.0), px(WINDOW_MIN_SIZE.1))),
            ..Default::default()
        };

        cx.open_window(options, |window, cx| {
            let planner = cx.new(|cx| PlannerApp::new(window, cx));
            // 首层必须是 Root：承载通知/对话框/Sheet 等浮层。
            cx.new(|cx| Root::new(planner, window, cx))
        })
        .expect("Failed to open window");
    });
}
