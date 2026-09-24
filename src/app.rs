//! gpui-component 桌面应用：状态机、视图与交互（Neo-Brutalist 视觉）。
//!
//! 业务编排全部在 [`crate::core`]（无 GUI 依赖）；本模块只负责把状态
//! 渲染为组件树，并在事件回调中驱动 core 的纯函数。
//!
//! ## 视觉语言（新野兽风）
//!
//! - 2px 墨色硬边框 + 纯偏移「硬阴影」（blur=0，见 [`hard_shadow`]）；
//! - 直角、原色大标题、区块用黄色斜头条带分隔；
//! - 计划表用列分隔线 + 日汇总行黄色高亮，替代默认细线表格。
//!
//! ## 视图结构
//!
//! - `TitleBar`：应用名 + 亮/暗切换（macOS 与系统红绿灯融合）。
//! - Hero 区：超大标题 + 关键词高亮。
//! - 「来源」卡：来源切换、链接输入、凭证字段、天数与模式、获取按钮。
//! - 「结果」区：合集信息卡、科目选择、操作行、计划表。
//! - 反馈走 `Notification`（自动消失）。

use std::path::PathBuf;

use gpui::{
    canvas, div, fill, hsla, point, prelude::*, px, size, AnimationExt, App, BoxShadow, Context,
    Entity, Focusable, FontWeight, InteractiveElement, IntoElement, MouseButton, Render, Styled,
    Window,
};
use gpui_component::{
    alert::Alert,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputEvent, InputState},
    label::Label,
    notification::Notification,
    resizable::{h_resizable, resizable_panel},
    table::{Column, Table, TableDelegate, TableState},
    v_flex, ActiveTheme, Disableable, Icon, IconName, Root, Selectable, Sizable, Theme, ThemeMode,
    TitleBar, WindowExt,
};

use crate::core::{
    add_custom_study_plan, add_daily_note, add_one_off_calendar_task,
    advance_completed_study_tasks, append_calendar_series_task, check_cloud_bind_status,
    checkin_study_task, clear_history, create_calendar_series_plan, delete_calendar_task,
    delete_daily_note, enroll_study_plan, export_payload, generate_plan, get_daily_notes,
    load_config, parse_days, push_forward_study_plan, record_history, remove_history,
    remove_study_plan, request_cloud_bind_code, reschedule_unfinished_study_plan, save_config,
    sync_with_cloud, toggle_study_plan_status, update_calendar_task, AppConfig, FetchSource,
    ReadyState, Selection, SourceMode,
};
use crate::plan::{fmt_human, fmt_seconds, Mode, PlanEntry};
use crate::study::{
    compute_month_study_stats, compute_plan_progress, compute_study_stats, format_date,
    generate_month_calendar_matrix, get_tasks_for_date, infer_plan_start_date, parse_date_or_today,
    today_date_str, PlanStatus, StudyPlan,
};
use planner_domain::source::{video_link, SourceKind};

/// 顶部活动标签页。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AppTab {
    #[default]
    TodayCheckIn,
    Calendar,
    PlanGenerator,
    MyPlans,
}

/// 从日历新增任务的归属。系列计划会在计划库中显示并可持续追加。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum CalendarTaskTarget {
    #[default]
    OneOff,
    NewSeries,
    ExistingSeries,
}

/// 从右侧任务列表传入编辑弹窗的不可变初始值。
#[derive(Clone)]
struct CalendarTaskEditSeed {
    plan_id: String,
    task_id: String,
    title: String,
    date: String,
    portion: i64,
}

/// 来源标记（`StudyPlan.source_type` / 历史记录 `source`）→ 展示用图标与短名。
///
/// 集中一处，避免新增来源时在各处散落的 `if tag == "jellyfin"` 分支漏改。
fn source_badge(tag: &str) -> (&'static str, &'static str) {
    match SourceKind::from_tag(tag) {
        SourceKind::Jellyfin => ("icons/film.svg", "JF"),
        SourceKind::FnOs => ("icons/server.svg", "飞牛"),
        _ => ("icons/tv.svg", "B站"),
    }
}

