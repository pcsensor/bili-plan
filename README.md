# bili-planner — Bilibili & Jellyfin 合集观看计划生成器（桌面应用）

用 Rust + [gpui-component](https://github.com/longbridge/gpui-component)（v0.5，构建于 Zed 的 GPU 加速 UI 框架 [GPUI](https://gpui.rs) 之上的 shadcn 风格组件库）实现的跨平台桌面应用，视觉为 Neo-Brutalist（新野兽风）主题：纸面底色 + 点阵肌理与低透明度几何色块装饰、2px 硬边框、直角、纯偏移硬阴影，卡片以 quint 减速曲线错峰入场；内置亮/暗双模式（亮色电光蓝主色、暗色荧光黄主色）。

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
- 计划表展示与 UTF-8 文本导出（虚拟滚动表格，长计划不掉帧）
- 搜索历史：获取成功的输入自动记录（去重、上限 20 条、持久化），点击一键回填链接，支持单条删除与一键清空
- 亮/暗主题切换（Neo-Brutalist 主题）

## 构建与运行

前置：最新 stable Rust（gpui 使用 2024 edition）；macOS 需 Xcode 及 Metal 工具链（`xcodebuild -downloadComponent MetalToolchain`）。

```bash
cargo build --release
cargo run            # 启动桌面应用
```


## 打包安装包（Windows MSI）

前置：安装 [cargo-packager](https://docs.crabnebula.dev/packager/)（打包配置见 `Cargo.toml` 的 `[package.metadata.packager]`）：

```bash
cargo install cargo-packager --locked
cargo packager --release    # 生成 target/release/bili-planner_0.2.0_x64_en-US.msi
```

> 说明：
> - `identifier` 目前为占位符 `com.example.bili-planner`，正式发布前请改成你自己的反向域名（影响 MSI 的 Manufacturer 与 UpgradeCode）。
> - `icons/` 图标素材由 `python tools/gen_icons.py` 生成（与窗口内程序化绘制的图标同款设计）。
> - 首次打包会自动联网下载 WiX Toolset。
## 配置

- Jellyfin 服务器地址、Token 与搜索历史保存到本机（`~/.bili-planner.json`），下次启动自动填入/展示；文件缺 `history` 键也能兼容读入
- B 站 Cookie 仅在 UI 会话内使用，不持久化

## 测试与验证

```bash
cargo test           # 单元 + 集成 + core 编排冒烟 + gpui 无头渲染冒烟
cargo clippy --all-targets
cargo fmt --all -- --check
cargo run --example live_check -- "BV1ps4y1d73V" 30 all split             # B 站端到端（真实 API）
cargo run --example live_check_jellyfin -- "<Jellyfin 链接>" 30 all split # Jellyfin 端到端
```

## 跨平台

- gpui 渲染（macOS Metal / Windows DirectX / Linux），中文由系统字体回退渲染。
- 已在 macOS arm64 上验证构建与运行；Windows / Linux 需在对应环境构建验证。

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
src/core.rs      业务编排层（状态数据 + 获取/生成/导出 + 凭证持久化，无 GUI 依赖）
src/theme.rs     主题定制（默认主题 + Apple System Blue 主色）
src/assets.rs    资产源（官方组件图标 + 应用专属图标合并）
src/app.rs       gpui-component 桌面应用（状态机 + 视图 + 计划表委托）
src/main.rs      应用入口（Application + 窗口 + Root 包裹）
assets/icons/    应用专属 lucide 图标（组件内部图标由 gpui-component-assets 提供）
examples/        端到端验证示例
tests/           测试（含 Python 期望向量 + Jellyfin fixture + core 编排冒烟）
```

## 许可证

MIT（见 `Cargo.toml`）。