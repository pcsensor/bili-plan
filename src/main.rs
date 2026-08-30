//! Bilibili & Jellyfin 合集观看计划生成器（跨平台桌面应用入口）。
//!
//! 启动 GPUI 应用 → 初始化 gpui-component 与新野兽派（Neo-Brutalist）主题 →
//! 打开主窗口（`Root` 包裹 [`PlannerApp`]，TitleBar 与系统红绿灯融合）。

use bili_planner::app::PlannerApp;
use bili_planner::{assets::Assets, theme as app_theme};
use gpui::{
    actions, px, size, App, AppContext, Application, Bounds, KeyBinding, Menu, MenuItem, OsAction,
    SystemMenuType, WindowBounds, WindowOptions,
};
use gpui_component::{Root, TitleBar};

const WINDOW_SIZE: (f32, f32) = (1480.0, 960.0);
const WINDOW_MIN_SIZE: (f32, f32) = (1080.0, 720.0);

actions!(
    bili_planner,
    [
        Quit,
        Hide,
        HideOthers,
        ShowAll,
        Minimize,
        Zoom,
        CloseWindow,
        Cut,
        Copy,
        Paste,
        SelectAll,
    ]
);

fn open_main_window(cx: &mut App) {
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
}

fn init_menus(cx: &mut App) {
    #[cfg(target_os = "macos")]
    {
        cx.bind_keys([
            KeyBinding::new("cmd-h", Hide, None),
            KeyBinding::new("cmd-alt-h", HideOthers, None),
            KeyBinding::new("cmd-m", Minimize, None),
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-w", CloseWindow, None),
        ]);

        cx.on_action(|_: &Quit, cx| {
            cx.quit();
        });
        cx.on_action(|_: &Hide, cx| {
            cx.hide();
        });
        cx.on_action(|_: &HideOthers, cx| {
            cx.hide_other_apps();
        });
        cx.on_action(|_: &ShowAll, cx| {
            cx.unhide_other_apps();
        });
        cx.on_action(|_: &Minimize, cx| {
            cx.defer(|cx| {
                if let Some(window) = cx.active_window() {
                    window
                        .update(cx, |_, window, _| {
                            window.minimize_window();
                        })
                        .ok();
                }
            });
        });
        cx.on_action(|_: &Zoom, cx| {
            cx.defer(|cx| {
                if let Some(window) = cx.active_window() {
                    window
                        .update(cx, |_, window, _| {
                            window.zoom_window();
                        })
                        .ok();
                }
            });
        });
        cx.on_action(|_: &CloseWindow, cx| {
            cx.defer(|cx| {
                if let Some(window) = cx.active_window() {
                    window
                        .update(cx, |_, window, _| {
                            window.remove_window();
                        })
                        .ok();
                }
            });
        });

        cx.set_menus(vec![
            Menu {
                name: "bili-planner".into(),
                items: vec![
                    MenuItem::os_submenu("Services", SystemMenuType::Services),
                    MenuItem::separator(),
                    MenuItem::action("Hide bili-planner", Hide),
                    MenuItem::action("Hide Others", HideOthers),
                    MenuItem::action("Show All", ShowAll),
                    MenuItem::separator(),
                    MenuItem::action("Quit bili-planner", Quit),
                ],
            },
            Menu {
                name: "Edit".into(),
                items: vec![
                    MenuItem::os_action("Cut", Cut, OsAction::Cut),
                    MenuItem::os_action("Copy", Copy, OsAction::Copy),
                    MenuItem::os_action("Paste", Paste, OsAction::Paste),
                    MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
                ],
            },
            Menu {
                name: "Window".into(),
                items: vec![
                    MenuItem::action("Minimize", Minimize),
                    MenuItem::action("Zoom", Zoom),
                    MenuItem::separator(),
                    MenuItem::action("Close Window", CloseWindow),
                ],
            },
        ]);
    }
}

fn main() {
    let app = Application::new().with_assets(Assets);

    app.on_reopen(|cx| {
        if cx.windows().is_empty() {
            open_main_window(cx);
        }
        cx.activate(true);
    });

    app.run(|cx| {
        gpui_component::init(cx);
        app_theme::init(cx);
        init_menus(cx);

        open_main_window(cx);
    });
}