/// 来源标记 → 来源枚举。未知标记回退 B 站（兼容旧数据）。
fn source_mode_of(tag: &str) -> SourceMode {
    match SourceKind::from_tag(tag) {
        SourceKind::Jellyfin => SourceMode::Jellyfin,
        SourceKind::FnOs => SourceMode::FnOs,
        _ => SourceMode::Bilibili,
    }
}

/// 打开外部视频链接。
fn open_video_link(source_type: &str, source_url: &str, vid_no: i64) {
    if source_url.trim().is_empty() {
        return; // 自定义任务没有外部播放地址。
    }
    let url = video_link(source_type, source_url, vid_no);

    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(&url).spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/c", "start", &url])
        .spawn();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
}

/// 格式化日期为带星期的展示标签。
fn format_date_with_weekday(date_str: &str) -> String {
    use chrono::Datelike;
    let d = parse_date_or_today(date_str);
    let weekday_str = match d.weekday() {
        chrono::Weekday::Mon => "周一",
        chrono::Weekday::Tue => "周二",
        chrono::Weekday::Wed => "周三",
        chrono::Weekday::Thu => "周四",
        chrono::Weekday::Fri => "周五",
        chrono::Weekday::Sat => "周六",
        chrono::Weekday::Sun => "周日",
    };
    let is_today = date_str == today_date_str();
    if is_today {
        format!("{date_str} 今日（{weekday_str}）")
    } else {
        format!("{date_str}（{weekday_str}）")
    }
}

/// 日期平移工具函数。
fn shift_date_str(date_str: &str, delta_days: i64) -> String {
    let d = parse_date_or_today(date_str);
    let shifted = d + chrono::Duration::days(delta_days);
    format_date(shifted)
}

/// 右侧计划面板宽度（生成计划后窗口向右扩展的空间）。
const PLAN_PANEL_WIDTH: gpui::Pixels = px(640.);

// 6 行摘要（含“还有 N 项”提示）需要完整行高；148px 可避免底行被裁切。
const CALENDAR_CELL_HEIGHT: f32 = 148.;
/// 日期格内容区最多显示的行数；超出时最后一行改为省略提示。
const CALENDAR_CELL_MAX_SUMMARY_LINES: usize = 6;
/// 背景点阵的间距与点径。
const BACKDROP_GRID_STEP: f32 = 24.;
const BACKDROP_DOT: f32 = 2.;

/// 相对时间标签（"3分钟前"），用于历史条目。
fn ago_label(at: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    match now.saturating_sub(at) {
        0..=59 => "刚刚".to_string(),
        s if s < 3600 => format!("{}分钟前", s / 60),
        s if s < 86400 => format!("{}小时前", s / 3600),
        s => format!("{}天前", s / 86400),
    }
}

