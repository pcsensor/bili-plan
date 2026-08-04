# bili-planner — Bilibili & Jellyfin 合集观看计划生成器（桌面应用）

用 Rust + [fenestra](https://github.com/richer-richard/fenestra)（v0.40，纯 Rust 原生 GUI：winit + wgpu + vello + taffy + parley）实现的跨平台桌面应用。

输入 B 站或 Jellyfin 的合集链接，识别分栏 / 分 P / 季 / 子合集结构，按目标天数生成每日观看计划，可导出为 UTF-8 文本。

## 功能

### 数据来源
- **B 站**：输入视频链接 / BV 号 / 合集 sid 链接，通过 view API 与合集归档接口获取，支持 Cookie、重试
- **Jellyfin**：输入服务器地址 + API Token + 网页链接（`?id=` / `?parentId=` / `?seasonId=` / `?seriesId=`），按 Folder/Series/Season/Movie 自动分派

### 合集结构识别
- B 站：多分栏按科目统计、单分栏多 P 视频按科目、普通合集整集统计、非合集多 P/单视频、分栏缺失回退归档接口
- Jellyfin：Folder/SubFolder 两层抓取按子合集分科目；Series 按 Season 分科目；Season/Movie/Episode 单视频

### 计划生成
- 科目选择（单个科目或整个合集）
- 总时长统计（时:分:秒 + 人类可读）
- 按目标天数生成每日观看计划：
  - `split`：日均精确切分，标注跨天分割点
  - `whole`：视频保持完整不拆分
  - 休息日提示
- 计划表展示与 UTF-8 文本导出
- 亮/暗主题切换（Apple HIG 风格 UI）

## 构建与运行

前置：Rust 1.88+（含 MSVC 构建工具，Windows）。

```bash
cargo build --release
cargo run            # 启动桌面应用
```

## 配置

- Jellyfin 服务器地址与 Token 可保存到本机（`~/.bili-planner.json`），下次启动自动填入
- B 站 Cookie 仅在 UI 会话内使用，不持久化

## 测试与验证

```bash
cargo test           # 单元 + 集成 + UI 冒烟（与 Python 输出向量逐项对比）
cargo clippy --all-targets
cargo fmt --all -- --check
cargo run --example live_check -- "BV1ps4y1d73V" 30 all split             # B 站端到端（真实 API）
cargo run --example live_check_jellyfin -- "<Jellyfin 链接>" 30 all split # Jellyfin 端到端
cargo run --example screenshot                                           # 无头 UI 截图
```

## 跨平台

- 代码无平台专属 API；Windows / macOS / Linux 均可用原生窗口与文件对话框（rfd 在 Linux 走 xdg-portal）。
- 已在 Windows x86_64 上验证构建与运行；macOS / Linux 需在对应环境构建验证。
- 中文由操作系统字体渲染（fenestra 窗口内使用系统字体）。

## 目录结构

```
src/lib.rs       核心库导出
src/model.rs     B 站 API 响应模型（serde）
src/api.rs       B 站 HTTP 层（ureq + 重试 + 错误提示）
src/jellyfin.rs  Jellyfin 适配器（Client + 链接解析 + Item 分派）
src/parse.rs     B 站输入解析 + 合集结构识别
src/plan.rs      观看计划算法与格式化（与 Python 逐行对齐）
src/export.rs    完整文本输出
src/error.rs     应用错误类型（Input/Api/Network/Data，与 Python 错误消息对齐）
src/app.rs       fenestra 桌面应用（状态机 + 视图 + 配置持久化）
src/main.rs      应用入口（窗口标题 + 程序化图标）
examples/        端到端验证与截图示例
tests/           测试（含 Python 生成的期望向量 + Jellyfin fixture + UI 冒烟）
```

## 许可证

MIT（见 `Cargo.toml`）。