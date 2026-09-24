# 架构与开发流程

本文是项目的架构约束和功能变更指南。实现与本文不一致时，先核对代码和测试，再同步修正文档；不要仅为通过检查而放宽依赖规则。

## 1. 项目组成与依赖方向

仓库是一个 Cargo workspace，共有三个包：

| 包 | 位置 | 职责 |
| --- | --- | --- |
| `planner-domain` | `crates/domain/` | 视频规划输入、计划算法、打卡与排期模型、排期恢复及统计；无网络、文件、数据库或 GUI 依赖 |
| `bili-planner` | 根目录 `src/` | GPUI 桌面应用、本地 SQLite、云同步客户端、B 站/Jellyfin/飞牛影视适配器 |
| `bili-plan-server` | `server/` | HTTP API、云端 SQLite、飞书与 Telegram 机器人、定时推送 |

```text
桌面应用 ─────┐
             ├──> planner-domain
云端服务 ─────┘
```

箭头表示“依赖”。桌面端和服务端不得互相引用；领域包不得引用任何应用层或基础设施。跨端共享的 Rust 类型必须放在领域包，不能再用 `#[path = "../..."]` 或复制一份结构体来共享行为。`Cargo.toml` 的 workspace 和 `tools/check_architecture.py` 会检查包级依赖。

领域包内部也保持单向关系：

```text
catalog ──> plan ─────────────┐
model ────> schedule_recovery ├──> study
model ────────────────────────┘
source ────────────────────────> study
```

`catalog` 包含 `EpisodeItem`、`Group`；`model` 包含 `StudyPlan`、`TaskItem`、`DailyNote`、`ScheduleShift` 等持久化类型；`plan` 分配视频时长；`schedule_recovery` 负责补位与撤销；`source` 集中来源标签和跨端播放链接；`study` 组合这些规则形成打卡、改期、日历及统计操作。图中没有反向边，检查脚本也会拒绝新增反向导入。

## 2. 桌面端模块边界

| 位置 | 可承担的工作 | 不应承担的工作 |
| --- | --- | --- |
| `src/api.rs`、`parse.rs`、`jellyfin.rs`、`fnos.rs` | 外部来源请求、响应解析、转成统一 `Group` | GPUI 状态、本地计划持久化 |
| `src/core.rs` | 来源分派、计划生成、领域操作编排与应用配置 | 直接实现 SQLite 表读写或 HTTP 请求细节 |
| `src/core/storage.rs` | 本机配置数据库、旧 JSON 导入、持久化 | 渲染、来源解析、排期业务规则 |
| `src/core/cloud.rs` | 云端认证、协议请求、同步响应解析 | 把后台配置副本直接写回本机数据库 |
| `src/app.rs`、`src/app/action_*.rs` | GPUI 状态、用户事件、异步任务及结果合并 | 复制领域算法或数据库 SQL |
| `src/app/view_*.rs`、`table.rs` | 页面、弹窗、表格显示 | 修改跨端数据契约 |

来源适配器产出 `Group` 后，计划生成、导出和排期不再关心原始来源。新增来源仍需接入输入表单、凭证、`source_type` 标记、链接打开方式、历史记录、机器人卡片和测试；“统一 `Group`”只隔离后半段业务，并不自动完成这些入口和展示工作。

`PlannerApp` 的网络任务使用配置副本执行后台请求。请求完成后，回到 UI 线程，把响应合并到**当前** `self.config`，再持久化当前状态。后台副本不得调用 `save_config`：用户可能已在请求期间继续修改计划。`tools/check_architecture.py` 对云同步函数设有静态检查。

## 3. 服务端模块边界

- `server/src/main.rs` 负责配置、路由、鉴权接入和飞书回调入口。
- `server/src/store.rs` 是服务端 SQLite 的写入出口，设备绑定、计划同步、备注和推送记录都经过它。
- `server/src/card.rs` 与 `telegram.rs` 分别负责渠道内容；`feishu.rs` 负责飞书 API；`scheduler.rs` 负责每日发送时机与重试。
- `server/src/models.rs` 只保留服务端专用的请求/响应和设备类型。计划、任务、备注模型复用 `planner-domain` 导出的同一类型，不能定义镜像结构。

服务端模块之间的允许依赖边由 `tools/check_architecture.py` 固定。扩展机器人渠道时，应先说明它需要读取哪些领域数据和存储操作，再确定新模块的位置；不得让存储层反向依赖卡片或调度器。

## 4. 数据契约、存储与迁移

本机与云端各有 SQLite 数据库，计划和备注仍以 JSON 载荷保存。`crates/domain/src/model.rs` 是字段的唯一来源，`tools/schema_contract.json` 锁定持久化类型的字段名及 Rust 类型。字段清单变化会使本地架构检查失败，直到维护者明确更新快照。

修改持久化字段时按以下顺序处理：

1. 判断旧 JSON 是否仍可反序列化、新 JSON 是否能被当前服务端接收。新增可选字段通常需要 `#[serde(default)]`；重命名或删除字段需要显式迁移，不能只改结构体。
2. 增加至少一个旧数据样例测试，以及新数据往返测试。跨端字段要分别考虑桌面本地配置、服务端计划载荷、机器人打卡后的同步。
3. 如需 SQLite 列，初始化和旧库迁移都要覆盖，并在服务端存储测试中使用旧 schema 验证升级。
4. 更新 `tools/schema_contract.json`，在交付说明中写出服务端与桌面端升级顺序。

`schema_contract.json` 是变更提醒，不是自动迁移工具；检查通过也不等于所有历史数据都已验证。