/// 全窗口背景装饰层（画在内容层之下）：
/// 先铺主题底色（根节点保持透明，让装饰层成为真正的最底层），
/// 再叠细点阵纸纹与少量大尺度低透明度几何色块——右上大圆、
/// 左下描边圆环、菱形与半调网点补丁，野兽风海报语言。
fn render_backdrop(dark: bool, base: gpui::Hsla) -> impl IntoElement {
    let paint = move |bounds: gpui::Bounds<gpui::Pixels>,
                      _: (),
                      window: &mut gpui::Window,
                      _: &mut gpui::App| {
        let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        let (ox, oy) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));

        // 0. 主题底色。
        window.paint_quad(fill(bounds, base));

        // 1. 纸面点阵。
        let dot_color = if dark {
            hsla(0.13, 0.25, 0.92, 0.05)
        } else {
            hsla(0.0, 0.0, 0.04, 0.06)
        };
        let half = BACKDROP_DOT / 2.;
        let mut gy = step_origin(oy);
        while gy < oy + h {
            let mut gx = step_origin(ox);
            while gx < ox + w {
                window.paint_quad(fill(
                    gpui::Bounds {
                        origin: point(px(gx - half), px(gy - half)),
                        size: size(px(BACKDROP_DOT), px(BACKDROP_DOT)),
                    },
                    dot_color,
                ));
                gx += BACKDROP_GRID_STEP;
            }
            gy += BACKDROP_GRID_STEP;
        }

        // 圆形用正多边形近似（48 段已足够圆滑）。
        let circle_pts = |cxp: f32, cyp: f32, r: f32| {
            (0..=48)
                .map(|i| {
                    let a = i as f32 * std::f32::consts::TAU / 48.;
                    point(px(cxp + r * a.cos()), px(cyp + r * a.sin()))
                })
                .collect::<Vec<_>>()
        };
        let paint_circle = |window: &mut gpui::Window, cxp: f32, cyp: f32, r: f32, color| {
            let mut pb = gpui::PathBuilder::fill();
            pb.add_polygon(&circle_pts(cxp, cyp, r), true);
            if let Ok(path) = pb.build() {
                window.paint_path(path, color);
            }
        };

        // 2. 右上大圆（主色，极低透明度）。
        let big_circle = if dark {
            hsla(0.16, 1.0, 0.62, 0.05)
        } else {
            hsla(0.135, 1.0, 0.5, 0.10)
        };
        paint_circle(window, ox + w - 120., oy + 60., 190., big_circle);

        // 3. 左下描边圆环。
        let ring = if dark {
            hsla(0.62, 0.9, 0.78, 0.08)
        } else {
            hsla(0.62, 1.0, 0.59, 0.10)
        };
        let mut pb = gpui::PathBuilder::stroke(px(5.));
        pb.add_polygon(&circle_pts(ox + 90., oy + h - 60., 120.), true);
        if let Ok(path) = pb.build() {
            window.paint_path(path, ring);
        }

        // 4. 标题右侧菱形点缀。
        let diamond = if dark {
            hsla(0.9, 0.85, 0.75, 0.09)
        } else {
            hsla(0.055, 1.0, 0.53, 0.12)
        };
        let (dx, dy, dr) = (ox + w * 0.62, oy + 110., 14.);
        let mut pb = gpui::PathBuilder::fill();
        pb.add_polygon(
            &[
                point(px(dx), px(dy - dr)),
                point(px(dx + dr), px(dy)),
                point(px(dx), px(dy + dr)),
                point(px(dx - dr), px(dy)),
            ],
            true,
        );
        if let Ok(path) = pb.build() {
            window.paint_path(path, diamond);
        }

        // 5. 左上半调网点补丁（行进间点径衰减，波普肌理）。
        let halftone = if dark {
            hsla(0.13, 0.25, 0.92, 0.10)
        } else {
            hsla(0.0, 0.0, 0.04, 0.12)
        };
        for row in 0..7 {
            for col in 0..10 {
                let r = 4.5 - col as f32 * 0.35 - row as f32 * 0.18;
                if r <= 0.4 {
                    continue;
                }
                let hx = ox + 40. + col as f32 * 14.;
                let hy = oy + h - 210. + row as f32 * 14.;
                paint_circle(window, hx, hy, r, halftone);
            }
        }
    };

    div()
        .absolute()
        .inset_0()
        .overflow_hidden()
        .child(canvas(|_, _, _| {}, paint))
}

/// 对齐到点阵网格的起始坐标（避免原点偏移导致边缘半格）。
fn step_origin(origin: f32) -> f32 {
    origin - origin % BACKDROP_GRID_STEP
}

// ---------------------------------------------------------------------------
// Neo-Brutalist 构件
// ---------------------------------------------------------------------------

