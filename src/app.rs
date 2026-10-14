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

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    canvas, div, hsla, point, prelude::*, px, AnimationExt, App, BoxShadow, Context, Entity,
    Focusable, FontWeight, InteractiveElement, IntoElement, MouseButton, Render, RenderImage,
    Styled, Window,
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
use image::{Frame, RgbaImage};

use crate::core::{
    add_custom_study_plan, add_daily_note, add_one_off_calendar_task,
    advance_completed_study_tasks, append_calendar_series_task, check_cloud_bind_status,
    checkin_study_task, clear_cloud_binding_state, clear_history, create_calendar_series_plan,
    delete_calendar_task, delete_cloud_binding, delete_daily_note, enroll_study_plan,
    export_payload, generate_plan, get_daily_notes, load_config, parse_days,
    push_forward_study_plan, record_history, redistribute_unfinished_study_plan, remove_history,
    remove_study_plan, request_cloud_bind_code, reschedule_unfinished_study_plan, sync_with_cloud,
    toggle_study_plan_status, try_save_config, update_calendar_task, AppConfig, FetchSource,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum PlanRescheduleMode {
    #[default]
    ShiftStart,
    RedistributeToEnd,
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

/// 全窗口背景装饰层（画在内容层之下）。
///
/// 装饰（点阵、几何色块）按窗口像素栅格化成一张 `RenderImage` 并缓存，
/// 每帧只提交 1 个 sprite；尺寸或主题变化时才重建。此前每帧用数千个
/// `paint_quad`/`PathBuilder` 重绘，是滚轮滚动卡顿的主要来源。
fn render_backdrop(dark: bool, base: gpui::Hsla) -> impl IntoElement {
    let paint = move |bounds: gpui::Bounds<gpui::Pixels>,
                      _: (),
                      window: &mut gpui::Window,
                      _: &mut gpui::App| {
        let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        if w <= 0. || h <= 0. {
            return;
        }
        let scale = window.scale_factor();
        let image = backdrop_image(w, h, scale, dark, base);
        let _ = window.paint_image(bounds, gpui::Corners::default(), image, 0, false);
    };

    div()
        .absolute()
        .inset_0()
        .overflow_hidden()
        .child(canvas(|_, _, _| {}, paint))
}

/// 背景纹理缓存键：设备像素尺寸 + 是否暗色。
type BackdropKey = (u32, u32, bool);

thread_local! {
    /// 同一窗口同一主题下背景纹理只保留最近一张。
    static BACKDROP_CACHE: RefCell<Option<(BackdropKey, Arc<RenderImage>)>> =
        const { RefCell::new(None) };
}

/// 取（或按需生成）背景装饰位图。像素按设备分辨率栅格化，避免高 DPI 发糊。
fn backdrop_image(
    logical_w: f32,
    logical_h: f32,
    scale: f32,
    dark: bool,
    base: gpui::Hsla,
) -> Arc<RenderImage> {
    let dw = (logical_w * scale).ceil().max(1.) as u32;
    let dh = (logical_h * scale).ceil().max(1.) as u32;
    let key = (dw, dh, dark);

    BACKDROP_CACHE.with(|cache| {
        if let Some((k, img)) = cache.borrow().as_ref() {
            if *k == key {
                return img.clone();
            }
        }
        let img = Arc::new(rasterize_backdrop(dw, dh, scale, dark, base));
        *cache.borrow_mut() = Some((key, img.clone()));
        img
    })
}

/// 将底色 + 装饰一次性栅格化进 BGRA 缓冲（gpui 的 `RenderImage` 约定 BGRA）。
fn rasterize_backdrop(dw: u32, dh: u32, scale: f32, dark: bool, base: gpui::Hsla) -> RenderImage {
    let base_rgb = base.to_rgb();
    let mut img = RgbaImage::from_pixel(
        dw,
        dh,
        image::Rgba(rgba_bytes(base_rgb.r, base_rgb.g, base_rgb.b, 1.0)),
    );

    let w = dw as f32 / scale;
    let h = dh as f32 / scale;

    // 1. 纸面点阵。
    let dot_color = if dark {
        hsla(0.13, 0.25, 0.92, 0.05)
    } else {
        hsla(0.0, 0.0, 0.04, 0.06)
    };
    let half = BACKDROP_DOT / 2.;
    let mut gy = 0.;
    while gy < h {
        let mut gx = 0.;
        while gx < w {
            fill_circle(
                &mut img,
                scale,
                gx,
                gy,
                half.max(0.5),
                hsla_to_bgra(dot_color),
            );
            gx += BACKDROP_GRID_STEP;
        }
        gy += BACKDROP_GRID_STEP;
    }

    // 2. 右上大圆（主色，极低透明度）。
    let big_circle = if dark {
        hsla(0.16, 1.0, 0.62, 0.05)
    } else {
        hsla(0.135, 1.0, 0.5, 0.10)
    };
    fill_circle(
        &mut img,
        scale,
        w - 120.,
        60.,
        190.,
        hsla_to_bgra(big_circle),
    );

    // 3. 左下描边圆环（≈5px 线宽）。
    let ring = if dark {
        hsla(0.62, 0.9, 0.78, 0.08)
    } else {
        hsla(0.62, 1.0, 0.59, 0.10)
    };
    fill_ring(&mut img, scale, 90., h - 60., 120., 2.5, hsla_to_bgra(ring));

    // 4. 标题右侧菱形点缀。
    let diamond = if dark {
        hsla(0.9, 0.85, 0.75, 0.09)
    } else {
        hsla(0.055, 1.0, 0.53, 0.12)
    };
    let (dx, dy, dr) = (w * 0.62, 110., 14.);
    fill_diamond(&mut img, scale, dx, dy, dr, hsla_to_bgra(diamond));

    // 5. 左上半调网点补丁（行进间点径衰减，波普肌理）。
    let halftone = if dark {
        hsla(0.13, 0.25, 0.92, 0.10)
    } else {
        hsla(0.0, 0.0, 0.04, 0.12)
    };
    let ht = hsla_to_bgra(halftone);
    for row in 0..7 {
        for col in 0..10 {
            let r = 4.5 - col as f32 * 0.35 - row as f32 * 0.18;
            if r <= 0.4 {
                continue;
            }
            let hx = 40. + col as f32 * 14.;
            let hy = h - 210. + row as f32 * 14.;
            fill_circle(&mut img, scale, hx, hy, r, ht);
        }
    }

    // RenderImage 约定 BGRA；这里把 RGBA 通道写进 RgbaImage 的字节槽。
    for px in img.pixels_mut() {
        px.0.swap(0, 2);
    }

    let frame = Frame::new(img);
    RenderImage::new(vec![frame])
}

fn rgba_bytes(r: f32, g: f32, b: f32, a: f32) -> [u8; 4] {
    [
        (r.clamp(0., 1.) * 255.).round() as u8,
        (g.clamp(0., 1.) * 255.).round() as u8,
        (b.clamp(0., 1.) * 255.).round() as u8,
        (a.clamp(0., 1.) * 255.).round() as u8,
    ]
}

fn hsla_to_bgra(c: gpui::Hsla) -> [u8; 4] {
    let rgb = c.to_rgb();
    // 与最终 BGRA 打包一致：先按 RGBA 写入，收尾统一 swap。
    rgba_bytes(rgb.r, rgb.g, rgb.b, rgb.a)
}

fn blend_pixel(img: &mut RgbaImage, x: u32, y: u32, rgba: [u8; 4]) {
    let Some(pixel) = img.get_pixel_mut_checked(x, y) else {
        return;
    };
    let [r, g, b, a] = rgba;
    if a == 0 {
        return;
    }
    if a == 255 {
        pixel.0 = [r, g, b, 255];
        return;
    }
    let af = a as f32 / 255.;
    let inv = 1. - af;
    let dst = pixel.0;
    pixel.0 = [
        (r as f32 * af + dst[0] as f32 * inv) as u8,
        (g as f32 * af + dst[1] as f32 * inv) as u8,
        (b as f32 * af + dst[2] as f32 * inv) as u8,
        255,
    ];
}

/// 逻辑坐标下的实心圆（低透明度装饰，直接距离场即可）。
fn fill_circle(img: &mut RgbaImage, scale: f32, cx: f32, cy: f32, r: f32, rgba: [u8; 4]) {
    if r <= 0. || rgba[3] == 0 {
        return;
    }
    let (scx, scy, sr) = (cx * scale, cy * scale, r * scale);
    let x0 = ((scx - sr).floor().max(0.)) as u32;
    let y0 = ((scy - sr).floor().max(0.)) as u32;
    let x1 = ((scx + sr).ceil().min(img.width() as f32)) as u32;
    let y1 = ((scy + sr).ceil().min(img.height() as f32)) as u32;
    let r2 = sr * sr;
    for y in y0..y1 {
        for x in x0..x1 {
            let dx = x as f32 + 0.5 - scx;
            let dy = y as f32 + 0.5 - scy;
            if dx * dx + dy * dy <= r2 {
                blend_pixel(img, x, y, rgba);
            }
        }
    }
}

/// 描边圆环：外径 r、线宽 half_w（逻辑像素）。
fn fill_ring(
    img: &mut RgbaImage,
    scale: f32,
    cx: f32,
    cy: f32,
    r: f32,
    half_w: f32,
    rgba: [u8; 4],
) {
    if rgba[3] == 0 {
        return;
    }
    let (scx, scy) = (cx * scale, cy * scale);
    let outer = (r + half_w) * scale;
    let inner = (r - half_w).max(0.) * scale;
    let (outer2, inner2) = (outer * outer, inner * inner);
    let x0 = ((scx - outer).floor().max(0.)) as u32;
    let y0 = ((scy - outer).floor().max(0.)) as u32;
    let x1 = ((scx + outer).ceil().min(img.width() as f32)) as u32;
    let y1 = ((scy + outer).ceil().min(img.height() as f32)) as u32;
    for y in y0..y1 {
        for x in x0..x1 {
            let dx = x as f32 + 0.5 - scx;
            let dy = y as f32 + 0.5 - scy;
            let d2 = dx * dx + dy * dy;
            if d2 <= outer2 && d2 >= inner2 {
                blend_pixel(img, x, y, rgba);
            }
        }
    }
}

/// 菱形（对角线半长 dr）。
fn fill_diamond(img: &mut RgbaImage, scale: f32, cx: f32, cy: f32, dr: f32, rgba: [u8; 4]) {
    if rgba[3] == 0 {
        return;
    }
    let (scx, scy, sr) = (cx * scale, cy * scale, dr * scale);
    let x0 = ((scx - sr).floor().max(0.)) as u32;
    let y0 = ((scy - sr).floor().max(0.)) as u32;
    let x1 = ((scx + sr).ceil().min(img.width() as f32)) as u32;
    let y1 = ((scy + sr).ceil().min(img.height() as f32)) as u32;
    for y in y0..y1 {
        for x in x0..x1 {
            let dx = (x as f32 + 0.5 - scx).abs();
            let dy = (y as f32 + 0.5 - scy).abs();
            if dx + dy <= sr {
                blend_pixel(img, x, y, rgba);
            }
        }
    }
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

/// 卡片入场动画：quint 减速曲线（先快后慢）+ 淡入。
/// 只改 opacity，不碰 top/位移：滚动重绘时不触发额外布局。
/// `delay`（0..0.5）裁剪时间轴，实现多卡片错峰入场。
/// `id` 需在同层兄弟间唯一（动画状态按元素 id 记账）。
fn entrance(id: &'static str, delay: f32, el: gpui::Div) -> gpui::AnimationElement<gpui::Div> {
    let quint_out = |t: f32| 1.0 - (1.0 - t).powi(5);
    el.with_animation(
        id,
        gpui::Animation::new(std::time::Duration::from_millis(480)),
        move |el, delta| {
            let t = ((delta - delay) / (1. - delay).max(1e-3)).clamp(0., 1.);
            el.opacity(quint_out(t))
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
    plan_reschedule_mode: PlanRescheduleMode,
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
    /// 「我的计划库」卡片列表虚拟滚动状态。
    my_plans_list: gpui::ListState,
    /// 「今日打卡」科目分组卡片列表虚拟滚动状态。
    today_list: gpui::ListState,

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
    /// 解除云端绑定的二次确认；首次点击打开，再次点击才真正删除。
    cloud_unbind_confirm: bool,
}

impl PlannerApp {
    /// 列表项点击：闭包捕获 `WeakEntity`，在 `&mut App` 里回到本视图。
    /// `list` 的 render 回调拿不到 `Context<Self>`，不能直接 `cx.listener`。
    fn list_action(
        this: &gpui::WeakEntity<Self>,
        f: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static {
        let this = this.clone();
        move |_, window, cx| {
            this.update(cx, |view, cx| f(view, window, cx)).ok();
        }
    }

    /// 把当前配置写入本地库；失败时通知用户，不让“内存已改”冒充已落盘。
    fn persist_config(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match try_save_config(&self.config) {
            Ok(()) => true,
            Err(error) => {
                window.push_notification(
                    Notification::error(format!("保存本地配置失败: {error}")),
                    cx,
                );
                false
            }
        }
    }

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
            plan_reschedule_mode: PlanRescheduleMode::default(),
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
            my_plans_list: gpui::ListState::new(0, gpui::ListAlignment::Top, px(240.)),
            today_list: gpui::ListState::new(0, gpui::ListAlignment::Top, px(240.)),
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
            cloud_unbind_confirm: false,
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
