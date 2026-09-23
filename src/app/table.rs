use super::*;

/// 计划表的一行：日汇总行或视频行。
struct PlanRow {
    cells: [String; 5],
    is_day_head: bool,
}

/// `Table` 委托：静态 5 列 + 预构建行数据，虚拟滚动由 Table 内部处理。
pub(super) struct PlanTableDelegate {
    columns: Vec<Column>,
    rows: Vec<PlanRow>,
}

impl PlanTableDelegate {
    pub(super) fn new(plan: &crate::core::PlanData) -> Self {
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
pub(super) fn compact_subject(title: &str) -> String {
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