/// 硬阴影：纯色无模糊、纯偏移，模拟丝网印刷的套版错位效果。
/// 透明度略降（0.85），避免纯色块在浅色背景上过于生硬。
fn hard_shadow(dark: bool) -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: if dark {
            hsla(0.16, 1.0, 0.62, 0.55) // 暗模式用荧光黄错位
        } else {
            hsla(0.0, 0.0, 0.04, 0.85) // 亮模式用纯墨
        },
        offset: point(px(6.), px(6.)),
        blur_radius: px(0.),
        spread_radius: px(0.),
    }]
}

/// 卡片入场动画：quint 减速曲线（先快后慢）+ 上滑淡入；
/// `delay`（0..0.5）裁剪时间轴，实现多卡片错峰入场。
/// `id` 需在同层兄弟间唯一（动画状态按元素 id 记账）。
fn entrance(id: &'static str, delay: f32, el: gpui::Div) -> gpui::AnimationElement<gpui::Div> {
    let quint_out = |t: f32| 1.0 - (1.0 - t).powi(5);
    el.with_animation(
        id,
        gpui::Animation::new(std::time::Duration::from_millis(560)),
        move |el, delta| {
            let t = ((delta - delay) / (1. - delay).max(1e-3)).clamp(0., 1.);
            let eased = quint_out(t);
            el.opacity(eased).top(px((1. - eased) * 14.))
        },
    )
}

/// 野兽风卡片：2px 墨边框 + 直角 + 硬阴影。
fn bcard(cx: &App) -> gpui::Div {
    let theme = cx.theme();
    let dark = theme.is_dark();
    v_flex()
        .w_full()
        .min_w_0()
        .gap_3()
        .p_5()
        .rounded_none()
        .border_2()
        .border_color(theme.foreground)
        .bg(theme.popover)
        .shadow(hard_shadow(dark))
}

/// 区块标题条：黄色底条带 + 黑色粗体标题。
/// 返回具体 `Div` 类型，调用方可继续追加子元素（如条带右侧动作）。
fn section_band(title: &str, icon_path: &str, cx: &App) -> gpui::Div {
    let theme = cx.theme();
    let dark = theme.is_dark();
    let band_bg = if dark {
        theme.primary // 暗模式主色即荧光黄
    } else {
        hsla(0.135, 1.0, 0.5, 1.0) // 亮模式明黄
    };
    h_flex()
        .items_center()
        .gap_2()
        .px_3()
        .py_2()
        .bg(band_bg)
        .border_2()
        .border_color(theme.foreground)
        .text_color(hsla(0.0, 0.0, 0.04, 1.0))
        .child(
            Icon::empty()
                .path(icon_path.to_string())
                .size_4()
                .text_color(hsla(0.0, 0.0, 0.04, 1.0)),
        )
        .child(
            div()
                .text_size(px(14.))
                .font_weight(FontWeight::BOLD)
                .child(title.to_string()),
        )
}

// ---------------------------------------------------------------------------
// 状态
// ---------------------------------------------------------------------------

pub enum Phase {
    Input,
    Loading,
    Ready(ReadyState),
}

pub struct PlannerApp {
    /// 当前活动标签页
    active_tab: AppTab,
    /// 今日打卡面板当前选中的日历日期 "YYYY-MM-DD"
    selected_date: String,
    /// 今日打卡面板按科目过滤 (None = 全部)
    filter_plan_id: Option<String>,
    /// 加入打卡时的起始日期输入框
    start_date_input: Entity<InputState>,
    /// 可选：将今天视为计划第 N 天，并据此自动倒推起始日期
    today_plan_day_input: Entity<InputState>,
    /// 加入打卡时是否跳过周末
    skip_weekends_toggle: bool,

