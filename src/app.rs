//! fenestra 桌面应用：状态机、视图与消息处理。
//!
//! 与 Python 脚本功能一一对应；核心算法在 api / parse / plan / export
//! 模块中，本模块只做状态编排与 UI。

use std::path::PathBuf;
use std::time::Duration;

use crate::api;
use crate::export;
use crate::parse::{self, EpisodeItem, Group};
use crate::plan::{build_plan, fmt_human, fmt_seconds, note_for, trunc, Mode, PlanEntry};
use crate::{extract_sid, Error};

use fenestra::prelude::*;
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
        let header = row().w_full().items_center().justify_between().children([
            text("Bilibili 合集观看计划")
                .size(TextSize::Xl)
                .weight(Weight::Semibold),
            Element::from(
                button(if self.dark { "亮色" } else { "暗色" })
                    .variant(ButtonVariant::Secondary)
                    .on_click(Msg::DarkToggled),
            ),
        ]);

        let mut children: Vec<Element<Msg>> = vec![header, self.form_card()];

        if let Some(err) = &self.last_error {
            children.push(callout(Status::Danger, err.clone()));
        }

        match &self.phase {
            Phase::Loading => children.push(
                row()
                    .gap(SP2)
                    .items_center()
                    .children([spinner(), text("正在获取视频信息…")]),
            ),
            Phase::Ready(rd) => children.extend(self.ready_children(rd)),
            Phase::Input => {}
        }

        children.push(Element::from(
            toast_stack(self.toasts.iter().map(|(_, m, s)| (m.clone(), *s)))
                .on_dismiss(Msg::DismissToast),
        ));

        col()
            .w_full()
            .h_full()
            .p(SP6)
            .gap(SP4)
            .scroll_y()
            .children(children)
    }

    fn form_card(&self) -> Element<Msg> {
        let loading = self.loading();
        let days_field = field("目标观看天数").child(
            text_input(&self.days_text)
                .placeholder("如 30")
                .width(120.0)
                .on_input(Msg::EditDays)
                .id("days"),
        );
        let mode_field = field("计划模式").child(segmented(
            self.mode.index(),
            ["split 精确切分", "whole 完整不拆"],
            Msg::ModeChanged,
        ));
        card().children([
            Element::from(
                field("链接 / BV 号 / 合集 sid")
                    .help("支持 https://www.bilibili.com/video/BVxxxx、BV 号或合集 sid=xxxx 链接")
                    .child(
                        text_input(&self.input)
                            .placeholder(
                                "https://www.bilibili.com/video/BV1ps4y1d73V 或 BV 号 或 sid=6789",
                            )
                            .width(560.0)
                            .on_input(Msg::EditInput)
                            .id("input"),
                    ),
            ),
            row()
                .gap(SP4)
                .items_end()
                .children([Element::from(days_field), Element::from(mode_field)]),
            Element::from(
                field("Cookie（可选，风控时使用）")
                    .help("例如 SESSDATA=xxx；留空则匿名请求")
                    .child(
                        text_input(&self.cookie)
                            .placeholder("SESSDATA=xxx")
                            .width(560.0)
                            .on_input(Msg::EditCookie)
                            .id("cookie"),
                    ),
            ),
            row().gap(SP3).items_center().children([
                Element::from(
                    button("获取视频信息")
                        .on_click(Msg::Fetch)
                        .disabled(loading),
                ),
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
        out.push(card().children(info));

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
            out.push(card().children(sel));
        }

        // 操作按钮
        let has_plan = rd.plan.is_some();
        out.push(
            row().gap(SP3).children([
                Element::from(
                    button("生成观看计划")
                        .variant(ButtonVariant::Secondary)
                        .on_click(Msg::Generate),
                ),
                Element::from(
                    button("导出计划文本（UTF-8）")
                        .on_click(Msg::Export)
                        .disabled(!has_plan),
                ),
            ]),
        );

        // 计划表格
        if let Some(p) = &rd.plan {
            out.push(self.plan_table(p));
        } else {
            out.push(
                text("填写目标天数后点击「生成观看计划」。")
                    .size(TextSize::Sm)
                    .themed(|t: &Theme, s| s.color(t.text_muted)),
            );
        }

        out
    }

    fn plan_table(&self, p: &PlanData) -> Element<Msg> {
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

        Element::from(
            data_table(["天", "视频#", "标题", "本日时长", "备注"], rows)
                .id("plan-table")
                .column_widths([64.0, 84.0, 320.0, 116.0, 400.0]),
        )
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
        if self.dark {
            Theme::dark()
        } else {
            Theme::light()
        }
    }

    fn view(&self) -> Element<Msg> {
        PlannerApp::view(self)
    }
}
