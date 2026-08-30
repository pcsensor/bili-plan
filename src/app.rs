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
    Entity, Focusable, FontWeight, InteractiveElement, IntoElement, Render, Styled, Window,
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
    v_flex, ActiveTheme, Disableable, Icon, IconName, Root, Selectable, Sizable, Theme, ThemeMode, TitleBar,
    WindowExt,
};

use crate::core::{
    check_cloud_bind_status, checkin_study_task, clear_history, enroll_study_plan, export_payload,
    generate_plan, load_config, parse_days, push_forward_study_plan, record_history,
    remove_history, remove_study_plan, request_cloud_bind_code, save_config, set_daily_note,
    sync_with_cloud, toggle_study_plan_status, AppConfig, FetchSource, ReadyState, Selection,
    SourceMode,
};
use crate::plan::{fmt_human, fmt_seconds, Mode, PlanEntry};
use crate::study::{
    compute_month_study_stats, compute_plan_progress, compute_study_stats, format_date,
    generate_month_calendar_matrix, get_tasks_for_date, parse_date_or_today, today_date_str,
    PlanStatus, StudyPlan,
};

/// 顶部活动标签页。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AppTab {
    #[default]
    TodayCheckIn,
    Calendar,
    PlanGenerator,
    MyPlans,
}