    /// 自定义任务表单状态（自定义任务同样生成 StudyPlan，复用既有打卡闭环）。
    custom_task_form_open: bool,
    custom_title_input: Entity<InputState>,
    custom_start_date_input: Entity<InputState>,
    custom_days_input: Entity<InputState>,
    custom_duration_input: Entity<InputState>,
    custom_skip_weekends_toggle: bool,
    /// 计划库中整体调整未完成任务日期的弹窗。
    plan_reschedule_modal_open: bool,
    plan_reschedule_plan_id: Option<String>,
    plan_reschedule_date_input: Entity<InputState>,

    /// gpui-component 输入框为独立 `Entity<InputState>`，这里持有引用并
    /// 在渲染时绑定；取值通过 `read(cx).value()` 按需读取。
    link_input: Entity<InputState>,
    cookie_input: Entity<InputState>,
    jf_server_input: Entity<InputState>,
    jf_token_input: Entity<InputState>,
    /// 飞牛影视来源的服务器地址与登录凭证（地址 + 账号 + 密码）。
    fnos_server_input: Entity<InputState>,
    fnos_user_input: Entity<InputState>,
    fnos_password_input: Entity<InputState>,
    days_input: Entity<InputState>,
    /// 云端同步服务地址（在标题栏“云端设置”中编辑）。
    cloud_server_input: Entity<InputState>,

    source: SourceMode,
    mode: Mode,
    phase: Phase,
    last_error: Option<String>,
    fetch_progress: String,
    fetch_elapsed_secs: u64,
    fetch_generation: u64,

    /// 本机配置（Jellyfin 凭证 + 搜索历史 + 学习打卡计划），操作后写盘。
    config: AppConfig,

    /// 计划表状态；生成/切换科目/修改天数时重建。
    plan_table: Option<Entity<TableState<PlanTableDelegate>>>,

    /// 首次生成计划时已向右扩展过窗口，避免反复 resize 覆盖用户手动调整。
    window_expanded: bool,

    /// 学习日历面板当前查看的年、月
    calendar_year: i32,
    calendar_month: u32,
    /// 学习日历面板选中的具体日期 "YYYY-MM-DD"
    calendar_selected_date: String,
    /// 学习日历面板中对选中日期的备注输入框
    calendar_note_input: Entity<InputState>,

    /// 日历格右键新增任务的弹窗与表单状态。
    calendar_task_modal_open: bool,
    calendar_task_date: String,
    calendar_task_title_input: Entity<InputState>,
    calendar_task_duration_input: Entity<InputState>,
    calendar_task_date_input: Entity<InputState>,
    calendar_series_name_input: Entity<InputState>,
    calendar_task_target: CalendarTaskTarget,
    calendar_existing_series_id: Option<String>,
    calendar_task_edit_modal_open: bool,
    task_date_only: bool,
    calendar_task_edit_plan_id: Option<String>,
    calendar_task_edit_task_id: Option<String>,
    calendar_task_edit_title_input: Entity<InputState>,
    calendar_task_edit_duration_input: Entity<InputState>,
    calendar_task_edit_date_input: Entity<InputState>,

