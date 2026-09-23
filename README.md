# bili-planner — Bilibili & Jellyfin 合集观看计划生成器与飞书云端督促打卡

<p align="center">
  <img src="icons/128x128.png" alt="bili-planner Logo" width="96" height="96" />
</p>

`bili-planner` 是一款专为自律学习者设计的跨平台合集观看计划生成与督导工具。包含 **GPU 加速的跨平台桌面客户端** 与 **支持飞书机器人交互卡片的轻量级云端同步服务**。

---

## 🌟 核心特性

### 1. 桌面端 (Client)
- **多源数据解析**：
  - **Bilibili**：支持视频链接、BV 号、合集 sid 链接，智能识别多分栏科目、单分栏多 P、课程归档等复杂层级结构。
  - **Jellyfin**：通过 API Token 直接抓取媒体库，支持 Folder/Series/Season 自动分层与单片/剧集结构分派。
  - **飞牛影视 (fnOS)**：通过服务器地址 + 账号密码登录，按条目 guid 递归展开影视库 / 合集 / 季，每季或每个子合集视为一门科目。
- **科学计划排期**：
  - 支持 `split`（按时长精准切分，自动计算跨天分割时间戳）与 `whole`（单集完整排期）两种模式。
  - 支持全集或指定单一科目排期、自定义目标完成天数与休息日标注。
- **任务改期与排期联动**：
  - 在“我的计划库”可为任一已有计划整体调整未完成排期：指定新的最早未完成日期后，所有未完成日期批次同步前移或后移，并保留原批次间隔；已完成任务与历史打卡日期不动。
  - 在“今日打卡”或日历任务列表点击“调整日期”，输入目标日期后保存，保留打卡记录和视频切片信息。
  - 向前改期清空某一天后，同计划后续任务按实际日期依次补位；部分移走或向后改期不触发补位。日历系列与视频计划规则一致，支持周末手动安排。
  - 一键提前整天已完成任务后，取消其中任意一项打卡，会恢复该次补位，并将取消的任务放回来源日期；其余已完成任务留在今天。后来明确手动改期的任务保留新日期。
  - 一键顺延收集所有过去未完成任务，按原日期分批排到今天起的学习日，后续排期让位；过去已完成的记录不动。一次性日历事项仍不参与全局自动顺延。
  - “今日打卡”筛选科目后，一键提前仅作用于该科目；日历页面作用于全部计划。改动自动本地保存，并沿用云同步设置。
  - 使用机器人撤销提前时，需同步升级客户端与服务端。旧版未记录补位历史，无法自动推断已经发生的旧补位；新版操作会持久保存恢复信息。
- **本地进度打卡**：
  - 内置打卡系统与进度环，支持每日进度完成度统计与日历打卡状态追踪。