/// 打开外部视频链接。
fn open_video_link(source_type: &str, source_url: &str, vid_no: i64) {
    let url = if source_type == "bilibili" {
        let trimmed = source_url.trim();
        // 识别形如 BV... 的 12 位 ID
        let chars: Vec<char> = trimmed.chars().collect();
        let n = chars.len();
        let mut found_bv = None;
        for i in 0..n.saturating_sub(11) {
            if chars[i] == 'B'
                && chars[i + 1] == 'V'
                && chars[i + 2..i + 12].iter().all(|c| c.is_ascii_alphanumeric())
            {
                found_bv = Some(chars[i..i + 12].iter().collect::<String>());
                break;
            }
        }

        if let Some(bvid) = found_bv {
            format!("https://www.bilibili.com/video/{bvid}?p={vid_no}")
        } else if trimmed.starts_with("http") {
            let base = trimmed.split('?').next().unwrap_or(trimmed);
            format!("{base}?p={vid_no}")
        } else {
            format!("https://www.bilibili.com/video/{trimmed}?p={vid_no}")
        }
    } else {
        source_url.to_string()
    };

    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(&url).spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd").args(["/c", "start", &url]).spawn();
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
    /// 加入打卡时是否跳过周末
    skip_weekends_toggle: bool,

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

    /// 飞书云同步相关状态
    cloud_bind_modal_open: bool,
    cloud_bind_code: Option<String>,
    cloud_bind_expires: Option<u64>,
    cloud_syncing: bool,
    has_pending_auto_sync: bool,
    cloud_sync_modal_open: bool,
    cloud_sync_modal_data: Option<(String, bool, Vec<String>)>,
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
        let start_date_input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value(today_date_str(), window, cx);
            state
        });

        // 启动时加载本机配置：Jellyfin 凭证预热输入框，历史记录供列表展示。
        let config = load_config().unwrap_or_default();

        use chrono::Datelike;
        let now_local = chrono::Local::now();
        let calendar_year = now_local.year();
        let calendar_month = now_local.month();
        let calendar_selected_date = today_date_str();
        let initial_note = config.daily_notes.get(&calendar_selected_date).cloned().unwrap_or_default();
        let calendar_note_input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value(initial_note, window, cx);
            state.placeholder("写下当天的学习总结、心得体会或重要备忘...")
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
            skip_weekends_toggle: false,
            link_input,
            cookie_input,
            jf_server_input,
            jf_token_input,
            days_input,
            calendar_year,
            calendar_month,
            calendar_selected_date,
            calendar_note_input,
            source: SourceMode::Bilibili,
            mode: Mode::Split,
            phase: Phase::Input,
            last_error: None,
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

        let source_mode = self.source;
        cx.spawn_in(window, async move |this, cx| {
            let fetch_input = input.clone();
            let fetch_source = source.clone();
            let result = cx
                .background_executor()
                .spawn(async move { crate::core::fetch_and_parse(&fetch_input, &fetch_source) })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.on_fetched(result, input, source_mode, window, cx)
            })
            .ok();
        })
        .detach();
    }

    fn on_fetched(
        &mut self,
        result: Result<ReadyState, String>,
        input: String,
        source_mode: SourceMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(mut rd) => {
                // 本次成功：记住凭证（Jellyfin）+ 记录搜索历史并写盘。
                if matches!(source_mode, SourceMode::Jellyfin) {
                    self.config.server_url = self
                        .input_value(&self.jf_server_input, cx)
                        .trim()
                        .to_string();
                    self.config.token = self
                        .input_value(&self.jf_token_input, cx)
                        .trim()
                        .to_string();
                }
                record_history(&mut self.config, source_mode, &input, &rd.season_title);
                save_config(&self.config);
                // 获取成功后自动按当前天数与模式生成一次计划。
                match self.days(cx) {
                    Ok(days) => Self::run_generate(
                        &mut rd,
                        self.mode,
                        days,
                        &mut self.plan_table,
                        &mut self.window_expanded,
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
    /// 首次生成计划时把窗口向右加宽，为计划表腾出右侧空间。
    /// 最大化 / 全屏下跳过；仅执行一次，后续尊重用户手动调整的尺寸。
    /// 宽度以屏幕右缘为上限——`resize` 不改变原点，超屏会截断右栏。
    fn expand_window_for_plan(window: &mut Window, window_expanded: &mut bool, cx: &App) {
        if *window_expanded || !matches!(window.window_bounds(), gpui::WindowBounds::Windowed(_)) {
            return;
        }
        *window_expanded = true;
        let b = window.bounds();
        let want = b.size.width + PLAN_PANEL_WIDTH + px(32.);
        let mut new_w = want.min(b.size.width * 2.0);
        if let Some(display) = window.display(cx) {
            let avail = display.bounds().right() - px(12.) - b.origin.x;
            if avail < b.size.width {
                return; // 窗口已贴右缘，不再扩张，交由 flex 压缩左栏
            }
            new_w = new_w.min(avail);
        }
        window.resize(gpui::size(new_w, b.size.height));
    }

    fn run_generate(
        rd: &mut ReadyState,
        mode: Mode,
        days: i64,
        plan_table: &mut Option<Entity<TableState<PlanTableDelegate>>>,
        window_expanded: &mut bool,
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
                    Self::expand_window_for_plan(window, window_expanded, cx);
                    let delegate = PlanTableDelegate::new(plan);
                    *plan_table = Some(cx.new(|cx| {
                        let mut state = TableState::new(delegate, window, cx);
                        // 只读展示表：关闭行/列选择与排序（开启列宽拖拽自适应）。
                        state.col_selectable = false;
                        state.row_selectable = false;
                        state.col_movable = false;
                        state.col_resizable = true;
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
            Self::run_generate(
                rd,
                self.mode,
                days,
                &mut self.plan_table,
                &mut self.window_expanded,
                window,
                cx,
            );
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
                Self::run_generate(
                    rd,
                    self.mode,
                    days,
                    &mut self.plan_table,
                    &mut self.window_expanded,
                    window,
                    cx,
                );
            }
        }
        cx.notify();
    }

    fn switch_source(&mut self, source: SourceMode, window: &mut Window, cx: &mut Context<Self>) {
        // IMK 修复：切换前先让链接输入框失焦，避免 macOS 输入法框架
        // 在 placeholder 变更时唤醒已挂起的 IMKCFRunLoop（stderr 噪声来源）。
        if source != self.source {
            self.link_input.update(cx, |state, cx| {
                if state.focus_handle(cx).is_focused(window) {
                    window.blur();
                }
            });
        }
        self.source = source;
        self.last_error = None;
        // 链接输入的 placeholder 随来源切换；仅在确实需要时设置，
        // 多余的 set_placeholder 会触发一次 IMK 标记文本重建。
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
                Self::run_generate(
                    rd,
                    mode,
                    days,
                    &mut self.plan_table,
                    &mut self.window_expanded,
                    window,
                    cx,
                );
            }
        }
        cx.notify();
    }

    // -----------------------------------------------------------------------
    // 进度打卡与多科目管理交互动作
    // -----------------------------------------------------------------------

    /// 把当前生成的计划加入打卡计划库。
    fn enroll_current_plan(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Phase::Ready(rd) = &self.phase else {
            window.push_notification(Notification::warning("请先获取视频并生成观看计划。"), cx);
            return;
        };
        let input = self.input_value(&self.link_input, cx);
        let start_date = self.input_value(&self.start_date_input, cx);
        let start_date = if start_date.trim().is_empty() {
            today_date_str()
        } else {
            start_date.trim().to_string()
        };
        let source_tag = match self.source {
            SourceMode::Bilibili => "bilibili",
            SourceMode::Jellyfin => "jellyfin",
        };

        match enroll_study_plan(
            &mut self.config,
            rd,
            &input,
            source_tag,
            &start_date,
            self.skip_weekends_toggle,
        ) {
            Ok(plan) => {
                window.push_notification(
                    Notification::success(format!("已成功开启《{}》每日打卡计划！", plan.title)),
                    cx,
                );
                self.active_tab = AppTab::TodayCheckIn;
                self.selected_date = today_date_str();
                self.trigger_auto_sync(window, cx);
            }
            Err(e) => {
                window.push_notification(Notification::error(e), cx);
            }
        }
        cx.notify();
    }

    /// 切换单个学习任务的打卡状态。
    fn toggle_task_checkin_action(
        &mut self,
        plan_id: &str,
        task_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match checkin_study_task(&mut self.config, plan_id, task_id) {
            Ok(completed) => {
                if completed {
                    window.push_notification(
                        Notification::success("已完成打卡！保持专注与节奏 🔥"),
                        cx,
                    );
                } else {
                    window.push_notification(Notification::info("已撤回该项打卡"), cx);
                }
                self.trigger_auto_sync(window, cx);
            }
            Err(e) => {
                window.push_notification(Notification::error(e), cx);
            }
        }
        cx.notify();
    }

    /// 一键打卡指定计划在某一天的全部任务。
    fn checkin_entire_day_action(
        &mut self,
        plan_id: &str,
        date: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(e) = crate::study::checkin_entire_day(&mut self.config.plans, plan_id, date) {
            window.push_notification(Notification::error(e), cx);
        } else {
            save_config(&self.config);
            window.push_notification(
                Notification::success("🎉 今日该科目任务已全部完成打卡！"),
                cx,
            );
            self.trigger_auto_sync(window, cx);
        }
        cx.notify();
    }

    /// 针对所有进行中的计划执行一键落后顺延。
    fn push_forward_all_behind_action(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let today = today_date_str();
        let plan_ids: Vec<String> = self
            .config
            .plans
            .iter()
            .filter(|p| p.status == PlanStatus::Active)
            .map(|p| p.id.clone())
            .collect();

        let mut count = 0;
        for id in plan_ids {
            if push_forward_study_plan(&mut self.config, &id, &today).is_ok() {
                count += 1;
            }
        }
        if count > 0 {
            window.push_notification(
                Notification::success(format!("已成功为 {count} 门科目的未完成任务顺延至今日！")),
                cx,
            );
            self.trigger_auto_sync(window, cx);
        } else {
            window.push_notification(Notification::info("暂无需要顺延的落后计划"), cx);
        }
        cx.notify();
    }

    /// 切换计划状态（暂停/继续）。
    fn toggle_plan_status_action(&mut self, plan_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        toggle_study_plan_status(&mut self.config, plan_id);
        self.trigger_auto_sync(window, cx);
        cx.notify();
    }

    /// 删除指定计划。
    fn delete_plan_action(&mut self, plan_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        remove_study_plan(&mut self.config, plan_id);
        window.push_notification(Notification::info("已移除该学习计划"), cx);
        self.trigger_auto_sync(window, cx);
        cx.notify();
    }

    /// 顺延单门计划。
    fn push_forward_single_plan_action(
        &mut self,
        plan_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let today = today_date_str();
        match push_forward_study_plan(&mut self.config, plan_id, &today) {
            Ok(()) => {
                window.push_notification(Notification::success("已顺延本科目未完成任务至今日！"), cx);
                self.trigger_auto_sync(window, cx);
            }
            Err(e) => {
                window.push_notification(Notification::error(e), cx);
            }
        }
        cx.notify();
    }

    /// 日期前翻一天。
    fn prev_date_action(&mut self, cx: &mut Context<Self>) {
        self.selected_date = shift_date_str(&self.selected_date, -1);
        cx.notify();
    }

    /// 日期后翻一天。
    fn next_date_action(&mut self, cx: &mut Context<Self>) {
        self.selected_date = shift_date_str(&self.selected_date, 1);
        cx.notify();
    }

    /// 快速回到今天。
    fn reset_today_action(&mut self, cx: &mut Context<Self>) {
        self.selected_date = today_date_str();
        cx.notify();
    }

    /// 学习日历：前翻一个月。
    fn prev_calendar_month_action(&mut self, cx: &mut Context<Self>) {
        if self.calendar_month == 1 {
            self.calendar_year -= 1;
            self.calendar_month = 12;
        } else {
            self.calendar_month -= 1;
        }
        cx.notify();
    }

    /// 学习日历：后翻一个月。
    fn next_calendar_month_action(&mut self, cx: &mut Context<Self>) {
        if self.calendar_month == 12 {
            self.calendar_year += 1;
            self.calendar_month = 1;
        } else {
            self.calendar_month += 1;
        }
        cx.notify();
    }

    /// 学习日历：快速回到本月与今日。
    fn reset_calendar_month_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use chrono::Datelike;
        let now = chrono::Local::now();
        self.calendar_year = now.year();
        self.calendar_month = now.month();
        let today = today_date_str();
        self.select_calendar_date_action(&today, window, cx);
    }

    /// 学习日历：选中特定日期（更新右侧详细信息及备注输入框）。
    fn select_calendar_date_action(&mut self, date_str: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.calendar_selected_date = date_str.to_string();
        let current_note = self.config.daily_notes.get(date_str).cloned().unwrap_or_default();
        self.calendar_note_input.update(cx, |state, cx| {
            state.set_value(current_note, window, cx);
        });
        cx.notify();
    }

    /// 学习日历：保存当日备注。
    fn save_calendar_note_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let note = self.input_value(&self.calendar_note_input, cx);
        let date = self.calendar_selected_date.clone();
        set_daily_note(&mut self.config, &date, &note);
        window.push_notification(Notification::success(format!("已更新 {date} 的学习备注")), cx);
        if self.config.auto_sync {
            self.trigger_auto_sync(window, cx);
        }
        cx.notify();
    }

    /// 学习日历：清空当日备注。
    fn clear_calendar_note_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let date = self.calendar_selected_date.clone();
        set_daily_note(&mut self.config, &date, "");
        self.calendar_note_input.update(cx, |state, cx| {
            state.set_value("", window, cx);
        });
        window.push_notification(Notification::info(format!("已清除 {date} 的备注")), cx);
        if self.config.auto_sync {
            self.trigger_auto_sync(window, cx);
        }
        cx.notify();
    }

    /// 安全地将云端返回的最新打卡状态合并到本地正在编辑的计划中（防止覆盖本地尚未同步的最新修改）。
    fn merge_synced_plans(&mut self, remote_plans: Vec<StudyPlan>) {
        if self.config.plans.is_empty() {
            self.config.plans = remote_plans;
            return;
        }

        let mut remote_map: std::collections::HashMap<String, StudyPlan> = std::collections::HashMap::new();
        for rp in remote_plans {
            remote_map.insert(rp.id.clone(), rp);
        }

        for plan in &mut self.config.plans {
            if let Some(rp) = remote_map.get(&plan.id) {
                let mut remote_tasks: std::collections::HashMap<&str, &crate::study::TaskItem> =
                    std::collections::HashMap::new();
                for sch in &rp.schedules {
                    for t in &sch.tasks {
                        remote_tasks.insert(t.id.as_str(), t);
                    }
                }

                for sch in &mut plan.schedules {
                    for t in &mut sch.tasks {
                        if let Some(rt) = remote_tasks.get(t.id.as_str()) {
                            // 仅当远端打卡时间更新时才采纳（Last-Write-Wins）
                            if rt.updated_at > t.updated_at {
                                t.completed = rt.completed;
                                t.completed_at = rt.completed_at;
                                t.updated_at = rt.updated_at;
                            }
                        }
                    }
                }
            }
        }
    }

    /// 触发与云服务双向增量同步（手动同步，弹窗呈现详情）。
    fn sync_cloud_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cloud_syncing {
            return;
        }
        self.cloud_syncing = true;
        cx.notify();

        let mut cfg_clone = self.config.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let res = sync_with_cloud(&mut cfg_clone);
                    (res, cfg_clone)
                })
                .await;

            this.update_in(cx, |this, window, cx| {
                this.cloud_syncing = false;
                let (res, synced_cfg) = result;
                match res {
                    Ok(_) => {
                        this.merge_synced_plans(synced_cfg.plans);
                        this.config.feishu_bound = synced_cfg.feishu_bound;
                        this.config.feishu_user_name = synced_cfg.feishu_user_name;
                        if synced_cfg.sync_device_token.is_some() {
                            this.config.sync_device_token = synced_cfg.sync_device_token;
                        }
                        save_config(&this.config);

                        let feishu_status = if this.config.feishu_bound {
                            format!(
                                "已绑定 ({})",
                                this.config.feishu_user_name.as_deref().unwrap_or("学习者")
                            )
                        } else {
                            "未连接飞书机器人 (点击「绑定飞书」开始连接)".to_string()
                        };
                        let plan_count = this.config.plans.len();
                        let today = today_date_str();
                        let today_tasks = get_tasks_for_date(&this.config.plans, &today);
                        let done_count = today_tasks.iter().filter(|t| t.task.completed).count();

                        this.cloud_sync_modal_data = Some((
                            "云端同步成功".to_string(),
                            true,
                            vec![
                                format!("📚 学习科目：共 {plan_count} 门计划已完成状态对齐"),
                                format!("📱 飞书状态：{feishu_status}"),
                                format!("🔥 今日任务：共 {} 项，已完成 {done_count} 项打卡", today_tasks.len()),
                                "✨ 本地与云端数据已保持最新一致！".to_string(),
                            ],
                        ));
                        this.cloud_sync_modal_open = true;
                    }
                    Err(e) => {
                        this.cloud_sync_modal_data = Some((
                            "云端同步失败".to_string(),
                            false,
                            vec![
                                format!("❌ 失败原因：{e}"),
                                "💡 建议：请检查本地网络连接及云服务器运行状态。".to_string(),
                            ],
                        ));
                        this.cloud_sync_modal_open = true;
                    }
                }
                if this.has_pending_auto_sync {
                    this.has_pending_auto_sync = false;
                    this.trigger_auto_sync(window, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// 触发检测到修改后的自动同步（静默异步执行，成功后右上角弹出通知并自动消失）。
    fn trigger_auto_sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.config.auto_sync {
            return;
        }
        if self.cloud_syncing {
            self.has_pending_auto_sync = true;
            return;
        }
        self.cloud_syncing = true;
        cx.notify();

        let mut cfg_clone = self.config.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let res = sync_with_cloud(&mut cfg_clone);
                    (res, cfg_clone)
                })
                .await;

            this.update_in(cx, |this, window, cx| {
                this.cloud_syncing = false;
                let (res, synced_cfg) = result;
                match res {
                    Ok(_) => {
                        this.merge_synced_plans(synced_cfg.plans);
                        this.config.feishu_bound = synced_cfg.feishu_bound;
                        this.config.feishu_user_name = synced_cfg.feishu_user_name;
                        if synced_cfg.sync_device_token.is_some() {
                            this.config.sync_device_token = synced_cfg.sync_device_token;
                        }
                        save_config(&this.config);
                        window.push_notification(
                            Notification::success("☁️ 检测到修改，已自动同步到云端"),
                            cx,
                        );
                    }
                    Err(e) => {
                        window.push_notification(
                            Notification::warning(format!("⚠️ 自动同步未成功: {e}")),
                            cx,
                        );
                    }
                }
                if this.has_pending_auto_sync {
                    this.has_pending_auto_sync = false;
                    this.trigger_auto_sync(window, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// 打开飞书绑定弹窗并生成 6 位验证码。
    fn request_bind_code_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match request_cloud_bind_code(&mut self.config) {
            Ok((code, expires)) => {
                self.cloud_bind_code = Some(code);
                self.cloud_bind_expires = Some(expires);
                self.cloud_bind_modal_open = true;
            }
            Err(e) => {
                window.push_notification(Notification::error(e), cx);
            }
        }
        cx.notify();
    }

    /// 检查飞书绑定状态。
    fn check_bind_status_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match check_cloud_bind_status(&mut self.config) {
            Ok(bound) => {
                if bound {
                    self.cloud_bind_modal_open = false;
                    let name = self.config.feishu_user_name.as_deref().unwrap_or("飞书用户");
                    window.push_notification(
                        Notification::success(format!("🎉 飞书绑定成功！已连接到 {name}")),
                        cx,
                    );
                    self.trigger_auto_sync(window, cx);
                } else {
                    window.push_notification(
                        Notification::info("尚未检测到绑定消息，请先在飞书聊天框向机器人发送 /bind <验证码>"),
                        cx,
                    );
                }
            }
            Err(e) => {
                window.push_notification(Notification::error(e), cx);
            }
        }
        cx.notify();
    }

    // -----------------------------------------------------------------------
    // 搜索历史
    // -----------------------------------------------------------------------

    /// 点击历史条目：切到对应来源并回填链接输入框（不自动发起请求）。
    fn use_history_entry(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.config.history.get(index) else {
            return;
        };
        let entry = entry.clone();
        let source = if entry.source == "jellyfin" {
            SourceMode::Jellyfin
        } else {
            SourceMode::Bilibili
        };
        if source != self.source {
            self.switch_source(source, window, cx);
        }
        self.link_input.update(cx, |state, cx| {
            state.set_value(entry.input.clone(), window, cx)
        });
        cx.notify();
    }

    /// 删除单条历史。
    fn remove_history_entry(&mut self, index: usize, cx: &mut Context<Self>) {
        remove_history(&mut self.config, index);
        save_config(&self.config);
        cx.notify();
    }

    /// 一键清空历史。
    fn clear_all_history(&mut self, cx: &mut Context<Self>) {
        clear_history(&mut self.config);
        save_config(&self.config);
        cx.notify();
    }

    /// 历史记录卡片（无历史时不渲染）。
    fn render_history_card(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if self.config.history.is_empty() {
            return None;
        }
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;

        // 条带右侧的「清空」动作：直角描边小块，野兽风按钮语言。
        let clear = div()
            .id("history-clear")
            .px_2()
            .py_0p5()
            .text_size(px(11.))
            .font_weight(FontWeight::BOLD)
            .text_color(hsla(0.0, 0.0, 0.04, 1.0))
            .border_2()
            .border_color(hsla(0.0, 0.0, 0.04, 1.0))
            .hover(|s| s.bg(hsla(0.0, 0.0, 0.04, 0.12)))
            .child("清空")
            .on_click(cx.listener(|this, _, _, cx| this.clear_all_history(cx)));

        let rows: Vec<gpui::AnyElement> = self
            .config
            .history
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let (icon_path, source_label) = if h.source == "jellyfin" {
                    ("icons/film.svg", "JF")
                } else {
                    ("icons/tv.svg", "B站")
                };
                let display = if h.title.is_empty() {
                    h.input.clone()
                } else {
                    h.title.clone()
                };
                // 行内不挂 on_click：点击区（图标+文本）与删除按钮做兄弟节点，
                // 避免 gpui hitbox 不遮挡导致父子双触发。
                h_flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .child(
                        div()
                            .id(("history-item", i))
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_1()
                            .py_1()
                            .rounded_none()
                            .hover(|s| s.bg(theme.list_hover))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.use_history_entry(i, window, cx);
                            }))
                            .child(
                                Icon::empty()
                                    .path(icon_path.to_string())
                                    .size_3p5()
                                    .flex_shrink_0()
                                    .text_color(muted),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(10.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(muted)
                                    .child(source_label),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(12.5))
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .child(display),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(10.5))
                                    .text_color(muted)
                                    .child(ago_label(h.at)),
                            ),
                    )
                    .child(
                        div()
                            .id(("history-del", i))
                            .flex_shrink_0()
                            .p_1()
                            .rounded_none()
                            .hover(|s| s.bg(theme.danger.opacity(0.15)))
                            .child(
                                Icon::empty()
                                    .path("icons/close.svg")
                                    .size_3p5()
                                    .text_color(muted),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remove_history_entry(i, cx);
                            })),
                    )
                    .into_any_element()
            })
            .collect();

        Some(
            entrance(
                "anim-history",
                0.06,
                bcard(cx)
                    .child(
                        section_band("历史记录", "icons/clock.svg", cx)
                            .child(div().flex_1())
                            .child(clear),
                    )
                    .children(rows),
            )
            .into_any_element(),
        )
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
                Ok(path) => {
                    window.push_notification(
                        Notification::success(format!("已导出：{}", path.display())),
                        cx,
                    );
                }
                Err(e) if !e.is_empty() => {
                    window.push_notification(Notification::error(e), cx);
                }
                _ => {}
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

        let today_tasks = get_tasks_for_date(&self.config.plans, &today_date_str());
        let uncompleted_today = today_tasks.iter().filter(|t| !t.task.completed).count();
        let active_plans_count = self
            .config
            .plans
            .iter()
            .filter(|p| p.status == PlanStatus::Active)
            .count();

        let tab_btn = |tab: AppTab, label_text: &'static str, icon_path: &'static str, badge: Option<usize>, cx: &mut Context<Self>| {
            let active = self.active_tab == tab;
            let theme = cx.theme();
            let dark = theme.is_dark();
            let active_bg = if dark {
                theme.primary
            } else {
                hsla(0.135, 1.0, 0.5, 1.0)
            };
            let text_color = if active {
                hsla(0.0, 0.0, 0.04, 1.0)
            } else {
                theme.foreground
            };

            h_flex()
                .id(("tab-btn", tab as usize))
                .items_center()
                .gap_1p5()
                .px_3()
                .py_1()
                .cursor_pointer()
                .rounded_none()
                .border_2()
                .border_color(if active { theme.foreground } else { theme.border.opacity(0.5) })
                .bg(if active { active_bg } else { theme.background.opacity(0.3) })
                .hover(move |s| {
                    if !active {
                        s.bg(theme.accent.opacity(0.15))
                    } else {
                        s
                    }
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.active_tab = tab;
                    cx.notify();
                }))
                .child(
                    Icon::empty()
                        .path(icon_path.to_string())
                        .size_3p5()
                        .text_color(text_color),
                )
                .child(
                    div()
                        .text_size(px(12.5))
                        .font_weight(if active { FontWeight::BOLD } else { FontWeight::MEDIUM })
                        .text_color(text_color)
                        .child(label_text),
                )
                .children(badge.and_then(|count| {
                    if count > 0 {
                        Some(
                            div()
                                .px_1p5()
                                .py_0p5()
                                .text_size(px(10.))
                                .font_weight(FontWeight::BOLD)
                                .rounded_full()
                                .bg(if active { hsla(0.0, 0.0, 0.04, 0.8) } else { theme.danger })
                                .text_color(hsla(0.0, 0.0, 1.0, 1.0))
                                .child(count.to_string()),
                        )
                    } else {
                        None
                    }
                }))
        };

        let tabs = h_flex()
            .gap_2()
            .items_center()
            .child(tab_btn(AppTab::TodayCheckIn, "今日打卡", "icons/square-check.svg", (uncompleted_today > 0).then_some(uncompleted_today), cx))
            .child(tab_btn(AppTab::Calendar, "学习日历", "icons/calendar-days.svg", None, cx))
            .child(tab_btn(AppTab::PlanGenerator, "计划生成器", "icons/film.svg", None, cx))
            .child(tab_btn(AppTab::MyPlans, "我的计划库", "icons/table.svg", Some(active_plans_count), cx));

        let auto_sync_btn = Button::new("auto-sync-btn")
            .small()
            .ghost()
            .label(if self.config.auto_sync { "⚡ 自动同步: 开" } else { "⚡ 自动同步: 关" })
            .on_click(cx.listener(|this, _, window, cx| {
                this.config.auto_sync = !this.config.auto_sync;
                save_config(&this.config);
                let state_str = if this.config.auto_sync { "开启" } else { "关闭" };
                window.push_notification(
                    Notification::info(format!("已{}检测到修改自动同步功能", state_str)),
                    cx,
                );
                if this.config.auto_sync {
                    this.trigger_auto_sync(window, cx);
                }
                cx.notify();
            }));

        let bind_btn = if self.config.feishu_bound || self.config.telegram_bound {
            let mut labels = Vec::new();
            if self.config.feishu_bound {
                labels.push(format!("飞书:{}", self.config.feishu_user_name.as_deref().unwrap_or("已连")));
            }
            if self.config.telegram_bound {
                labels.push(format!("TG:{}", self.config.telegram_user_name.as_deref().unwrap_or("已连")));
            }
            let label = format!("📱 {}", labels.join(" | "));
            Button::new("bot-status-btn")
                .small()
                .ghost()
                .label(label)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.request_bind_code_action(window, cx);
                }))
        } else {
            Button::new("bot-bind-btn")
                .small()
                .primary()
                .label("📱 绑定机器人")
                .on_click(cx.listener(|this, _, window, cx| {
                    this.request_bind_code_action(window, cx);
                }))
        };

        let sync_btn = Button::new("sync-btn")
            .small()
            .ghost()
            .icon(Icon::empty().path("icons/refresh-cw.svg"))
            .label(if self.cloud_syncing { "同步中…" } else { "云端同步" })
            .disabled(self.cloud_syncing)
            .on_click(cx.listener(|this, _, window, cx| {
                this.sync_cloud_action(window, cx);
            }));

        let right_actions = h_flex()
            .gap_2()
            .items_center()
            .child(auto_sync_btn)
            .child(bind_btn)
            .child(sync_btn)
            .child(toggle);

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
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::BOLD)
                            .child("BILI-PLANNER"),
                    ),
            )
            .child(div().flex_1().flex().justify_center().child(tabs))
            .child(right_actions)
    }

    /// Hero 区：超大标题 + 关键词高亮块（野兽风海报语言）。
    fn render_hero(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = cx.theme();
        let dark = theme.is_dark();
        let ink = theme.foreground;

        // 高亮底块：亮=明黄 / 暗=荧光黄，文字始终用墨黑。
        let hl_bg = if dark {
            theme.primary
        } else {
            hsla(0.135, 1.0, 0.5, 1.0)
        };
        let hl = |text: &str| {
            div()
                .px_2()
                .py_0p5()
                .bg(hl_bg)
                .text_color(hsla(0.0, 0.0, 0.04, 1.0))
                .child(text.to_string())
        };

        v_flex()
            .gap_2()
            .pt_2()
            .pb_1()
            .child(
                h_flex()
                    .gap_3()
                    .items_end()
                    .flex_wrap()
                    .child(
                        div()
                            .text_size(px(40.))
                            .font_weight(FontWeight::BLACK)
                            .text_color(ink)
                            .child("合集观看"),
                    )
                    .child(
                        div()
                            .text_size(px(40.))
                            .font_weight(FontWeight::BLACK)
                            .child(hl("计划")),
                    )
                    .child(
                        div()
                            .text_size(px(40.))
                            .font_weight(FontWeight::BLACK)
                            .text_color(ink)
                            .child("生成器"),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(hl("BILIBILI"))
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.muted_foreground)
                            .child("×"),
                    )
                    .child(hl("JELLYFIN"))
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(theme.muted_foreground)
                            .child("— 粘贴链接，按目标天数自动排期"),
                    ),
            )
    }

    /// 来源切换用的成组按钮（gpui-component 无 segmented 控件）。
    fn source_toggle(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        h_flex()
            .gap_1()
            .child(
                Button::new("source-bilibili")
                    .small()
                    .icon(Icon::empty().path("icons/tv.svg").size_4())
                    .label("B 站")
                    .when(self.source == SourceMode::Bilibili, |b| b.primary())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.switch_source(SourceMode::Bilibili, window, cx);
                    })),
            )
            .child(
                Button::new("source-jellyfin")
                    .small()
                    .icon(Icon::empty().path("icons/film.svg").size_4())
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
                    .icon(Icon::empty().path("icons/scissors.svg").size_4())
                    .label("split 精确切分")
                    .when(self.mode == Mode::Split, |b| b.primary())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.switch_mode(Mode::Split, window, cx);
                    })),
            )
            .child(
                Button::new("mode-whole")
                    .small()
                    .icon(Icon::empty().path("icons/square-check.svg").size_4())
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
            .child(
                Label::new(label.to_string())
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD),
            )
            .child(
                Label::new(help.to_string())
                    .text_size(px(12.))
                    .text_color(cx.theme().muted_foreground),
            )
    }

    fn render_form_card(&self, cx: &mut Context<Self>) -> gpui::Div {
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

        bcard(cx)
            .child(section_band("01 · 数据来源", "icons/link.svg", cx))
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
                            .child(
                                Label::new("计划模式")
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::SEMIBOLD),
                            )
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
                    .child(Input::new(&self.cookie_input).cleanable(true))
                    .into_any_element()],
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

    /// 左栏：合集信息卡 + 科目选择 + 操作行。
    fn render_ready_left(&self, rd: &ReadyState, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
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
                .child(
                    Label::new(format!("合集：《{}》", rd.season_title))
                        .text_size(px(15.))
                        .font_weight(FontWeight::BOLD),
                )
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
        out.push(
            entrance(
                "anim-info",
                0.18,
                bcard(cx)
                    .child(section_band("02 · 合集信息", "icons/info.svg", cx))
                    .children(info),
            )
            .into_any_element(),
        );

        // 多科目选择
        if rd.groups.len() > 1 {
            let dark = cx.theme().is_dark();
            let is_all_selected = matches!(rd.selection, Selection::All);
            let all_total_eps: usize = rd.groups.iter().map(|g| g.episodes.len()).sum();
            let all_total_dur: i64 = rd
                .groups
                .iter()
                .flat_map(|g| g.episodes.iter())
                .map(|e| e.duration)
                .sum();

            let mut items: Vec<gpui::AnyElement> = Vec::new();

            // 1. 全部科目
            items.push(
                Self::render_subject_item(
                    "sel-all",
                    is_all_selected,
                    "整个合集（全部科目）".to_string(),
                    Some(format!(
                        "{} 个视频 · {}",
                        all_total_eps,
                        fmt_seconds(all_total_dur as f64, true)
                    )),
                    &theme,
                    dark,
                    cx.listener(|this, _, window, cx| {
                        this.set_selection(Selection::All, window, cx);
                    }),
                )
                .into_any_element(),
            );

            // 2. 各单科目
            for (i, g) in rd.groups.iter().enumerate() {
                let is_selected = matches!(rd.selection, Selection::Single(ix) if ix == i);
                let total: i64 = g.episodes.iter().map(|e| e.duration).sum();
                items.push(
                    Self::render_subject_item(
                        ("sel-group", i),
                        is_selected,
                        format!("{}. {}", i + 1, g.name),
                        Some(format!(
                            "{} 个视频 · {}",
                            g.episodes.len(),
                            fmt_seconds(total as f64, true)
                        )),
                        &theme,
                        dark,
                        cx.listener(move |this, _, window, cx| {
                            this.set_selection(Selection::Single(i), window, cx);
                        }),
                    )
                    .into_any_element(),
                );
            }

            out.push(
                entrance(
                    "anim-groups",
                    0.26,
                    bcard(cx)
                        .child(section_band("03 · 科目选择", "icons/filter.svg", cx))
                        .child(v_flex().w_full().min_w_0().gap_2().children(items)),
                )
                .into_any_element(),
            );
        }

        // 操作行
        let has_plan = rd.plan.is_some();
        out.push(
            entrance(
                "anim-actions",
                0.34,
                bcard(cx)
                    .child(section_band("04 · 计划操作", "icons/calendar-days.svg", cx))
                    .child(
                        h_flex()
                            .gap_3()
                            .child(
                                Button::new("generate")
                                    .primary()
                                    .icon(Icon::empty().path("icons/calendar-days.svg"))
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
                            ),
                    ),
            )
            .into_any_element(),
        );

        // 05 · 加入打卡计划（生成计划后呈现）
        if has_plan {
            let skip_weekends = self.skip_weekends_toggle;
            out.push(
                entrance(
                    "anim-enroll",
                    0.42,
                    bcard(cx)
                        .child(section_band("05 · 开启进度打卡", "icons/square-check.svg", cx))
                        .child(
                            v_flex()
                                .gap_3()
                                .child(
                                    h_flex()
                                        .gap_3()
                                        .items_end()
                                        .child(
                                            v_flex()
                                                .gap_1()
                                                .flex_1()
                                                .child(Self::field_label("起始学习日期", "YYYY-MM-DD，默认今天", cx))
                                                .child(Input::new(&self.start_date_input)),
                                        )
                                        .child(
                                            Button::new("toggle-weekend")
                                                .label(if skip_weekends { "跳过周末：是 ✅" } else { "跳过周末：否 ⬜" })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.skip_weekends_toggle = !this.skip_weekends_toggle;
                                                    cx.notify();
                                                })),
                                        ),
                                )
                                .child(
                                    Button::new("enroll-btn")
                                        .primary()
                                        .icon(Icon::empty().path("icons/square-check.svg"))
                                        .label("🚀 加入每日学习打卡并持久化")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.enroll_current_plan(window, cx);
                                        })),
                                ),
                        ),
                )
                .into_any_element(),
            );
        }

        out
    }

    /// 右栏：每日计划面板（独立滚动，高度撑满窗口）。
    fn render_plan_panel(&self, rd: &ReadyState, cx: &mut Context<Self>) -> gpui::Div {
        let theme = cx.theme().clone();
        let table_card = |content: gpui::AnyElement| {
            bcard(cx)
                .flex_1()
                .min_h_0()
                .min_w_0()
                .child(section_band("05 · 每日计划", "icons/table.svg", cx))
                .child(content)
        };
        match (&self.plan_table, &rd.plan) {
            (Some(table), _) => table_card(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .rounded_none()
                    .border_2()
                    .border_color(theme.foreground)
                    .overflow_hidden()
                    .child(Table::new(table).stripe(true))
                    .into_any_element(),
            ),
            (None, Some(_)) => table_card(
                Label::new("计划已失效，请点击「生成观看计划」重新生成。")
                    .text_size(px(13.))
                    .text_color(theme.muted_foreground)
                    .into_any_element(),
            ),
            (None, None) => table_card(
                v_flex()
                    .gap_2()
                    .items_center()
                    .py_8()
                    .child(
                        Icon::empty()
                            .path("icons/calendar-days.svg")
                            .size_8()
                            .text_color(theme.muted_foreground),
                    )
                    .child(
                        Label::new("填写目标天数后点击「生成观看计划」。")
                            .text_size(px(13.))
                            .text_color(theme.muted_foreground),
                    )
                    .into_any_element(),
            ),
        }
    }

    fn render_loading(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        bcard(cx).child(
            h_flex()
                .gap_2()
                .items_center()
                .child(gpui_component::spinner::Spinner::new().small())
                .child(Label::new("正在获取视频信息…").text_size(px(13.))),
        )
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

    /// 渲染科目选择的单选项（野兽风视觉：硬边框、高亮背景、单选指示器与右侧时长徽标）。
    fn render_subject_item(
        id: impl Into<gpui::ElementId>,
        selected: bool,
        title: String,
        badge: Option<String>,
        theme: &gpui_component::ThemeColor,
        dark: bool,
        on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let active_bg = if dark {
            theme.primary.opacity(0.18)
        } else {
            theme.primary.opacity(0.12)
        };
        let hover_bg = if dark {
            theme.accent.opacity(0.10)
        } else {
            theme.accent.opacity(0.08)
        };

        h_flex()
            .id(id)
            .w_full()
            .min_w_0()
            .items_center()
            .gap_3()
            .px_3()
            .py_2()
            .rounded_none()
            .border_2()
            .border_color(if selected {
                theme.primary
            } else {
                theme.border
            })
            .bg(if selected {
                active_bg
            } else {
                theme.background.opacity(0.35)
            })
            .cursor_pointer()
            .hover(move |s| {
                if !selected {
                    s.bg(hover_bg)
                } else {
                    s
                }
            })
            .on_click(move |event, window, cx| on_click(event, window, cx))
            .child(
                // 单选指示器圆圈
                div()
                    .flex_shrink_0()
                    .size(px(16.))
                    .rounded_full()
                    .border_2()
                    .border_color(if selected {
                        theme.primary
                    } else {
                        theme.muted_foreground
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(selected, |d| {
                        d.child(
                            div()
                                .size(px(8.))
                                .rounded_full()
                                .bg(theme.primary),
                        )
                    }),
            )
            .child(
                // 科目标题
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(13.))
                    .font_weight(if selected {
                        FontWeight::BOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .text_color(theme.foreground)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(title),
            )
            .children(badge.map(|b| {
                div()
                    .flex_shrink_0()
                    .text_size(px(11.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.muted_foreground)
                    .px_2()
                    .py_0p5()
                    .bg(theme.muted.opacity(0.5))
                    .rounded_none()
                    .border_1()
                    .border_color(theme.border.opacity(0.4))
                    .child(b)
            }))
    }
    /// 渲染计划生成器标签页（左侧配置 + 右侧计划表）。
    fn render_plan_generator_view(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme().clone();

        // 左栏：hero + 表单 + （就绪后）信息/科目/操作/打卡，独立滚动。
        let mut left = v_flex()
            .id("content-scroll")
            .h_full()
            .flex_1()
            .min_w_0()
            .overflow_y_scroll()
            .px_6()
            .py_5()
            .gap_5()
            .child(entrance("anim-hero", 0., self.render_hero(cx)))
            .child(entrance("anim-form", 0.12, self.render_form_card(cx)))
            .children(self.render_history_card(cx));

        if let Some(err) = &self.last_error {
            left = left.child(
                Alert::error("fetch-error", err.clone())
                    .title("获取失败")
                    .flex_shrink_0(),
            );
        }

        match &self.phase {
            Phase::Loading => left = left.child(self.render_loading(cx)),
            Phase::Ready(rd) => {
                left = left.children(self.render_ready_left(rd, cx));
            }
            Phase::Input => {}
        }

        // 右栏：就绪后展开的计划面板，独立滚动、高度撑满窗口。
        if let Phase::Ready(rd) = &self.phase {
            h_resizable("main-splitter")
                .child(
                    resizable_panel()
                        .size_range(px(360.)..px(1800.))
                        .child(left),
                )
                .child(
                    resizable_panel()
                        .size(PLAN_PANEL_WIDTH)
                        .size_range(px(380.)..px(2400.))
                        .child(
                            v_flex()
                                .size_full()
                                .min_h_0()
                                .min_w_0()
                                .border_l_2()
                                .border_color(theme.foreground)
                                .bg(theme.background.opacity(0.72))
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_h_0()
                                        .min_w_0()
                                        .px_5()
                                        .py_4()
                                        .child(entrance(
                                            "anim-plan",
                                            0.15,
                                            self.render_plan_panel(rd, cx),
                                        )),
                                    ),
                        ),
                )
                .into_any_element()
        } else {
            h_flex().flex_1().min_h_0().child(left).into_any_element()
        }
    }

    /// 渲染今日打卡板块（多科目叠加看板与任务打卡）。
    fn render_today_checkin_view(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme().clone();
        let stats = compute_study_stats(&self.config.plans, &self.selected_date);

        // 1. 统计概览卡片 (Neo-Brutalist 三列硬阴影卡片)
        let stats_row = h_flex()
            .w_full()
            .gap_4()
            .child(
                bcard(cx)
                    .flex_1()
                    .gap_1()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(Icon::empty().path("icons/square-check.svg").size_4().text_color(theme.primary))
                            .child(Label::new("今日任务").text_size(px(13.)).font_weight(FontWeight::BOLD)),
                    )
                    .child(
                        h_flex()
                            .items_baseline()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(26.))
                                    .font_weight(FontWeight::BLACK)
                                    .child(format!("{}/{}", stats.today_completed_tasks, stats.today_total_tasks)),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme.muted_foreground)
                                    .child(if stats.today_total_tasks > 0 {
                                        format!("完成率 {:.0}%", (stats.today_completed_tasks as f64 / stats.today_total_tasks as f64) * 100.0)
                                    } else {
                                        "无安排".to_string()
                                    }),
                            ),
                    ),
            )
            .child(
                bcard(cx)
                    .flex_1()
                    .gap_1()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(Icon::empty().path("icons/clock.svg").size_4().text_color(theme.primary))
                            .child(Label::new("今日学习时长").text_size(px(13.)).font_weight(FontWeight::BOLD)),
                    )
                    .child(
                        h_flex()
                            .items_baseline()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(22.))
                                    .font_weight(FontWeight::BLACK)
                                    .child(fmt_seconds(stats.today_total_duration as f64, true)),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme.muted_foreground)
                                    .child(format!("已学 {}", fmt_seconds(stats.today_completed_duration as f64, true))),
                            ),
                    ),
            )
            .child(
                bcard(cx)
                    .flex_1()
                    .gap_1()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(Icon::empty().path("icons/calendar-days.svg").size_4().text_color(theme.primary))
                            .child(Label::new("连续学习 Streak").text_size(px(13.)).font_weight(FontWeight::BOLD)),
                    )
                    .child(
                        h_flex()
                            .items_baseline()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(26.))
                                    .font_weight(FontWeight::BLACK)
                                    .child(format!("🔥 {} 天", stats.current_streak)),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme.muted_foreground)
                                    .child(format!("累计打卡 {} 天", stats.total_days_checked_in)),
                            ),
                    ),
            );

        // 2. 日期选择导航条
        let is_today = self.selected_date == today_date_str();
        let date_nav = bcard(cx)
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_3()
                            .child(
                                Button::new("prev-date")
                                    .label("◀ 前一天")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.prev_date_action(cx);
                                    })),
                            )
                            .child(
                                div()
                                    .text_size(px(16.))
                                    .font_weight(FontWeight::BOLD)
                                    .child(format_date_with_weekday(&self.selected_date)),
                            )
                            .child(
                                Button::new("next-date")
                                    .label("后一天 ▶")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.next_date_action(cx);
                                    })),
                            )
                            .children((!is_today).then(|| {
                                Button::new("back-today")
                                    .primary()
                                    .label("回到今天")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.reset_today_action(cx);
                                    }))
                            })),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("push-forward-all")
                                    .icon(Icon::empty().path("icons/refresh-cw.svg"))
                                    .label("一键顺延落后计划")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.push_forward_all_behind_action(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("add-plan-quick")
                                    .primary()
                                    .label("➕ 添加新科目")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.active_tab = AppTab::PlanGenerator;
                                        cx.notify();
                                    })),
                            ),
                    ),
            );

        // 3. 多科目任务聚合
        let all_tasks = get_tasks_for_date(&self.config.plans, &self.selected_date);

        let filtered_tasks: Vec<_> = if let Some(fid) = &self.filter_plan_id {
            all_tasks.into_iter().filter(|t| &t.plan_id == fid).collect()
        } else {
            all_tasks
        };

        // 科目过滤标签
        let active_plans: Vec<_> = self.config.plans.iter().filter(|p| p.status == PlanStatus::Active).collect();
        let filter_bar = if active_plans.len() > 1 {
            let mut buttons = Vec::new();
            let is_all = self.filter_plan_id.is_none();
            buttons.push(
                Button::new("filter-all")
                    .label("全部科目")
                    .selected(is_all)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.filter_plan_id = None;
                        cx.notify();
                    }))
                    .into_any_element(),
            );
            for (idx, p) in active_plans.iter().enumerate() {
                let pid = p.id.clone();
                let is_sel = self.filter_plan_id.as_deref() == Some(&pid);
                let title = p.title.clone();
                buttons.push(
                    Button::new(("filter-p", idx))
                        .label(title)
                        .selected(is_sel)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.filter_plan_id = Some(pid.clone());
                            cx.notify();
                        }))
                        .into_any_element(),
                );
            }
            Some(h_flex().gap_2().items_center().children(buttons))
        } else {
            None
        };

        // 任务列表分组渲染
        let task_content: gpui::Div = if self.config.plans.is_empty() {
            bcard(cx)
                .items_center()
                .py_10()
                .gap_3()
                .child(
                    Icon::empty()
                        .path("icons/calendar-days.svg")
                        .size_10()
                        .text_color(theme.muted_foreground),
                )
                .child(
                    div()
                        .text_size(px(16.))
                        .font_weight(FontWeight::BOLD)
                        .child("暂无进行中的学习计划"),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(theme.muted_foreground)
                        .child("请前往「计划生成器」输入 B 站或 Jellyfin 链接创建您的第一个学习打卡计划！"),
                )
                .child(
                    Button::new("go-generator")
                        .primary()
                        .label("🚀 前往计划生成器")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.active_tab = AppTab::PlanGenerator;
                            cx.notify();
                        })),
                )
        } else if filtered_tasks.is_empty() {
            bcard(cx)
                .items_center()
                .py_10()
                .gap_3()
                .child(
                    Icon::empty()
                        .path("icons/square-check.svg")
                        .size_10()
                        .text_color(theme.primary),
                )
                .child(
                    div()
                        .text_size(px(16.))
                        .font_weight(FontWeight::BOLD)
                        .child("🎉 今日无学习任务安排或为休息日！"),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(theme.muted_foreground)
                        .child("适当休息劳逸结合，保持最佳学习状态。"),
                )
        } else {
            // 按科目分组展示
            let mut plan_map: std::collections::BTreeMap<String, Vec<crate::study::TodayTaskView>> = std::collections::BTreeMap::new();
            for item in filtered_tasks {
                plan_map.entry(item.plan_id.clone()).or_default().push(item);
            }

            let mut group_cards = Vec::new();
            for (grp_idx, (plan_id, items)) in plan_map.into_iter().enumerate() {
                let plan_title = items[0].plan_title.clone();
                let day_display = items[0].day_display.clone();
                let source_type = items[0].source_type.clone();
                let source_url = items[0].source_url.clone();
                let total_in_group = items.len();
                let done_in_group = items.iter().filter(|t| t.task.completed).count();
                let is_all_group_done = total_in_group > 0 && done_in_group == total_in_group;

                let pid_clone = plan_id.clone();
                let sel_date_clone = self.selected_date.clone();

                let mut task_rows = Vec::new();
                for (item_idx, view) in items.into_iter().enumerate() {
                    let task = view.task;
                    let tid = task.id.clone();
                    let st = source_type.clone();
                    let su = source_url.clone();
                    let vno = task.vid_no;
                    let is_done = task.completed;

                    let (chk_icon, chk_color) = if is_done {
                        ("icons/square-check.svg", theme.primary)
                    } else {
                        ("icons/square.svg", theme.muted_foreground)
                    };

                    let btn_play_id = ("play-task", grp_idx * 1000 + item_idx);
                    let btn_chk_id = ("check-btn", grp_idx * 1000 + item_idx);
                    let pid_click1 = plan_id.clone();
                    let pid_click2 = plan_id.clone();
                    let tid_click1 = tid.clone();
                    let tid_click2 = tid.clone();

                    task_rows.push(
                        h_flex()
                            .id(("task-row", grp_idx * 1000 + item_idx))
                            .items_center()
                            .justify_between()
                            .px_3()
                            .py_2p5()
                            .border_1()
                            .border_color(if is_done { theme.primary.opacity(0.4) } else { theme.border.opacity(0.6) })
                            .bg(if is_done { theme.primary.opacity(0.08) } else { theme.background.opacity(0.25) })
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_3()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .id(("chk-icon", grp_idx * 1000 + item_idx))
                                            .cursor_pointer()
                                            .p_1()
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.toggle_task_checkin_action(&pid_click1, &tid_click1, window, cx);
                                            }))
                                            .child(
                                                Icon::empty()
                                                    .path(chk_icon)
                                                    .size_5()
                                                    .text_color(chk_color),
                                            ),
                                    )
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .gap_0p5()
                                            .child(
                                                h_flex()
                                                    .items_center()
                                                    .gap_2()
                                                    .child(
                                                        div()
                                                            .text_size(px(13.5))
                                                            .font_weight(FontWeight::BOLD)
                                                            .text_color(if is_done { theme.muted_foreground } else { theme.foreground })
                                                            .child(format!("P{}: {}", task.vid_no, task.title)),
                                                    )
                                                    .children(task.from_prev.then(|| {
                                                        div()
                                                            .text_size(px(10.5))
                                                            .px_1p5()
                                                            .py_0p5()
                                                            .bg(theme.accent.opacity(0.2))
                                                            .child("接上一日")
                                                    }))
                                                    .children((task.remainder > 0).then(|| {
                                                        div()
                                                            .text_size(px(10.5))
                                                            .px_1p5()
                                                            .py_0p5()
                                                            .bg(theme.primary.opacity(0.2))
                                                            .child("顺延至次日")
                                                    })),
                                            )
                                            .child(
                                                h_flex()
                                                    .gap_3()
                                                    .items_center()
                                                    .child(
                                                        div()
                                                            .text_size(px(11.5))
                                                            .text_color(theme.muted_foreground)
                                                            .child(format!("⏱️ 本日任务时长：{}", fmt_seconds(task.portion as f64, true))),
                                                    )
                                                    .children((task.remainder > 0).then(|| {
                                                        div()
                                                            .text_size(px(11.5))
                                                            .text_color(theme.muted_foreground)
                                                            .child(format!("剩余顺延：{}", fmt_seconds(task.remainder as f64, true)))
                                                    })),
                                            ),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        Button::new(btn_play_id)
                                            .icon(Icon::empty().path("icons/play.svg"))
                                            .label("直达播放")
                                            .small()
                                            .on_click(move |_, _, _| {
                                                open_video_link(&st, &su, vno);
                                            }),
                                    )
                                    .child(
                                        Button::new(btn_chk_id)
                                            .small()
                                            .label(if is_done { "已打卡" } else { "打卡" })
                                            .primary()
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.toggle_task_checkin_action(&pid_click2, &tid_click2, window, cx);
                                            })),
                                    ),
                            )
                            .into_any_element(),
                    );
                }

                group_cards.push(
                    bcard(cx)
                        .child(
                            h_flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    h_flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            Icon::empty()
                                                .path(if source_type == "jellyfin" { "icons/film.svg" } else { "icons/tv.svg" })
                                                .size_4()
                                                .text_color(theme.primary),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(15.))
                                                .font_weight(FontWeight::BOLD)
                                                .child(format!("《{plan_title}》 · {day_display}")),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(11.5))
                                                .px_2()
                                                .py_0p5()
                                                .bg(if is_all_group_done { theme.primary } else { theme.muted })
                                                .text_color(if is_all_group_done { hsla(0.0, 0.0, 0.04, 1.0) } else { theme.muted_foreground })
                                                .child(format!("完成 {done_in_group}/{total_in_group}")),
                                        ),
                                )
                                .children((!is_all_group_done).then(|| {
                                    Button::new(("check-all-grp", grp_idx))
                                        .small()
                                        .label("一键打卡本科目今日")
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.checkin_entire_day_action(&pid_clone, &sel_date_clone, window, cx);
                                        }))
                                })),
                        )
                        .child(v_flex().w_full().gap_2().children(task_rows))
                        .into_any_element(),
                );
            }

            v_flex().w_full().gap_4().children(group_cards)
        };

        v_flex()
            .id("today-scroll")
            .size_full()
            .overflow_y_scroll()
            .px_8()
            .py_6()
            .gap_5()
            .child(entrance("anim-stats", 0.0, stats_row))
            .child(entrance("anim-nav", 0.08, date_nav))
            .children(filter_bar.map(|f| entrance("anim-filter", 0.14, f).into_any_element()))
            .child(entrance("anim-tasks", 0.18, task_content))
            .into_any_element()
    }

    /// 渲染学习日历视图（月度学习看板 + 手动备忘录 + 选中日期右侧明细栏）。
    fn render_calendar_view(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme().clone();
        let year = self.calendar_year;
        let month = self.calendar_month;
        let matrix = generate_month_calendar_matrix(year, month, &self.config.plans);
        let month_stats = compute_month_study_stats(year, month, &self.config.plans);
        let selected_date = self.calendar_selected_date.clone();
        let selected_tasks = get_tasks_for_date(&self.config.plans, &selected_date);

        // 1. 左侧：月度日历看板
        // 1.1 月份导航与本月统计条
        let month_nav_bar = bcard(cx)
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_3()
                            .child(
                                Button::new("cal-prev-month")
                                    .label("◀ 上个月")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.prev_calendar_month_action(cx);
                                    })),
                            )
                            .child(
                                div()
                                    .text_size(px(18.))
                                    .font_weight(FontWeight::BLACK)
                                    .child(format!("{year} 年 {month} 月")),
                            )
                            .child(
                                Button::new("cal-next-month")
                                    .label("下个月 ▶")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.next_calendar_month_action(cx);
                                    })),
                            )
                            .child(
                                Button::new("cal-today-month")
                                    .small()
                                    .primary()
                                    .label("回到本月")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.reset_calendar_month_action(window, cx);
                                    })),
                            ),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_3()
                            .text_size(px(12.5))
                            .text_color(theme.muted_foreground)
                            .child(format!(
                                "⏱️ 规划 {} (已学 {})",
                                fmt_seconds(month_stats.total_duration as f64, true),
                                fmt_seconds(month_stats.completed_duration as f64, true)
                            ))
                            .child(format!("📚 任务 {}/{}", month_stats.completed_tasks, month_stats.total_tasks))
                            .child(format!("🔥 活跃 {} 天", month_stats.active_study_days)),
                    ),
            );

        // 1.2 星期表头 (周一 ~ 周日)
        // 1.2 星期表头 (周一 ~ 周日)
        let weekdays = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];
        let weekday_headers = h_flex()
            .w_full()
            .gap_1p5()
            .children(weekdays.iter().enumerate().map(|(idx, &w)| {
                let is_wkend = idx >= 5;
                div()
                    .flex_1()
                    .min_w_0()
                    .py_1p5()
                    .items_center()
                    .justify_center()
                    .flex()
                    .bg(if is_wkend { theme.primary.opacity(0.12) } else { theme.muted.opacity(0.4) })
                    .border_2()
                    .border_color(theme.border)
                    .text_size(px(12.5))
                    .font_weight(FontWeight::BOLD)
                    .text_color(if is_wkend { theme.primary } else { theme.foreground })
                    .child(w)
            }));

        // 1.3 日历网格主体 (按 7 列分行)
        let mut grid_rows = Vec::new();
        for (row_idx, chunk) in matrix.chunks(7).enumerate() {
            let mut row_cells = Vec::new();
            for (col_idx, day) in chunk.iter().enumerate() {
                let cell_index = row_idx * 7 + col_idx;

                // 若非当前所选月份，仅渲染尺寸完全一致的透明占位格，保证每列宽度严格对齐
                if !day.is_current_month {
                    let placeholder_el = div()
                        .id(("cal-ph", cell_index))
                        .flex_1()
                        .min_w_0()
                        .h(px(104.))
                        .p_1p5()
                        .border_2()
                        .border_color(hsla(0.0, 0.0, 0.0, 0.0));
                    row_cells.push(placeholder_el.into_any_element());
                    continue;
                }

                let d_date = day.date.clone();
                let is_sel = d_date == selected_date;
                let is_today = day.is_today;
                let has_note = self.config.daily_notes.get(&d_date).is_some();
                let note_snippet = self.config.daily_notes.get(&d_date).cloned().unwrap_or_default();

                let bg_color = if is_sel {
                    theme.primary.opacity(0.18)
                } else if is_today {
                    theme.primary.opacity(0.08)
                } else {
                    theme.background.opacity(0.35)
                };

                let border_color = if is_sel {
                    theme.primary
                } else if is_today {
                    theme.primary.opacity(0.6)
                } else {
                    theme.border.opacity(0.6)
                };

                let is_all_done = day.completed_tasks > 0 && day.completed_tasks == day.total_tasks;
                let status_bg = if is_all_done {
                    theme.primary.opacity(0.9)
                } else if day.completed_tasks > 0 {
                    theme.primary.opacity(0.4)
                } else {
                    theme.muted.opacity(0.6)
                };
                let status_text_color = if is_all_done {
                    hsla(0.0, 0.0, 0.04, 1.0)
                } else {
                    theme.foreground
                };

                let cell_el = div()
                    .id(("cal-cell", cell_index))
                    .flex_1()
                    .min_w_0()
                    .h(px(104.))
                    .p_1p5()
                    .gap_1()
                    .flex()
                    .flex_col()
                    .justify_between()
                    .cursor_pointer()
                    .bg(bg_color)
                    .border_2()
                    .border_color(border_color)
                    .overflow_hidden()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_calendar_date_action(&d_date, window, cx);
                    }))
                    .child(
                        // 顶部行：左侧[日期数字 + 今日 + 备忘图标]；右上角[学习时间 + 完成进度]
                        h_flex()
                            .items_center()
                            .justify_between()
                            .w_full()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_size(px(13.5))
                                            .font_weight(if is_today || is_sel { FontWeight::BLACK } else { FontWeight::BOLD })
                                            .text_color(if is_sel { theme.primary } else { theme.foreground })
                                            .child(day.day_num.to_string()),
                                    )
                                    .children(is_today.then(|| {
                                        div()
                                            .px_1()
                                            .py_0p5()
                                            .bg(theme.primary)
                                            .text_color(hsla(0.0, 0.0, 0.04, 1.0))
                                            .text_size(px(9.5))
                                            .font_weight(FontWeight::BOLD)
                                            .child("今日")
                                    }))
                                    .children(has_note.then(|| {
                                        div()
                                            .text_size(px(11.))
                                            .child("📝")
                                    })),
                            )
                            // 右上角统一放置完成进度和学习时间
                            .children(if day.total_tasks > 0 {
                                Some(
                                    h_flex()
                                        .items_center()
                                        .gap_1()
                                        .text_size(px(10.))
                                        .child(
                                            div()
                                                .text_color(theme.muted_foreground)
                                                .child(fmt_seconds(day.total_duration as f64, false)),
                                        )
                                        .child(
                                            div()
                                                .px_1()
                                                .py_0p5()
                                                .bg(status_bg)
                                                .text_color(status_text_color)
                                                .font_weight(FontWeight::BOLD)
                                                .child(format!("{}/{}", day.completed_tasks, day.total_tasks)),
                                        ),
                                )
                            } else if day.is_rest_day {
                                Some(
                                    h_flex()
                                        .items_center()
                                        .child(
                                            div()
                                                .text_size(px(10.))
                                                .text_color(theme.muted_foreground)
                                                .child("☕ 休息日"),
                                        ),
                                )
                            } else {
                                None
                            }),
                    )
                    .child(
                        // 底部课程列表区：每天的课程一行一个依次展示
                        v_flex()
                            .w_full()
                            .gap_0p5()
                            .children(if day.total_tasks > 0 {
                                let mut items = Vec::new();
                                for t in &day.plan_titles {
                                    items.push(
                                        div()
                                            .w_full()
                                            .text_size(px(9.5))
                                            .text_color(theme.muted_foreground)
                                            .whitespace_nowrap()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .child(format!("• {t}"))
                                            .into_any_element(),
                                    );
                                }
                                items
                            } else if has_note {
                                let preview: String = note_snippet.chars().take(7).collect();
                                vec![
                                    div()
                                        .w_full()
                                        .text_size(px(9.5))
                                        .text_color(theme.muted_foreground)
                                        .whitespace_nowrap()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .child(format!("📝 {preview}…"))
                                        .into_any_element(),
                                ]
                            } else {
                                vec![]
                            }),
                    );

                row_cells.push(cell_el.into_any_element());
            }
            grid_rows.push(h_flex().w_full().gap_1p5().h(px(104.)).children(row_cells));
        }

        let calendar_left = v_flex()
            .id("calendar-left-pane")
            .flex_1()
            .h_full()
            .overflow_y_scroll()
            .min_w_0()
            .gap_3()
            .pr_2()
            .child(month_nav_bar)
            .child(
                bcard(cx)
                    .p_3()
                    .gap_2()
                    .child(weekday_headers)
                    .child(v_flex().w_full().gap_1p5().children(grid_rows)),
            );

        // 2. 右侧：选中单日明细与备忘录侧边栏
        let total_day_dur: i64 = selected_tasks.iter().map(|t| t.task.portion).sum();
        let done_day_dur: i64 = selected_tasks.iter().filter(|t| t.task.completed).map(|t| t.task.portion).sum();
        let total_day_tasks = selected_tasks.len();
        let done_day_tasks = selected_tasks.iter().filter(|t| t.task.completed).count();

        let day_detail_card = bcard(cx)
            .gap_2()
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(FontWeight::BOLD)
                            .child(format_date_with_weekday(&selected_date)),
                    )
                    .children((selected_date == today_date_str()).then(|| {
                        div()
                            .px_2()
                            .py_0p5()
                            .bg(theme.primary)
                            .text_color(hsla(0.0, 0.0, 0.04, 1.0))
                            .text_size(px(11.))
                            .font_weight(FontWeight::BOLD)
                            .child("🔥 今日")
                    })),
            )
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(12.5))
                    .text_color(theme.muted_foreground)
                    .child(format!("任务：{done_day_tasks} / {total_day_tasks} 项"))
                    .child(format!(
                        "已学 {} / 共 {}",
                        fmt_seconds(done_day_dur as f64, true),
                        fmt_seconds(total_day_dur as f64, true)
                    )),
            );

        // 备忘录编辑区域
        let note_edit_card = bcard(cx)
            .gap_2p5()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(Icon::empty().path("icons/square-pen.svg").size_4().text_color(theme.primary))
                    .child(
                        div()
                            .text_size(px(13.5))
                            .font_weight(FontWeight::BOLD)
                            .child("当日学习备忘 / 心得"),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .child(Input::new(&self.calendar_note_input)),
            )
            .child(
                h_flex()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("cal-clear-note-btn")
                            .small()
                            .ghost()
                            .label("🗑️ 清空")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.clear_calendar_note_action(window, cx);
                            })),
                    )
                    .child(
                        Button::new("cal-save-note-btn")
                            .small()
                            .primary()
                            .label("💾 保存备注")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_calendar_note_action(window, cx);
                            })),
                    ),
            );

        // 当日具体任务列表
        let tasks_list_card = bcard(cx)
            .gap_2p5()
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(Icon::empty().path("icons/square-check.svg").size_4().text_color(theme.primary))
                            .child(
                                div()
                                    .text_size(px(13.5))
                                    .font_weight(FontWeight::BOLD)
                                    .child(format!("当日学习项目 (共 {total_day_tasks} 项)")),
                            ),
                    ),
            )
            .child(
                if selected_tasks.is_empty() {
                    v_flex()
                        .w_full()
                        .py_8()
                        .items_center()
                        .justify_center()
                        .gap_1p5()
                        .text_color(theme.muted_foreground)
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_weight(FontWeight::MEDIUM)
                                .child("✨ 当日无学习任务安排"),
                        )
                        .child(
                            div()
                                .text_size(px(11.5))
                                .child("可自由复习、预习或休息"),
                        )
                        .into_any_element()
                } else {
                    let mut task_items = Vec::new();
                    for (i, t_item) in selected_tasks.iter().enumerate() {
                        let pid = t_item.plan_id.clone();
                        let tid = t_item.task.id.clone();
                        let is_done = t_item.task.completed;
                        let st = t_item.source_type.clone();
                        let su = t_item.source_url.clone();
                        let vno = t_item.task.vid_no;

                        task_items.push(
                            div()
                                .id(("cal-task", i))
                                .w_full()
                                .p_2()
                                .border_1()
                                .border_color(theme.border)
                                .bg(if is_done { theme.primary.opacity(0.08) } else { theme.background.opacity(0.3) })
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .gap_0p5()
                                        .child(
                                            div()
                                                .text_size(px(12.5))
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(if is_done { theme.muted_foreground } else { theme.foreground })
                                                .whitespace_nowrap()
                                                .overflow_hidden()
                                                .text_ellipsis()
                                                .child(format!("《{}》 P{}: {}", t_item.plan_title, t_item.task.vid_no, t_item.task.title)),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(10.5))
                                                .text_color(theme.muted_foreground)
                                                .child(fmt_seconds(t_item.task.portion as f64, true)),
                                        ),
                                )
                                .child(
                                    h_flex()
                                        .items_center()
                                        .gap_1p5()
                                        .child(
                                            Button::new(("cal-play", i))
                                                .small()
                                                .ghost()
                                                .label("🔗 直达")
                                                .on_click(move |_, _, _| {
                                                    open_video_link(&st, &su, vno);
                                                }),
                                        )
                                        .child(
                                            Button::new(("cal-chk", i))
                                                .small()
                                                .primary()
                                                .label(if is_done { "已完成 ✅" } else { "打卡 ⬜" })
                                                .on_click(cx.listener(move |this, _, window, cx| {
                                                    this.toggle_task_checkin_action(&pid, &tid, window, cx);
                                                })),
                                        ),
                                )
                                .into_any_element(),
                        );
                    }
                    v_flex().w_full().gap_2().children(task_items).into_any_element()
                },
            );

        let calendar_right = v_flex()
            .id("calendar-right-pane")
            .w(px(400.))
            .min_w(px(400.))
            .h_full()
            .overflow_y_scroll()
            .gap_3()
            .pr_1()
            .child(day_detail_card)
            .child(note_edit_card)
            .child(tasks_list_card);

        h_flex()
            .id("calendar-split-view")
            .size_full()
            .px_8()
            .py_6()
            .gap_5()
            .child(calendar_left)
            .child(calendar_right)
            .into_any_element()
    }

    /// 渲染我的计划库标签页。
    fn render_my_plans_view(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme().clone();
        let total_plans = self.config.plans.len();
        let active_plans = self
            .config
            .plans
            .iter()
            .filter(|p| p.status == PlanStatus::Active)
            .count();

        let header = bcard(cx).child(
            h_flex()
                .items_center()
                .justify_between()
                .child(
                    v_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(20.))
                                .font_weight(FontWeight::BLACK)
                                .child("📚 我的学习计划库"),
                        )
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(theme.muted_foreground)
                                .child(format!("共 {total_plans} 门科目 · 进行中 {active_plans} 门")),
                        ),
                )
                .child(
                    Button::new("new-plan-btn")
                        .primary()
                        .label("➕ 设立新计划")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.active_tab = AppTab::PlanGenerator;
                            cx.notify();
                        })),
                ),
        );

        let plan_cards: gpui::Div = if self.config.plans.is_empty() {
            bcard(cx)
                .items_center()
                .py_12()
                .gap_3()
                .child(
                    Icon::empty()
                        .path("icons/table.svg")
                        .size_10()
                        .text_color(theme.muted_foreground),
                )
                .child(
                    div()
                        .text_size(px(16.))
                        .font_weight(FontWeight::BOLD)
                        .child("暂无任何学习计划"),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(theme.muted_foreground)
                        .child("点击下方按钮立即创建您的第一个多科目学习打卡计划。"),
                )
                .child(
                    Button::new("empty-create")
                        .primary()
                        .label("🚀 前往计划生成器")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.active_tab = AppTab::PlanGenerator;
                            cx.notify();
                        })),
                )
        } else {
            let mut cards = Vec::new();
            for (p_idx, plan) in self.config.plans.iter().enumerate() {
                let (done_cnt, total_cnt, done_dur, total_dur, ratio) = compute_plan_progress(plan);
                let pid = plan.id.clone();
                let pid_del = plan.id.clone();
                let pid_push = plan.id.clone();

                let status_badge_bg = match plan.status {
                    PlanStatus::Active => theme.primary,
                    PlanStatus::Paused => theme.muted,
                    PlanStatus::Completed => theme.success,
                    PlanStatus::Archived => theme.muted,
                };
                let status_badge_text = match plan.status {
                    PlanStatus::Active => hsla(0.0, 0.0, 0.04, 1.0),
                    PlanStatus::Paused => theme.muted_foreground,
                    PlanStatus::Completed => hsla(0.0, 0.0, 1.0, 1.0),
                    PlanStatus::Archived => theme.muted_foreground,
                };

                let card = bcard(cx)
                    .gap_3()
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_size(px(16.))
                                            .font_weight(FontWeight::BOLD)
                                            .child(format!("《{}》", plan.title)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.5))
                                            .font_weight(FontWeight::BOLD)
                                            .px_1p5()
                                            .py_0p5()
                                            .bg(theme.muted)
                                            .child(plan.source_type.to_uppercase()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .font_weight(FontWeight::BOLD)
                                            .px_2()
                                            .py_0p5()
                                            .bg(status_badge_bg)
                                            .text_color(status_badge_text)
                                            .child(plan.status.label()),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        Button::new(("pause-plan", p_idx))
                                            .small()
                                            .label(if plan.status == PlanStatus::Paused { "▶️ 继续" } else { "⏸️ 暂停" })
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.toggle_plan_status_action(&pid, window, cx);
                                            })),
                                    )
                                    .child(
                                        Button::new(("push-single", p_idx))
                                            .small()
                                            .label("🔄 顺延至今日")
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.push_forward_single_plan_action(&pid_push, window, cx);
                                            })),
                                    )
                                    .child(
                                        Button::new(("del-plan", p_idx))
                                            .small()
                                            .label("🗑️ 删除")
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.delete_plan_action(&pid_del, window, cx);
                                            })),
                                    ),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_1p5()
                            .child(
                                h_flex()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_size(px(12.5))
                                            .text_color(theme.muted_foreground)
                                            .child(format!("范围：{} · 排期：{} 至 {}（{} 天 · {}）", plan.scope_desc, plan.start_date, plan.end_date, plan.planned_days, if plan.skip_weekends { "跳过周末" } else { "连续每日" })),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.5))
                                            .font_weight(FontWeight::BOLD)
                                            .child(format!("进度：{done_cnt}/{total_cnt} 视频 · {} / {} ({:.1}%)", fmt_seconds(done_dur as f64, true), fmt_seconds(total_dur as f64, true), ratio * 100.0)),
                                    ),
                            )
                            // 野兽风进度条：墨色边框 + 明黄填充
                            .child(
                                div()
                                    .w_full()
                                    .h(px(12.))
                                    .border_2()
                                    .border_color(theme.foreground)
                                    .bg(theme.background)
                                    .child(
                                        div()
                                            .h_full()
                                            .w(gpui::DefiniteLength::Fraction(ratio as f32))
                                            .bg(theme.primary),
                                    ),
                            ),
                    );

                cards.push(card.into_any_element());
            }

            v_flex().w_full().gap_4().children(cards)
        };

        v_flex()
            .id("myplans-scroll")
            .size_full()
            .overflow_y_scroll()
            .px_8()
            .py_6()
            .gap_5()
            .child(entrance("anim-myplans-head", 0.0, header))
            .child(entrance("anim-myplans-cards", 0.1, plan_cards))
            .into_any_element()
    }

    /// 渲染机器人绑定弹窗（Neo-Brutalist 弹窗 + 醒目大字验证码，支持飞书与 Telegram）。
    fn render_bind_modal(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme().clone();
        let code_str = self.cloud_bind_code.as_deref().unwrap_or("------");

        div()
            .absolute()
            .inset_0()
            .bg(hsla(0.0, 0.0, 0.0, 0.6))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(480.))
                    .bg(theme.background)
                    .border_2()
                    .border_color(theme.foreground)
                    .shadow_lg()
                    .p_6()
                    .gap_4()
                    .flex()
                    .flex_col()
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(Icon::empty().path("icons/tv.svg").size_5().text_color(theme.primary))
                                    .child(div().text_size(px(16.)).font_weight(FontWeight::BOLD).child("📱 绑定飞书 / Telegram 学习助手")),
                            )
                            .child(
                                div()
                                    .id("close-bind-modal")
                                    .cursor_pointer()
                                    .p_1()
                                    .child(Icon::empty().path("icons/square.svg").size_4().text_color(theme.muted_foreground))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cloud_bind_modal_open = false;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(theme.muted_foreground)
                            .child("在飞书机器人 或 Telegram 机器人聊天窗口中，发送以下指令完成绑定："),
                    )
                    .child(
                        div()
                            .py_3()
                            .px_4()
                            .bg(theme.primary.opacity(0.15))
                            .border_2()
                            .border_color(theme.primary)
                            .items_center()
                            .justify_center()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(theme.muted_foreground)
                                    .child("复制并在 飞书/TG 中发送："),
                            )
                            .child(
                                div()
                                    .text_size(px(22.))
                                    .font_weight(FontWeight::BLACK)
                                    .text_color(theme.foreground)
                                    .child(format!("/bind {code_str}")),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child("• 验证码有效期 10 分钟\n• 绑定后两端均支持每日 08:30 计划早报与 21:30 督促提醒\n• 支持在消息卡片中直接点击按钮完成打卡并双向同步"),
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .justify_end()
                            .child(
                                Button::new("cancel-bind")
                                    .label("稍后再说")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cloud_bind_modal_open = false;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("confirm-bind")
                                    .primary()
                                    .label("✅ 我已发送，完成绑定")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.check_bind_status_action(window, cx);
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// 渲染云端同步结果弹窗。
    fn render_sync_result_modal(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme().clone();
        let Some((title, is_success, lines)) = &self.cloud_sync_modal_data else {
            return div().into_any_element();
        };

        let title_icon = if *is_success { "icons/square-check.svg" } else { "icons/refresh-cw.svg" };
        let icon_color = if *is_success { theme.primary } else { theme.danger };

        let content_items: Vec<gpui::AnyElement> = lines
            .iter()
            .map(|line| {
                div()
                    .text_size(px(13.))
                    .text_color(theme.foreground)
                    .child(line.clone())
                    .into_any_element()
            })
            .collect();

        div()
            .absolute()
            .inset_0()
            .bg(hsla(0.0, 0.0, 0.0, 0.6))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(480.))
                    .bg(theme.background)
                    .border_2()
                    .border_color(theme.foreground)
                    .shadow_lg()
                    .p_6()
                    .gap_4()
                    .flex()
                    .flex_col()
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(Icon::empty().path(title_icon).size_5().text_color(icon_color))
                                    .child(
                                        div()
                                            .text_size(px(16.))
                                            .font_weight(FontWeight::BOLD)
                                            .child(title.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .id("close-sync-modal")
                                    .cursor_pointer()
                                    .p_1()
                                    .child(Icon::empty().path("icons/square.svg").size_4().text_color(theme.muted_foreground))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cloud_sync_modal_open = false;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        v_flex()
                            .w_full()
                            .gap_2()
                            .p_3()
                            .bg(theme.primary.opacity(0.06))
                            .border_1()
                            .border_color(theme.border)
                            .children(content_items),
                    )
                    .child(
                        h_flex()
                            .justify_end()
                            .child(
                                Button::new("sync-modal-ok-btn")
                                    .primary()
                                    .label("好的")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cloud_sync_modal_open = false;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }
}

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

        let bind_modal = self.cloud_bind_modal_open.then(|| self.render_bind_modal(cx));
        let sync_modal = self.cloud_sync_modal_open.then(|| self.render_sync_result_modal(cx));
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
            .child(div().flex_1().min_h_0().child(body))
            .children(bind_modal)
            .children(sync_modal)
            .children(sheet_layer)
            .children(dialog_layer)
            .children(notification_layer)
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
        // 列宽合计 584px，贴合 640px 右栏的内容宽度，避免横向滚动。
        let columns = vec![
            Column::new("day", "天").width(px(44.)),
            Column::new("vid", "视频#").width(px(60.)),
            Column::new("title", "标题").width(px(196.)),
            Column::new("duration", "本日时长")
                .width(px(92.))
                .text_right(),
            Column::new("note", "备注").width(px(192.)),
        ];

        let day_note = |cumulative: i64| {
            // 不带"累计"前缀以控制宽度：首个时间即当日累计，"剩"标注全片剩余。
            format!(
                "{} · 剩 {}",
                fmt_seconds(cumulative as f64, true),
                fmt_seconds((plan.total - cumulative) as f64, true)
            )
        };

        let mut rows: Vec<PlanRow> = Vec::new();
        let mut cumulative: i64 = 0;
        for (di, entries) in plan.plan.iter().enumerate() {
            let day_total: i64 = entries.iter().map(|e| e.portion).sum();
            cumulative += day_total;
            let note = day_note(cumulative);
            if entries.is_empty() {
                rows.push(PlanRow {
                    cells: [
                        (di + 1).to_string(),
                        String::new(),
                        "（本日无安排 / 休息）".to_string(),
                        String::new(),
                        note,
                    ],
                    is_day_head: false,
                });
                continue;
            }
            // 日汇总行：黄色高亮 + 粗体（is_day_head），目标/累计/剩余拆到标题与备注列。
            rows.push(PlanRow {
                cells: [
                    (di + 1).to_string(),
                    String::new(),
                    format!("目标 {}", fmt_seconds(plan.capacities[di] as f64, true)),
                    String::new(),
                    note,
                ],
                is_day_head: true,
            });
            for e in entries {
                rows.push(PlanRow {
                    cells: [
                        String::new(),
                        format!("#{}", e.vid_no),
                        compact_subject(&e.title),
                        fmt_seconds(e.portion as f64, true),
                        compact_note(e),
                    ],
                    is_day_head: false,
                });
            }
        }

        Self { columns, rows }
    }
}

/// 表格内用的简短备注（完整散文版 `note_for` 仅用于导出文本）。
fn compact_note(e: &PlanEntry) -> String {
    if e.remainder > 0 {
        if e.from_prev {
            return "接上日·仍未完".to_string();
        }
        return match e.cont_day {
            Some(d) => format!("跨天·续至第{d}天"),
            None => "跨天·后续顺延".to_string(),
        };
    }
    if e.from_prev {
        return "接上日·本日完结".to_string();
    }
    "完整".to_string()
}

/// 表格内的紧凑科目前缀：`[科目 12] xxx` → `科12·xxx`，
/// 为窄列省出约 5 个显示宽度（导出文本仍用完整前缀）。
fn compact_subject(title: &str) -> String {
    let Some(rest) = title.strip_prefix("[科目") else {
        return title.to_string();
    };
    match rest.split_once(']') {
        Some((num, tail)) => format!("科{}·{}", num.trim(), tail.trim_start()),
        None => title.to_string(),
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
        let theme = cx.theme();
        div()
            .size_full()
            .px_2()
            .text_size(px(13.))
            .when(row.is_day_head, |d| {
                // 日汇总行：黄色高亮 + 粗体，是野兽风表格的标志节奏。
                d.font_weight(FontWeight::BOLD)
                    .bg(if theme.is_dark() {
                        hsla(0.16, 0.9, 0.30, 1.0)
                    } else {
                        hsla(0.135, 1.0, 0.86, 1.0)
                    })
                    .text_color(theme.foreground)
            })
            .when(!row.is_day_head, |d| {
                d.text_color(theme.secondary_foreground)
            })
            .whitespace_nowrap()
            .overflow_hidden()
            .text_ellipsis()
            .child(row.cells[col_ix].clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{EpisodeItem, Group};
    use gpui::TestAppContext;

    #[test]
    fn compact_subject_shortens_prefix() {
        assert_eq!(compact_subject("[科目 1] P1 前言"), "科1·P1 前言");
        assert_eq!(compact_subject("[科目12] 集合"), "科12·集合");
        assert_eq!(compact_subject("普通标题"), "普通标题");
        assert_eq!(compact_subject("[科目"), "[科目");
    }

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
    /// 都不应 panic。
    #[gpui::test]
    async fn render_smoke_both_themes(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::init(cx);
        });

        let window = cx.add_window(|window, cx| {
            let mut app = PlannerApp::new(window, cx);
            let mut rd = ready_with_plan();
            let mut expanded = false;
            PlannerApp::run_generate(
                &mut rd,
                Mode::Split,
                3,
                &mut app.plan_table,
                &mut expanded,
                window,
                cx,
            );
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