    /// 飞书云同步相关状态
    cloud_bind_modal_open: bool,
    cloud_bind_code: Option<String>,
    cloud_bind_expires: Option<u64>,
    cloud_syncing: bool,
    has_pending_auto_sync: bool,
    cloud_sync_modal_open: bool,
    cloud_sync_modal_data: Option<(String, bool, Vec<String>)>,
    cloud_settings_modal_open: bool,
    cloud_testing: bool,
    cloud_server_test_result: Option<(String, bool)>,
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
        let fnos_server_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("http://192.168.1.10:5666"));
        let fnos_user_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("飞牛影视账号（非飞牛系统账号）"));
        let fnos_password_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("飞牛影视密码")
                .masked(true)
        });
        let days_input = cx.new(|cx| InputState::new(window, cx).placeholder("如 30"));
        let cloud_server_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("https://plan.example.com"));
        let start_date_input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value(today_date_str(), window, cx);
            state
        });
        let today_plan_day_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("例如 7（可选）"));
        let custom_title_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("例如：刷题、背单词、阅读论文"));
        let custom_start_date_input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value(today_date_str(), window, cx);
            state
        });
        let custom_days_input = cx.new(|cx| InputState::new(window, cx).placeholder("例如 7"));
        let custom_duration_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("例如 30（分钟）"));
        let plan_reschedule_date_input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value(today_date_str(), window, cx);
            state.placeholder("YYYY-MM-DD")
        });

        // 启动时加载本机配置：Jellyfin 凭证预热输入框，历史记录供列表展示。
        let config = load_config().unwrap_or_default();

        use chrono::Datelike;
        let now_local = chrono::Local::now();
        let calendar_year = now_local.year();
        let calendar_month = now_local.month();
        let calendar_selected_date = today_date_str();
        let calendar_note_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("写下当天的学习总结、心得体会或重要备忘...")
        });
        let calendar_task_title_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("例如：完成第 3 章练习、背 50 个单词")
        });
        let calendar_task_duration_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("例如 45（分钟）"));
        let calendar_task_date_input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value(today_date_str(), window, cx);
            state.placeholder("YYYY-MM-DD")
        });
        let calendar_series_name_input = cx
            .new(|cx| InputState::new(window, cx).placeholder("例如：英语词汇冲刺、考研数学二轮"));
        let calendar_task_edit_title_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("任务名称"));
        let calendar_task_edit_duration_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("分钟数"));
        let calendar_task_edit_date_input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value(today_date_str(), window, cx);
            state.placeholder("YYYY-MM-DD")
        });

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

        if !config.server_url.trim().is_empty() {
            jf_server_input.update(cx, |state, cx| {
                state.set_value(config.server_url.clone(), window, cx)
            });
        }
        if !config.token.trim().is_empty() {
            jf_token_input.update(cx, |state, cx| {
                state.set_value(config.token.clone(), window, cx)
            });
        }
        if !config.fnos_server_url.trim().is_empty() {
            fnos_server_input.update(cx, |state, cx| {
                state.set_value(config.fnos_server_url.clone(), window, cx)
            });
        }
        if !config.fnos_username.trim().is_empty() {
            fnos_user_input.update(cx, |state, cx| {
                state.set_value(config.fnos_username.clone(), window, cx)
            });
        }
        if !config.fnos_password.is_empty() {
            fnos_password_input.update(cx, |state, cx| {
                state.set_value(config.fnos_password.clone(), window, cx)
            });
        }
        cloud_server_input.update(cx, |state, cx| {
            state.set_value(config.sync_server_url.clone(), window, cx)
        });

        let initial_tab = if config.plans.is_empty() {
            AppTab::PlanGenerator
        } else {
            AppTab::TodayCheckIn
        };

        Self {
            active_tab: initial_tab,
            selected_date: today_date_str(),
            filter_plan_id: None,
            start_date_input,
            today_plan_day_input,
            skip_weekends_toggle: false,
            custom_task_form_open: false,
            custom_title_input,
            custom_start_date_input,
            custom_days_input,
            custom_duration_input,
            custom_skip_weekends_toggle: false,
            plan_reschedule_modal_open: false,
            plan_reschedule_plan_id: None,
            plan_reschedule_date_input,
            link_input,
            cookie_input,
            jf_server_input,
            jf_token_input,
            fnos_server_input,
            fnos_user_input,
            fnos_password_input,
            days_input,
            cloud_server_input,
            calendar_year,
            calendar_month,
            calendar_selected_date,
            calendar_note_input,
            calendar_task_modal_open: false,
            calendar_task_date: today_date_str(),
            calendar_task_title_input,
            calendar_task_duration_input,
            calendar_task_date_input,
            calendar_series_name_input,
            calendar_task_target: CalendarTaskTarget::OneOff,
            calendar_existing_series_id: None,
            calendar_task_edit_modal_open: false,
            task_date_only: false,
            calendar_task_edit_plan_id: None,
            calendar_task_edit_task_id: None,
            calendar_task_edit_title_input,
            calendar_task_edit_duration_input,
            calendar_task_edit_date_input,
            source: SourceMode::Bilibili,
            mode: Mode::Split,
            phase: Phase::Input,
            last_error: None,
            fetch_progress: String::new(),
            fetch_elapsed_secs: 0,
            fetch_generation: 0,
            config,
            plan_table: None,
            window_expanded: false,
            cloud_bind_modal_open: false,
            cloud_bind_code: None,
            cloud_bind_expires: None,
            cloud_syncing: false,
            has_pending_auto_sync: false,
            cloud_sync_modal_open: false,
            cloud_sync_modal_data: None,
            cloud_settings_modal_open: false,
            cloud_testing: false,
            cloud_server_test_result: None,
        }
    }

    // -----------------------------------------------------------------------
    // 交互动作
    // -----------------------------------------------------------------------
}

