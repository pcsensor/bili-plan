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
- **科学计划排期**：
  - 支持 `split`（按时长精准切分，自动计算跨天分割时间戳）与 `whole`（单集完整排期）两种模式。
  - 支持全集或指定单一科目排期、自定义目标完成天数与休息日标注。
- **本地进度打卡**：
  - 内置打卡系统与进度环，支持每日进度完成度统计与日历打卡状态追踪。
- **Neo-Brutalist 视觉风格**：
  - 基于 Rust + [GPUI](https://gpui.rs) / [gpui-component](https://github.com/longbridge/gpui-component) 实现，纯 GPU 硬件加速渲染，冷启动毫秒级响应，内置亮/暗双色主题。

### 2. 云端服务与飞书机器人 (Cloud Server)
- **多端实时打卡同步**：桌面端生成的计划可一键同步至云端，保持进度最新。
- **飞书机器人交互**：
  - **每日定时推送**：早 08:30 自动推送当日学习任务卡片；晚 21:30 自动检查并督促未打卡科目。
  - **富文本交互卡片**：直接在飞书聊天界面点击按钮完成单集打卡，即时更新卡片进度条。
  - **快速指令**：向机器人发送“打卡”、“我的计划”随时获取最新学习进度与交互卡片。
- **极简绑定流程**：桌面端生成 6 位临时验证码，私聊飞书机器人发送即可瞬间完成设备与账号绑定。

---

## 📂 项目结构

```
.
├── src/                      # 桌面客户端源码 (GPUI + 核心业务)
│   ├── app.rs                # 桌面端主界面与交互状态机
│   ├── core.rs               # 业务编排层（解析/计划/打卡/云端同步）
│   ├── api.rs / parse.rs     # B 站适配器与合集结构提取
│   ├── jellyfin.rs           # Jellyfin 媒体库适配器
│   └── plan.rs / export.rs   # 核心计划生成算法与文本导出
├── server/                   # 云端服务与飞书机器人后端 (Rust Axum)
│   ├── Dockerfile            # 多阶段构建 Dockerfile
│   ├── docker-compose.yml    # 一键部署编排文件
│   ├── .env.example          # 环境变量模板文件
│   └── src/                  # 服务端源码 (Axum + 飞书 OpenAPI + 调度器)
├── assets/ / icons/          # 静态资产与跨平台图标
└── tools/                    # 自动化打包构建脚本
```

---

## 🚀 云端服务部署指南 (Server Deployment)

云端服务负责计划的云端存储、飞书开放平台回调接入以及每日定时推送通知。

### 1. 准备工作：创建飞书自建应用

1. 登录 [飞书开放平台开发者后台](https://open.feishu.cn/app) 并创建“企业自建应用”。
2. **获取凭据**：在 **“凭证与基础信息”** 页面获取 `App ID` 和 `App Secret`。
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
在服务器上创建工作目录并拉取项目（或仅拷贝 `server/` 目录）：

```bash
mkdir -p /opt/bili-plan-server
cd /opt/bili-plan-server
```

创建 `.env` 文件（可参考 `server/.env.example`）：
```env
PORT=3005
FEISHU_APP_ID=cli_xxxxxxxxxxxxxx
FEISHU_APP_SECRET=xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
DATA_DIR=/app/data
```

#### 步骤 2：启动容器
确保目录下包含 `Dockerfile`、`docker-compose.yml`、`Cargo.toml`、`src/` 及 `.env`：

```bash
# 构建并后台启动
docker compose up -d --build

# 查看运行日志与健康状态
docker compose logs -f
```

---

### 3. 方式二：原生二进制 / Systemd 部署

#### 步骤 1：本地/服务器构建
```bash
cd server
cargo build --release
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
3. 点击 **“生成绑定码”**，界面将显示一个 6 位数字绑定码（5 分钟有效）。
4. 在飞书中打开自建机器人单聊，发送这 6 位数字。
5. 桌面端自动提示绑定成功，之后每次本地排期或打卡均会自动同步到飞书！

### 3. 应用打包

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
# 运行全部单元测试与集成测试
cargo test

# 运行代码规范检查
cargo clippy --all-targets
cargo fmt --all -- --check

# 端到端 API 真实联调验证示例
cargo run --example live_check -- "BV1ps4y1d73V" 30 all split
cargo run --example live_check_jellyfin -- "<Jellyfin_URL>" 30 all split
```

---

## 📄 开源许可证

本项目基于 [MIT License](LICENSE) 许可发布。