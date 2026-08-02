# bili-planner — Bilibili 合集观看计划生成器（桌面应用）

用 Rust + [fenestra](https://github.com/richer-richard/fenestra)（v0.40，纯 Rust 原生 GUI：winit + wgpu + vello + taffy + parley）实现的跨平台桌面应用，功能与 `bilibili_collection_planner.py` 完全一致。

## 功能

- 输入 B 站视频链接 / BV 号 / 合集 sid 链接；
- 通过 B 站 API 获取视频/合集信息（view API、合集归档 API，支持 Cookie、重试）；
- 智能识别合集层级（合集 → 分栏 → 视频 → 分P）：多分栏按科目统计、单分栏多 P 视频按科目、普通合集整集统计、非合集多 P/单视频、分栏缺失回退归档接口；
- 科目选择（单个科目或整个合集）；
- 总时长统计（时:分:秒 + 人类可读）；
- 按目标天数生成每日观看计划：
  - `split`：日均精确切分，标注跨天分割点；
  - `whole`：视频保持完整不拆分；
  - 休息日提示；
- 计划表展示与 UTF-8 文本导出（内容与 CLI 输出逐字节一致）；
- 亮/暗主题切换。

## 构建与运行

前置：Rust 1.88+（含 MSVC 构建工具，Windows）。

```bash
cargo build --release
cargo run            # 启动桌面应用
```

## 测试与验证

```bash
cargo test           # 单元 + 集成 + UI 冒烟（与 Python 输出向量逐项对比）
cargo clippy --all-targets
cargo fmt --all -- --check
cargo run --example live_check -- "BV1ps4y1d73V" 30 all split   # 端到端（真实 API）
cargo run --example screenshot                                   # 无头 UI 截图
```

## 跨平台

- 代码无平台专属 API；Windows / macOS / Linux 均可用原生窗口与文件对话框（rfd 在 Linux 走 xdg-portal）。
- 已在 Windows x86_64 上验证构建与运行；macOS / Linux 需在对应环境构建验证。
- 中文由操作系统字体渲染（fenestra 窗口内使用系统字体）。

## 目录

```
src/lib.rs    核心库导出
src/model.rs  B 站 API 响应模型（serde）
src/api.rs    HTTP 层（ureq + 重试 + 错误提示）
src/parse.rs  输入解析 + 合集结构识别
src/plan.rs   观看计划算法与格式化（与 Python 逐行对齐）
src/export.rs 完整文本输出
src/app.rs    fenestra 桌面应用（状态机 + 视图）
src/main.rs   应用入口
examples/     验证与截图示例
tests/        测试（含 Python 生成的期望向量）
```