mod action_calendar;
mod action_fetch;
mod action_history_export;
mod action_plans;
mod action_sync;
mod table;
mod view_calendar;
mod view_cloud_modals;
mod view_library;
mod view_modals;
mod view_plan_generator;
mod view_shell;
mod view_today;
use table::PlanTableDelegate;

impl Render for PlannerApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dark = cx.theme().is_dark();
        let theme = cx.theme().clone();

        let body: gpui::AnyElement = match self.active_tab {
            AppTab::TodayCheckIn => self.render_today_checkin_view(cx),
            AppTab::Calendar => self.render_calendar_view(cx),
            AppTab::PlanGenerator => self.render_plan_generator_view(cx),
            AppTab::MyPlans => self.render_my_plans_view(cx),
        };

        let bind_modal = self
            .cloud_bind_modal_open
            .then(|| self.render_bind_modal(cx));
        let sync_modal = self
            .cloud_sync_modal_open
            .then(|| self.render_sync_result_modal(cx));
        let cloud_settings_modal = self
            .cloud_settings_modal_open
            .then(|| self.render_cloud_settings_modal(cx));
        let calendar_task_modal = self
            .calendar_task_modal_open
            .then(|| self.render_calendar_task_modal(cx));
        let calendar_task_edit_modal = self
            .calendar_task_edit_modal_open
            .then(|| self.render_calendar_task_edit_modal(cx));
        let plan_reschedule_modal = self
            .plan_reschedule_modal_open
            .then(|| self.render_plan_reschedule_modal(cx));
        let sheet_layer = Root::render_sheet_layer(window, cx);
        let dialog_layer = Root::render_dialog_layer(window, cx);
        let notification_layer = Root::render_notification_layer(window, cx);

        div()
            .size_full()
            .flex()
            .flex_col()
            .relative()
            // 根节点不再铺底色：底色与装饰纹理由 render_backdrop 先画，
            // 内容层保持透明，卡片阴影/纹理才能透出层次。
            .text_color(theme.foreground)
            .child(render_backdrop(dark, theme.background))
            .child(self.render_title_bar(dark, cx))
            // 关键：内容包裹层必须是 flex 容器。gpui 的 div() 默认 display:Block，
            // Block 子元素高度为 auto，各页面根节点（overflow_y_scroll + 百分比高度）
            // 解析不到确定高度，滚动永远不会触发、内容溢出窗口底部。
            .child(div().flex().flex_1().min_h_0().child(body))
            .children(bind_modal)
            .children(sync_modal)
            .children(cloud_settings_modal)
            .children(calendar_task_modal)
            .children(calendar_task_edit_modal)
            .children(plan_reschedule_modal)
            .children(sheet_layer)
            .children(dialog_layer)
            .children(notification_layer)
    }
}

#[cfg(test)]
mod tests;
