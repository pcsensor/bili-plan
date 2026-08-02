//! Bilibili 合集观看计划生成器核心库（无 GUI 依赖，可纯逻辑测试）。

pub mod api;
pub mod app;
pub mod error;
pub mod export;
pub mod model;
pub mod parse;
pub mod plan;

pub use error::{Error, ErrorKind, Result};
pub use parse::{extract_bvid, extract_sid, parse_groups, Group, ParseResult};
pub use plan::{build_plan, Mode, PlanEntry, PlanOut};