- **Neo-Brutalist 视觉风格**：
  - 基于 Rust + [GPUI](https://gpui.rs) / [gpui-component](https://github.com/longbridge/gpui-component) 实现，纯 GPU 硬件加速渲染，冷启动毫秒级响应，内置亮/暗双色主题。

### 2. 云端服务与飞书机器人 (Cloud Server)
- **桌面与机器人打卡同步**：桌面端生成的计划可一键同步至云端，机器人打卡与桌面进度合并；过期桌面快照会被拒绝，避免覆盖更新的计划。
- **飞书机器人交互**：
  - **每日定时推送**：早 08:30 自动推送当日学习任务卡片；晚 21:30 自动检查并督促未打卡科目。
  - **富文本交互卡片**：直接在飞书聊天界面点击按钮完成单集打卡，即时更新卡片进度条。
  - **快速指令**：向机器人发送“打卡”、“我的计划”随时获取最新学习进度与交互卡片。
- **极简绑定流程**：桌面端生成 6 位临时验证码，私聊飞书机器人发送即可瞬间完成设备与账号绑定。

---

## 📂 项目结构

```
.
├── crates/domain/           # 纯领域层与跨端数据契约
├── src/                      # 桌面客户端源码 (GPUI + 来源适配器)
│   ├── app.rs / app/         # 桌面状态、动作、视图与表格
│   ├── core.rs / core/       # 编排、本地 SQLite 与云端同步
│   ├── api.rs / parse.rs     # B 站适配器与合集结构提取
│   ├── jellyfin.rs           # Jellyfin 媒体库适配器
│   ├── fnos.rs               # 飞牛影视适配器（含 authx 签名）
│   └── export.rs             # 文本导出
├── server/                   # 云端服务与飞书机器人后端 (Rust Axum)
│   ├── Dockerfile            # 多阶段构建 Dockerfile
│   ├── docker-compose.yml    # 一键部署编排文件
│   ├── .env.example          # 环境变量模板文件
│   └── src/                  # 服务端源码 (Axum + 飞书 OpenAPI + 调度器)
├── assets/ / icons/          # 静态资产与跨平台图标
└── tools/                    # 自动化打包构建脚本
```

架构边界和新增功能流程见 [项目架构说明](docs/project-overview.md)。

---

## 🚀 云端服务部署指南 (Server Deployment)

云端服务负责计划的云端存储、飞书开放平台回调接入以及每日定时推送通知。

### 1. 准备工作：创建飞书自建应用

1. 登录 [飞书开放平台开发者后台](https://open.feishu.cn/app) 并创建“企业自建应用”。
2. **获取凭据**：在 **“凭证与基础信息”** 页面获取 `App ID` 和 `App Secret`；在 **“事件订阅”** 中设置并保存 `Verification Token`，作为服务器的 `FEISHU_VERIFICATION_TOKEN`。服务会校验它以拒绝伪造回调。
3. **添加机器人能力**：在 **“添加应用能力”** 中开启 **“机器人”**。
4. **配置权限**：在 **“权限管理”** 中开通以下权限：
   - `im:message`（获取与发送单聊/群聊消息）
   - `im:message:send_as_bot`（以应用身份发消息）
5. **配置事件订阅与卡片请求网址**（部署服务并配置域名后回填）：
   - 请求网址 URL：`https://your-domain.com/api/feishu/callback`
   - 添加事件监听：`im.message.receive_v1`（接收消息事件）
6. **发布版本**：创建并发布一个应用版本以激活机器人。

---

### 2. 方式一：Docker Compose 部署（推荐）

#### 步骤 1：准备部署目录与配置文件
在服务器上拉取完整仓库。服务端依赖 `crates/domain`，不能只复制 `server/`。以下命令假定 `/opt/bili-plan-server` 是仓库根目录：

```bash
mkdir -p /opt/bili-plan-server
cd /opt/bili-plan-server
```

创建 `server/.env` 文件（可参考 `server/.env.example`）：
```env
PORT=3005
FEISHU_APP_ID=cli_xxxxxxxxxxxxxx
FEISHU_APP_SECRET=xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
FEISHU_VERIFICATION_TOKEN=xxxxxxxxxxxxxxxx
DATA_DIR=/app/data
```

#### 步骤 2：启动容器
确保仓库根目录的 `Cargo.toml`、`Cargo.lock`、`crates/domain/` 与 `server/` 都在。`server/docker-compose.yml` 会以仓库根目录作为构建上下文：

```bash
# 构建并后台启动
cd server
docker compose up -d --build

# 查看运行日志与健康状态
docker compose logs -f
```

服务端会在 `DATA_DIR` 创建 SQLite 数据库 `store.sqlite3`（启用 WAL）。升级前若该目录中存在旧版 `store.json`，首次启动会自动导入，原文件保留为备份。

---

### 3. 方式二：原生二进制 / Systemd 部署

#### 步骤 1：本地/服务器构建
```bash
cargo build --release --locked -p bili-plan-server
```

#### 步骤 2：配置 Systemd 守护进程
创建 `/etc/systemd/system/bili-plan-server.service`：

```ini
[Unit]
Description=Bili Plan Server with Feishu Bot
After=network.target

[Service]
Type=simple
User=root
WorkingDirectory=/opt/bili-plan-server
EnvironmentFile=/opt/bili-plan-server/.env
ExecStart=/opt/bili-plan-server/target/release/bili-plan-server
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

启动并设置开机自启：
```bash
systemctl daemon-reload
systemctl enable --now bili-plan-server
```

---

### 4. 反向代理与 SSL 配置 (Caddy)

飞书 Webhook 回调强制要求 HTTPS 协议。使用 **Caddy** 可以实现全自动申请和续签 Let's Encrypt SSL 证书，并自动配置 HTTP -> HTTPS 重定向。

编辑 `/etc/caddy/Caddyfile`（或 Caddy 配置目录）：

```caddy
# --- bili-plan-server (plan.yourdomain.com) ---
plan.yourdomain.com {
    encode gzip

    reverse_proxy 127.0.0.1:3005 {
        header_up Host {host}
        header_up X-Real-IP {remote_host}
        header_up X-Forwarded-For {remote_host}
        header_up X-Forwarded-Proto {scheme}
    }
}
```

重新加载 Caddy 服务使配置生效：
```bash
caddy reload --config /etc/caddy/Caddyfile
# 或通过 systemd：systemctl reload caddy
```

---

## 💻 桌面客户端使用与打包 (Client)

### 1. 本地开发与运行

**环境要求**：
- 最新 Stable Rust (Rust 2024 Edition)
- **macOS**：需安装 Xcode 与 Metal 工具链（`xcodebuild -downloadComponent MetalToolchain`）
- **Linux**：需安装 Vulkan 驱动与 `libxkbcommon-dev` / `libfontconfig1-dev` 等开发包

```bash
# 启动桌面端
cargo run --release
```

### 2. 绑定云端服务与飞书机器人
1. 打开桌面端应用，在左侧导航栏点击 **“云端同步” / “飞书机器人”**。
2. 填入您的云服务地址（如 `https://plan.yourdomain.com`）。
3. 点击 **“生成绑定码”**，界面将显示一个 6 位数字绑定码（10 分钟有效）。
4. 在飞书中打开自建机器人单聊，发送 `/bind <这 6 位数字>`。
5. 桌面端自动提示绑定成功，之后每次本地排期或打卡均会自动同步到飞书！

#### 云端接口的鉴权与限流

- **设备令牌**：由 `POST /api/device/register` 用 CSPRNG 签发，客户端持久化后作为
  `Authorization: Bearer <device_token>` 凭据。令牌只走请求头，不出现在 query string 中，
  避免被反向代理访问日志记录下来。
- **拒绝匿名写入**：`/api/sync` 与 `/api/bind/*` 使用 Bearer 令牌——缺失或畸形回 401，
  服务端查无此设备回带有 `unknown_device` 错误码的 404。客户端只在确认该错误码后注册
  新设备，并在重试成功后替换本地令牌；普通代理 404 不会导致绑定丢失。
- **滚动升级兼容**：迁移期内服务端仍识别旧客户端放在 body/query 中的令牌，但只允许访问
  数据库中已经存在的设备，绝不会通过旧协议创建新设备。新版客户端只在服务器明确不支持
  新协议时回退旧格式。所有桌面端升级后，将 `ALLOW_LEGACY_TOKEN_TRANSPORT=false` 并重启
  服务，即可彻底拒绝旧传输；迁移期间也应关闭反向代理的 query 日志。
- **限流**（固定窗口，超限回 429）：注册 20 次/小时/IP；同步 120 次/分/设备、600 次/分/IP；
  生成绑定码 10 次/小时/设备；查询绑定状态 60 次/分/设备。服务先执行 IP 限流、再验证
  设备、最后记录设备桶；限流表有硬容量，随机伪造令牌不能造成无界内存增长。
- **绑定码防暴破**：6 位 CSPRNG、10 分钟有效、绑定成功即作废；同一飞书 / Telegram 身份
  在 10 分钟内连续 5 次失败后锁定 15 分钟，锁定期内即使给出正确验证码也会被拒。失败记录
  会过期清理且有硬容量；过期码与不存在的码返回同一条消息，不泄露"该码是否签发过"。
- **飞书回调**：强制校验 `FEISHU_VERIFICATION_TOKEN`，未配置则服务拒绝启动。
- **日志不落密钥**：设备令牌不写入日志；机器人消息中的 `/bind <验证码>` 会被遮蔽为
  `/bind ******`，其余命令原文保留以便排查。

> ⚠️ **部署前提**：限流用的客户端 IP 取自 `X-Forwarded-For` 的**最右侧**一项，这要求服务
> 只经受信任的反向代理暴露（`docker-compose.yml` 已把端口绑在 `127.0.0.1`）。**不要**把
> 3005 端口直接开到公网，否则攻击者可伪造该头绕过按 IP 的限流。

### 3. 本地数据存储

桌面端在用户家目录保存 SQLite 数据库：`~/.bili-planner.sqlite3`。其中包含 Jellyfin 凭证、搜索历史、学习计划、自定义任务、日历备注与云同步设备标识。旧版本的 `~/.bili-planner.json` 会在首次启动新版时自动导入，且原文件保留为备份。

### 4. 飞牛影视 (fnOS) 接入说明

在左侧导航栏的「计划生成」页把数据来源切到 **飞牛影视**，填写：

| 字段 | 说明 |
| --- | --- |
| 服务器地址 | 形如 `http://192.168.1.10:5666`，**只填到端口**，不要带 `/v` 等路径 |
| 账号 / 密码 | **飞牛影视自己的账号**，不是飞牛系统登录账号 |
| 链接 / 条目 guid | 粘贴飞牛影视网页链接（自动识别 `?guid=` 参数或路径末段），或直接输入 guid |

填好后点「获取视频信息」。链接需指向**影视库 / 合集 / 季**（下面还有下级剧集），而不是单集。服务器地址留空时会尝试从粘贴的链接里自动反推并回填。

> **接口来源说明**：飞牛影视没有公开 API 文档，客户端用的是官方 Web 端
> （`trimemedia-web`）的 `POST /v/api/v1/login` 与 `POST /v/api/v1/item/list`，
> 并复刻其 `authx` 签名（`md5(api_key_path_nonce_timestamp_md5(body)_api_secret)`）。
> 因此：
> - 飞牛影视升级若改动签名密钥或字段名，可能需要在 `src/fnos.rs` 同步；
>   验签失败会明确报「invalid sign」。
> - 只支持 HTTP 或证书受信任的 HTTPS；自签名证书的 HTTPS 会连接失败。
> - 密码仅保存在本机 SQLite 中，与 Jellyfin Token 一样未做额外加密。
> - 每层目录自动分页，并检查返回总数；分页异常会明确报错。
> - 网盘挂载视频的列表时长可能为 `0`。客户端会自动探测真实媒体时长，首次获取可能较慢；探测失败会指出具体视频，避免生成缺课的计划。
> - 获取期间会显示当前目录已完成的视频数量和已用时间；时长探测最多同时处理 3 个视频，完成后仍按原课程顺序排列。网盘或 NAS 处理较慢时，首次获取可能需要几分钟。

#### 识别不全时怎么排查

如果发现「只识别到几个视频」，先确认使用了包含时长补全的新版本。
已实测的故障是：目录返回全部 32 个视频，其中 22 个 `duration=0`，旧版本直接跳过，最终只识别到 10 个。现在通过 `POST /v/api/v1/stream` 的 `level=0` 模式补全真实时长。

可用以下命令验证整个获取流程（末尾 `32` 是预期视频数，可替换或省略）：

```bash
FNOS_URL=http://192.168.1.10:5666 FNOS_USER=admin FNOS_PASS=密码 \
  cargo run --example live_check_fnos -- "<guid 或网页链接>" 32
```

仍有异常时，再跑接口自检工具，检查列表分页、目录类型和原始时长字段：

```bash
# 推荐用环境变量，避免密码出现在程序命令行参数中
FNOS_URL=http://192.168.1.10:5666 FNOS_USER=admin FNOS_PASS=密码 \
  cargo run --example fnos_probe -- "<guid 或网页链接>"
```

输出会逐层打印每个条目的类型、时长、被当作容器还是叶子（或为何被丢弃），
并把全部原始响应写入 `fnos-probe-dump.json`。判读要点：

诊断输出含媒体名称、NAS 路径和条目 ID，分享前请脱敏；诊断 JSON、本地 SQLite 配置和 `.env` 已加入 Git 忽略规则。不要使用 `git add -f` 强行上传这些文件。在终端直接输入含密码的环境变量赋值仍可能进入 shell 历史。

| 现象 | 结论 |
| --- | --- |
| 单页 `total` > 实际返回条数 | 需要多页读取，新版会自动处理 |
| `exclude_folder=1` 的条数远少于基线 | 当前用 `0` 是对的（保留文件夹） |
| 大量条目 `duration` 为 `0` 或为空 | 可能尚未探测网盘媒体信息；新版会尝试补全，失败时提示具体视频 |
| `✗ 容器「…」没有 guid 字段` | 该层无法下钻，内容会整层丢失 |

### 5. 应用打包

#### macOS (.app / .dmg)
项目提供了一键原生 DMG 打包脚本：
```bash
./tools/build_macos.sh
```
产物位置：
- `.app`：`target/release/bundle/osx/bili-planner.app`
- `.dmg`：`target/release/bundle/osx/bili-planner_0.2.0_aarch64.dmg`

#### Windows (.msi)
使用 [cargo-packager](https://docs.crabnebula.dev/packager/) 打包：
```bash
cargo install cargo-packager --locked
cargo packager --release
```
产物位于 `target/release/` 下的 `.msi` 安装包。

---

## 🧪 测试与质量保证

```bash
# 从仓库根目录运行架构、契约、格式、Clippy 与全量测试
bash tools/check.sh

# 端到端 API 真实联调验证示例
cargo run --example live_check -- "BV1ps4y1d73V" 30 all split
cargo run --example live_check_jellyfin -- "<Jellyfin_URL>" 30 all split
```

---

## 📄 开源许可证

本项目基于 [MIT License](LICENSE) 许可发布。
