//! Bilibili & Jellyfin 合集观看计划生成器（跨平台桌面应用入口）。

use bili_planner::app::PlannerApp;
use fenestra::prelude::*;

fn main() {
    fenestra::run(
        PlannerApp::new(),
        WindowOptions::titled("bili-planner — Bilibili & Jellyfin 观看计划")
            .with_size(1120.0, 820.0)
            .with_min_size(960.0, 660.0)
            .with_icon(ICON_W, ICON_H, icon_bytes()),
    );
}

// ---------------------------------------------------------------------------
// 应用图标：32×32 RGBA，圆角蓝底 + 中央白色三角播放符号
// ---------------------------------------------------------------------------

const ICON_W: u32 = 32;
const ICON_H: u32 = 32;

/// 生成 32×32 RGBA 图标（与窗口标题栏、任务栏对应）。
///
/// 设计：Apple HIG 极简风——System Blue (`#0A84FF`) 圆形底，中央一个
/// 白色等腰三角形作为播放符号。圆 + 三角形都是判内可见的简单几何，
/// 反走样近似采用覆盖率（边缘像素 alpha 按距离衰减），避免锯齿。
fn icon_bytes() -> Vec<u8> {
    let mut buf = vec![0u8; (ICON_W * ICON_H * 4) as usize];

    let cx = 15.5_f32;
    let cy = 15.5_f32;
    let radius = 14.0_f32;

    // 播放三角形顶点（顺时针，y 向下）：
    //   A(12, 10) 左上
    //   C(22, 15.5) 右中
    //   B(12, 21) 左下
    // 对一个点 P，包括圆覆盖率和三角形内判定。
    for y in 0..ICON_H {
        for x in 0..ICON_W {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;

            // 圆覆盖率（反走样）：圆心距离与半径之差决定 alpha 平滑。
            let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt() - radius;
            let circle_alpha = smoothstep(0.5, -0.5, d);

            // 三角形内判定（叉积同号，三角形 ABC 顺时针）：
            let inside_tri = tri_barycentric((px, py), (12.0, 10.0), (22.0, 15.5), (12.0, 21.0));

            // 像素颜色：圆内 → 蓝底；三角形 → 白
            let (r, g, b) = if inside_tri {
                (255_u8, 255, 255)
            } else {
                (10_u8, 132, 255) // System Blue
            };
            let a = (circle_alpha * 255.0).round().clamp(0.0, 255.0) as u8;

            let i = ((y * ICON_W + x) * 4) as usize;
            buf[i] = r;
            buf[i + 1] = g;
            buf[i + 2] = b;
            buf[i + 3] = a;
        }
    }
    buf
}

/// 顺时针三角形 A→B→C 的内点判定（叉积同号）。顶点用 `(x, y)` 元组。
fn tri_barycentric(p: (f32, f32), a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> bool {
    let (px, py) = p;
    let (ax, ay) = a;
    let (bx, by) = b;
    let (cx, cy) = c;
    let d1 = (px - bx) * (ay - by) - (ax - bx) * (py - by);
    let d2 = (px - cx) * (by - cy) - (bx - cx) * (py - cy);
    let d3 = (px - ax) * (cy - ay) - (cx - ax) * (py - ay);
    let neg = (d1 < 0.0) || (d2 < 0.0) || (d3 < 0.0);
    let pos = (d1 > 0.0) || (d2 > 0.0) || (d3 > 0.0);
    !(neg && pos)
}

/// 简易平滑阶跃：`edge0` 处 0、`edge1` 处 1，中间线性过渡。
/// 用法 `smoothstep(edge0, edge1, x)`；这里 `edge0 > edge1`，
/// 表示 x ≤ edge0 时输出 0、x ≥ edge1 时输出 1。
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
