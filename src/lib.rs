//! Bilibili 合集观看计划生成器核心库（无 GUI 依赖，可纯逻辑测试）。

pub mod api;
pub mod app;
pub mod error;
pub mod export;
pub mod jellyfin;
pub mod model;
pub mod parse;
pub mod plan;

pub use error::{Error, ErrorKind, Result};
pub use jellyfin::{
    classify_item, extract_base_url, extract_item_id, fetch_groups, group_episodes_by_season,
    group_episodes_by_series, ticks_to_secs, Item, JellyfinClient,
};
pub use parse::{extract_bvid, extract_sid, parse_groups, Group, ParseResult};
pub use plan::{build_plan, Mode, PlanEntry, PlanOut};
