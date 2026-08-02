//! Bilibili 合集观看计划生成器（跨平台桌面应用入口）。

use bili_planner::app::PlannerApp;
use fenestra::prelude::*;

fn main() {
    fenestra::run(
        PlannerApp::new(),
        WindowOptions::titled("Bilibili 合集观看计划")
            .with_size(1120.0, 820.0)
            .with_min_size(960.0, 660.0),
    );
}
