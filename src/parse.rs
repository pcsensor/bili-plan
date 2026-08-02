//! 输入解析与结构识别（合集 -> 分栏 -> 视频 -> 分P），与 Python 脚本逻辑对齐。

use crate::api;
use crate::error::{Error, Result};
use crate::model::{ArchiveItem, Episode, ViewData};
use serde::Serialize;

/// 一个观看单元（视频或分 P）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EpisodeItem {
    pub title: String,
    pub duration: i64,
}

/// 一门科目（分栏 / 多 P 视频 / 整个合集）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Group {
    pub name: String,
    pub episodes: Vec<EpisodeItem>,
}

/// parse_groups 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseResult {
    pub season_title: String,
    pub groups: Vec<Group>,
    pub structure: String,
}

/// 分栏缺失时的回退拉取函数（可注入以便测试）。
pub type FallbackFetch<'a> = &'a dyn Fn(i64, Option<&str>) -> Result<Vec<ArchiveItem>>;

/// 默认回退：合集归档接口。
pub fn default_fallback(season_id: i64, cookie: Option<&str>) -> Result<Vec<ArchiveItem>> {
    api::fetch_season_archives(season_id, cookie)
}

/// 从文本中识别 BV 号（BV + 10 位字母数字）。
pub fn extract_bvid(text: &str) -> Result<String> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    for i in 0..n.saturating_sub(11) {
        if chars[i] == 'B'
            && chars[i + 1] == 'V'
            && chars[i + 2..i + 12]
                .iter()
                .all(|c| c.is_ascii_alphanumeric())
        {
            return Ok(chars[i..i + 12].iter().collect());
        }
    }
    Err(Error::input(
        "无法从输入中识别 BV 号。请提供形如 \
         https://www.bilibili.com/video/BV1ps4y1d73V 的链接或直接输入 BV 号。",
    ))
}

/// 从文本中识别合集 sid / season_id。
pub fn extract_sid(text: &str) -> Option<i64> {
    let lower = text.to_lowercase();
    for key in ["sid", "season_id"] {
        let mut rest = lower.as_str();
        while let Some(pos) = rest.find(key) {
            let after = &rest[pos + key.len()..];
            if let Some(eq) = after.strip_prefix('=') {
                let digits: String = eq.chars().take_while(|c| c.is_ascii_digit()).collect();
                if !digits.is_empty() {
                    return digits.parse().ok();
                }
            }
            rest = after;
        }
    }
    None
}

/// 把一个合集条目展开为观看单元列表（与 `_episode_items` 对齐）。
pub fn episode_items(ep: &Episode) -> Vec<EpisodeItem> {
    let ep_title = ep.title.clone().unwrap_or_default().trim().to_string();
    let pages = ep.pages.clone().unwrap_or_default();
    let a = ep.arc.as_ref().map_or(0, |a| a.duration);
    let arc_dur = if a != 0 {
        a
    } else if ep.duration != 0 {
        ep.duration
    } else {
        0
    };

    if !pages.is_empty() {
        let mut items: Vec<EpisodeItem> = Vec::new();
        for p in &pages {
            let part = p.part.clone().unwrap_or_default().trim().to_string();
            let num = p.page;
            let title = if !part.is_empty() {
                if num != 0 {
                    format!("P{num} {part}")
                } else {
                    part
                }
            } else if num != 0 {
                format!("P{num}")
            } else {
                ep_title.clone()
            };
            items.push(EpisodeItem {
                title,
                duration: p.duration,
            });
        }
        let psum: i64 = items.iter().map(|i| i.duration).sum();
        // pages 时长之和与整集时长不一致（数据疑似被截断）时，退回整集时长。
        let threshold = 30.0f64.max(arc_dur as f64 * 0.01);
        if arc_dur > 0 && ((psum - arc_dur) as f64).abs() > threshold {
            return vec![EpisodeItem {
                title: ep_title,
                duration: arc_dur,
            }];
        }
        return items;
    }

    vec![EpisodeItem {
        title: ep_title,
        duration: arc_dur,
    }]
}