`StudyPlan.source_type` 与日期字段继续以字符串存储，保持旧 JSON 与现有服务端协议兼容。来源判断应经 `planner_domain::source::SourceKind`，新写入的日期须先在领域层严格解析；不应把错误日期静默替换成今天。

## 5. 云同步的一致性规则

同步采用**一个桌面端主写计划结构、机器人更新任务打卡**的模型。桌面端提交完整计划快照和上次确认的 `base_revision`。服务端在同一个 SQLite 事务中读取版本、拒绝过期快照、合并机器人任务状态、保存快照并递增版本。版本不匹配时返回 HTTP 409；客户端必须提示用户核对数据，不能拿旧请求体自动重试。

机器人打卡使用任务的 `updated_at` 合并；同一任务在同一秒的连续操作仍要有严格递增的时间戳。备注删除保留 tombstone。整日提前的补位历史和机器人撤销后的 `advance_restored` 信号需要跨同步保留到客户端完成归位。

绑定与同步接口只从 `Authorization: Bearer` 请求头读取设备令牌。服务端不接受 body/query 令牌，桌面端也不再回退旧传输格式。升级时应同时准备好新版服务端和桌面端：旧传输格式的客户端在服务端升级后立即收到 401。仍用 Bearer 头但未提交版本号的旧客户端，仅在服务端版本为 0 时可同步；首次新版同步使版本增加后，其无版本快照也会被拒绝。多个桌面端使用同一设备令牌并发编辑时，版本检查防止旧快照静默覆盖新数据，但系统没有自动合并两份独立排期的能力。

本地数据库写入失败会输出错误信息；部分旧 UI 动作仍使用不返回错误的保存入口。新增关键写入流程应使用 `try_save_config` 并向用户显示失败，不要把内存更新当作已经落盘。

## 6. 排期业务不变量

改动排期时，应把以下规则当成必须保持的行为，并补与改动相关的组合测试：

- 向前移空某日后，同计划后续任务按实际任务日期依次补位；部分移走或向后改期不补位。周末手动任务和稀疏日期按实际日期槽位处理。
- 整日提前后取消任一任务的打卡，只撤销该次补位一次；被取消的任务归位，其余已完成任务留在今天。后来的手动改期优先，不被旧补位历史覆盖。
- 一键顺延收集全部过去未完成任务，按原日期分批排到目标学习日；已完成记录保留原日期。一次性日历事项不参与全局顺延。
- 同一日期的任务汇总为一个日程；任务 ID 在改期、删除、追加后不能复用。写操作必须严格校验日期，不能把非法日期当成今天。
- 今日打卡的科目筛选会限制一键提前的作用范围；日历页面的全局入口按全部计划处理。
- 旧版本已发生但未记录补位历史的操作，不能可靠推断原日期。不要用猜测的迁移自动改写旧用户排期。

相关回归在 `tests/study_transitions.rs`、`crates/domain/src/study/tests.rs` 和 `server/src/store/tests.rs`。真实 GUI 点击、第三方 API 在线联调和旧用户数据库迁移需要针对实际环境另行验证。

## 7. 新功能与维护流程

1. **定义行为和归属。** 写清用户操作、数据所有权、受影响的入口和失败情形。先选定领域、适配器、存储、协议或 UI 层，再改代码；不跨层复制实现。
2. **列出兼容性影响。** 核对持久化 JSON、SQLite schema、同步请求/响应、旧客户端、来源响应和渠道展示。涉及跨端协议时写出升级顺序与冲突处理方式。
3. **先固定关键行为。** 为新规则或历史故障增加能失败的测试。排期改动覆盖周末、稀疏日期、重复操作与撤销顺序；同步改动覆盖旧快照、机器人打卡、重复请求和重启；来源改动覆盖解析与所有展示路径。
4. **按依赖方向实现。** 新功能从内向外完成领域、适配、编排和 UI；只暴露上层需要的接口。若模块超过 `tools/check_architecture.py` 中的体量预算，先拆职责再扩大功能。
5. **在本地运行检查。** 从仓库根目录执行 `bash tools/check.sh`。它依次运行架构与契约检查、架构导入解析器测试、`cargo fmt --check`、workspace Clippy（警告视为错误）、workspace 全目标测试。单独运行脚本时也可用 `python3 tools/check_architecture.py` 快速检查依赖。
6. **交付前复核。** 记录用户可见变化、测试结果、未验证的真实环境、旧数据兼容性与发布顺序。改了容器构建上下文且本机有 Docker 时，再运行 `docker build -f server/Dockerfile -t bili-plan-server:local .`。

检查脚本会拦截包级和主要模块级反向依赖、领域层引入 GUI/网络/数据库、跨目录源码包含、跨端模型重复定义、字段或类型漂移、关键模块继续膨胀，以及后台同步函数直接保存配置。它是自动保护网，不能替代行为测试和代码审查。

## 8. 本地构建入口

```bash
# 完整质量检查（不依赖 GitHub CI）
bash tools/check.sh

# 仅运行共享领域测试
cargo test -p planner-domain --locked

# 服务端编译；Docker Compose 从完整仓库的 server/ 启动，构建上下文是仓库根目录
cargo build --release --locked -p bili-plan-server

# 桌面端
cargo run --release --bin bili-planner
```

桌面端在 macOS 上构建需要可用的 Xcode Metal Toolchain；`xcrun --find metal` 和 `xcrun metal -v` 可检查本机工具链。真实 B 站、Jellyfin、飞牛影视、飞书或 Telegram 联调会访问外部服务，不属于离线测试脚本。

服务端部署必须保持完整 workspace 目录结构。旧版只上传 `server/` 内容到 `/opt/bili-plan-server` 的方式与当前 `docker-compose.yml` 不兼容；迁移目录及数据步骤见根目录 README 的部署章节。