/// 从 view API 数据中提取合集结构（与 `parse_groups` 对齐）。
pub fn parse_groups(
    view: &ViewData,
    cookie: Option<&str>,
    fallback: FallbackFetch<'_>,
) -> Result<ParseResult> {
    let mut groups: Vec<Group> = Vec::new();
    let season_title: String;
    let mut structure: String;

    if let Some(season) = &view.ugc_season {
        season_title = season.title.clone().unwrap_or_default().trim().to_string();
        let sections = season.sections.clone().unwrap_or_default();

        if sections.len() > 1 {
            // 多分栏：每个分栏视为一门科目
            for (i, sec) in sections.iter().enumerate() {
                let mut items: Vec<EpisodeItem> = Vec::new();
                for ep in sec.episodes.clone().unwrap_or_default() {
                    items.extend(episode_items(&ep));
                }
                if !items.is_empty() {
                    let t = sec.title.clone().unwrap_or_default().trim().to_string();
                    let name = if t.is_empty() {
                        format!("分栏{}", i + 1)
                    } else {
                        t
                    };
                    groups.push(Group {
                        name,
                        episodes: items,
                    });
                }
            }
            structure = "多分栏合集（每个分栏视为一门科目）".to_string();
        } else if sections.len() == 1 {
            let sec = &sections[0];
            let episodes = sec.episodes.clone().unwrap_or_default();
            let multi: Vec<&Episode> = episodes
                .iter()
                .filter(|ep| ep.pages.as_ref().map_or(0, Vec::len) > 1)
                .collect();
            if episodes.len() > 1 && multi.len() >= 2 {
                // 分栏内含多个多P视频 → 每个视频视为一门科目
                for ep in &episodes {
                    let items = episode_items(ep);
                    if !items.is_empty() {
                        let t = ep.title.clone().unwrap_or_default().trim().to_string();
                        let name = if t.is_empty() {
                            "科目".to_string()
                        } else {
                            t
                        };
                        groups.push(Group {
                            name,
                            episodes: items,
                        });
                    }
                }
                structure = "单分栏合集（分栏内每个多P视频视为一门科目，含多个分P）".to_string();
            } else {
                // 普通合集：整个分栏视为一门课程
                let mut items: Vec<EpisodeItem> = Vec::new();
                for ep in &episodes {
                    items.extend(episode_items(ep));
                }
                if !items.is_empty() {
                    let t = sec.title.clone().unwrap_or_default().trim().to_string();
                    let name = if t.is_empty() {
                        if season_title.is_empty() {
                            "整个合集".to_string()
                        } else {
                            season_title.clone()
                        }
                    } else {
                        t
                    };
                    groups.push(Group {
                        name,
                        episodes: items,
                    });
                }
                structure = "单分栏合集（整个合集视为一门课程）".to_string();
            }
        } else {
            structure = "合集无分栏数据".to_string();
        }

        // 分栏数据为空时，回退到合集归档接口拉取扁平列表
        if groups.is_empty() && season.id != 0 {
            let flat = fallback(season.id, cookie)?;
            if !flat.is_empty() {
                let name = if season_title.is_empty() {
                    "整个合集".to_string()
                } else {
                    season_title.clone()
                };
                let episodes = flat
                    .into_iter()
                    .map(|it| EpisodeItem {
                        title: it.title,
                        duration: it.duration,
                    })
                    .collect();
                groups.push(Group { name, episodes });
                structure = "分栏缺失，已通过归档接口拉取扁平列表".to_string();
            }
        }
    } else {
        // 非合集：单视频或多P视频
        season_title = view.title.clone().unwrap_or_default().trim().to_string();
        let pages = view.pages.clone().unwrap_or_default();
        if pages.len() > 1 {
            let mut items: Vec<EpisodeItem> = Vec::new();
            for p in &pages {
                let part = p.part.clone().unwrap_or_default().trim().to_string();
                let num = p.page;
                let title = if !part.is_empty() {
                    if num != 0 {
                        format!("{season_title}｜P{num} {part}")
                    } else {
                        format!("{season_title}｜{part}")
                    }
                } else {
                    season_title.clone()
                };
                items.push(EpisodeItem {
                    title,
                    duration: p.duration,
                });
            }
            groups.push(Group {
                name: "多P视频".to_string(),
                episodes: items,
            });
            structure = "非合集视频（含多个分P）".to_string();
        } else {
            groups.push(Group {
                name: "单视频".to_string(),
                episodes: vec![EpisodeItem {
                    title: season_title.clone(),
                    duration: view.duration,
                }],
            });
            structure = "非合集视频（单视频）".to_string();
        }
    }

    Ok(ParseResult {
        season_title,
        groups,
        structure,
    })
}
