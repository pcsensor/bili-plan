use super::*;

impl PlannerApp {
    pub(super) fn input_value(&self, state: &Entity<InputState>, cx: &App) -> String {
        state.read(cx).value().to_string()
    }

    pub(super) fn days(&self, cx: &App) -> Result<i64, String> {
        parse_days(&self.input_value(&self.days_input, cx))
    }

    pub(super) fn normalized_cloud_server_url(&self, cx: &App) -> Result<String, String> {
        let server = self.input_value(&self.cloud_server_input, cx);
        let server = server.trim().trim_end_matches('/').to_string();
        if server.is_empty() {
            return Err("请填写云端服务器地址。".to_string());
        }
        if !server.starts_with("https://") && !server.starts_with("http://") {
            return Err("云端地址必须以 https:// 或 http:// 开头。".to_string());
        }
        Ok(server)
    }

    /// 打开云端地址设置面板，并以已保存的地址作为编辑起点。
    pub(super) fn open_cloud_settings_action(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cloud_server_input.update(cx, |state, cx| {
            state.set_value(self.config.sync_server_url.clone(), window, cx)
        });
        self.cloud_server_test_result = None;
        self.cloud_settings_modal_open = true;
        cx.notify();
    }

    /// 保存云端地址。切换到另一台服务时，旧服务的设备绑定不再有效，必须重置。
    pub(super) fn save_cloud_server_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let server = match self.normalized_cloud_server_url(cx) {
            Ok(server) => server,
            Err(error) => {
                window.push_notification(Notification::warning(error), cx);
                return;
            }
        };
        let changed = server != self.config.sync_server_url.trim_end_matches('/');
        self.config.sync_server_url = server.clone();
        if changed {
            self.config.sync_device_token = None;
            self.config.sync_revision = 0;
            self.config.feishu_bound = false;
            self.config.feishu_user_name = None;
            self.config.telegram_bound = false;
            self.config.telegram_user_name = None;
        }
        save_config(&self.config);
        self.cloud_settings_modal_open = false;
        window.push_notification(
            Notification::success(if changed {
                format!("已保存云端地址 {server}；请重新绑定机器人。")
            } else {
                format!("已保存云端地址 {server}")
            }),
            cx,
        );
        cx.notify();
    }

    /// 异步检测 `/api/health`，避免测试连接时阻塞 GPUI 渲染线程。
    pub(super) fn test_cloud_server_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cloud_testing {
            return;
        }
        let server = match self.normalized_cloud_server_url(cx) {
            Ok(server) => server,
            Err(error) => {
                self.cloud_server_test_result = Some((error, false));
                cx.notify();
                return;
            }
        };
        self.cloud_testing = true;
        self.cloud_server_test_result = None;
        cx.notify();
        let health_url = format!("{server}/api/health");

        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let agent = ureq::Agent::config_builder()
                        .timeout_global(Some(std::time::Duration::from_secs(10)))
                        .build()
                        .new_agent();
                    let mut response = agent
                        .get(&health_url)
                        .call()
                        .map_err(|error| format!("连接失败：{error}"))?;
                    let body = response
                        .body_mut()
                        .read_to_string()
                        .map_err(|error| format!("读取健康检查响应失败：{error}"))?;
                    let payload: serde_json::Value = serde_json::from_str(&body)
                        .map_err(|error| format!("健康检查响应不是有效 JSON：{error}"))?;
                    if payload["status"].as_str() == Some("ok") {
                        Ok("连接成功：云端服务健康。".to_string())
                    } else {
                        Err("服务已响应，但不是 bili-plan-server 健康检查接口。".to_string())
                    }
                })
                .await;
            this.update_in(cx, |this, _window, cx| {
                this.cloud_testing = false;
                this.cloud_server_test_result = Some(match result {
                    Ok(message) => (message, true),
                    Err(error) => (error, false),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// 点击「获取视频信息」：前台做最小校验，网络请求放到后台执行器。
    pub(super) fn start_fetch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.phase, Phase::Loading) {
            return;
        }
        let input = self.input_value(&self.link_input, cx);
        let cookie = {
            let v = self.input_value(&self.cookie_input, cx);
            if v.trim().is_empty() {
                None
            } else {
                Some(v)
            }
        };
        let source = match self.source {
            SourceMode::Bilibili => FetchSource::Bilibili { cookie },
            SourceMode::Jellyfin => {
                let server_url = self.input_value(&self.jf_server_input, cx);
                let token = self.input_value(&self.jf_token_input, cx);
                // 前端先做最小校验，避免起任务后才报错；core 会再 trim 检查。
                if server_url.trim().is_empty() || token.trim().is_empty() {
                    window.push_notification(
                        Notification::warning("请填写 Jellyfin 服务器地址与 API Token。"),
                        cx,
                    );
                    return;
                }
                FetchSource::Jellyfin { server_url, token }
            }
            SourceMode::FnOs => {
                let mut base_url = self.input_value(&self.fnos_server_input, cx);
                let username = self.input_value(&self.fnos_user_input, cx);
                let password = self.input_value(&self.fnos_password_input, cx);
                // 服务器地址留空时，尝试从粘贴的链接反推 `scheme://host:port`
                // 并回填输入框——省去让用户从链接里手抄 NAS 地址。
                if base_url.trim().is_empty() {
                    if let Some(derived) = crate::jellyfin::extract_base_url(&input) {
                        base_url = derived;
                        self.fnos_server_input.update(cx, |state, cx| {
                            state.set_value(base_url.clone(), window, cx)
                        });
                    }
                }
                // 前端先做最小校验，避免起任务后才报错；core 会再 trim 检查。
                if base_url.trim().is_empty() {
                    window
                        .push_notification(Notification::warning("请填写飞牛影视服务器地址。"), cx);
                    return;
                }
                if username.trim().is_empty() || password.is_empty() {
                    window
                        .push_notification(Notification::warning("请填写飞牛影视账号与密码。"), cx);
                    return;
                }
                FetchSource::FnOs {
                    base_url,
                    username,
                    password,
                }
            }
        };

        self.phase = Phase::Loading;
        self.last_error = None;
        self.fetch_progress = "正在连接服务器…".to_string();
        self.fetch_elapsed_secs = 0;
        self.fetch_generation = self.fetch_generation.wrapping_add(1);
        let generation = self.fetch_generation;
        cx.notify();

        enum FetchMessage {
            Progress(String),
            Finished(Result<ReadyState, String>),
        }
        let source_mode = self.source;
        cx.spawn_in(window, async move |this, cx| {
            let fetch_input = input.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            let started = std::time::Instant::now();
            cx.background_executor()
                .spawn(async move {
                    let result = crate::core::fetch_and_parse_with_progress(
                        &fetch_input,
                        &source,
                        &mut |message| {
                            let _ = tx.send(FetchMessage::Progress(message));
                        },
                    );
                    let _ = tx.send(FetchMessage::Finished(result));
                })
                .detach();
            loop {
                let mut latest = None;
                let mut finished = None;
                loop {
                    match rx.try_recv() {
                        Ok(FetchMessage::Progress(message)) => latest = Some(message),
                        Ok(FetchMessage::Finished(result)) => {
                            finished = Some(result);
                            break;
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            finished = Some(Err("获取任务意外结束，请重新获取。".to_string()));
                            break;
                        }
                    }
                }
                let keep_waiting = this
                    .update_in(cx, |this, window, cx| {
                        // 切换来源或发起新任务后，旧任务不得覆盖新状态。
                        if this.fetch_generation != generation
                            || !matches!(this.phase, Phase::Loading)
                        {
                            return false;
                        }
                        if let Some(result) = finished {
                            this.on_fetched(result, input.clone(), source_mode, window, cx);
                            return false;
                        }
                        let elapsed = started.elapsed().as_secs();
                        let changed = latest.is_some() || elapsed != this.fetch_elapsed_secs;
                        if let Some(message) = latest {
                            this.fetch_progress = message;
                        }
                        this.fetch_elapsed_secs = elapsed;
                        if changed {
                            cx.notify();
                        }
                        true
                    })
                    .unwrap_or(false);
                if !keep_waiting {
                    break;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(250))
                    .await;
            }
        })
        .detach();
    }

    pub(super) fn on_fetched(
        &mut self,
        result: Result<ReadyState, String>,
        input: String,
        source_mode: SourceMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(mut rd) => {
                // 本次成功：记住凭证（Jellyfin / 飞牛影视）+ 记录搜索历史并写盘。
                if matches!(source_mode, SourceMode::Jellyfin) {
                    self.config.server_url = self
                        .input_value(&self.jf_server_input, cx)
                        .trim()
                        .to_string();
                    self.config.token = self
                        .input_value(&self.jf_token_input, cx)
                        .trim()
                        .to_string();
                }
                if matches!(source_mode, SourceMode::FnOs) {
                    self.config.fnos_server_url = self
                        .input_value(&self.fnos_server_input, cx)
                        .trim()
                        .trim_end_matches('/')
                        .to_string();
                    self.config.fnos_username = self
                        .input_value(&self.fnos_user_input, cx)
                        .trim()
                        .to_string();
                    // 密码不做 trim：前后空格可能是密码本身的一部分。
                    self.config.fnos_password = self.input_value(&self.fnos_password_input, cx);
                }
                record_history(&mut self.config, source_mode, &input, &rd.season_title);
                save_config(&self.config);
                // 获取成功后自动按当前天数与模式生成一次计划。
                match self.days(cx) {
                    Ok(days) => Self::run_generate(
                        &mut rd,
                        self.mode,
                        days,
                        &mut self.plan_table,
                        &mut self.window_expanded,
                        window,
                        cx,
                    ),
                    Err(e) => window.push_notification(Notification::warning(e), cx),
                }
                self.phase = Phase::Ready(rd);
                window.push_notification(Notification::success("已获取视频信息。"), cx);
            }
            Err(e) => {
                self.phase = Phase::Input;
                self.last_error = Some(e);
            }
        }
        cx.notify();
    }

    /// 在就绪状态上生成计划，并给出"天数 > 总时长"的提示。
    ///
    /// 以字段级参数规避借用冲突：调用方可能正持有 `self.phase` 的
    /// `&mut ReadyState` 借用。
    /// 首次生成计划时把窗口向右加宽，为计划表腾出右侧空间。
    /// 最大化 / 全屏下跳过；仅执行一次，后续尊重用户手动调整的尺寸。
    /// 宽度以屏幕右缘为上限——`resize` 不改变原点，超屏会截断右栏。
    pub(super) fn expand_window_for_plan(
        window: &mut Window,
        window_expanded: &mut bool,
        cx: &App,
    ) {
        if *window_expanded || !matches!(window.window_bounds(), gpui::WindowBounds::Windowed(_)) {
            return;
        }
        *window_expanded = true;
        let b = window.bounds();
        let want = b.size.width + PLAN_PANEL_WIDTH + px(32.);
        let mut new_w = want.min(b.size.width * 2.0);
        if let Some(display) = window.display(cx) {
            let avail = display.bounds().right() - px(12.) - b.origin.x;
            if avail < b.size.width {
                return; // 窗口已贴右缘，不再扩张，交由 flex 压缩左栏
            }
            new_w = new_w.min(avail);
        }
        window.resize(gpui::size(new_w, b.size.height));
    }

    pub(super) fn run_generate(
        rd: &mut ReadyState,
        mode: Mode,
        days: i64,
        plan_table: &mut Option<Entity<TableState<PlanTableDelegate>>>,
        window_expanded: &mut bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match generate_plan(rd, days, mode) {
            Ok(()) => {
                if let Some(p) = &rd.plan {
                    if days > p.total {
                        window.push_notification(
                            Notification::warning(format!(
                                "目标天数（{days}）大于总时长秒数（{}），部分日期将为空闲/休息日。",
                                p.total
                            )),
                            cx,
                        );
                    }
                }
                if let Some(plan) = &rd.plan {
                    Self::expand_window_for_plan(window, window_expanded, cx);
                    let delegate = PlanTableDelegate::new(plan);
                    *plan_table = Some(cx.new(|cx| {
                        let mut state = TableState::new(delegate, window, cx);
                        // 只读展示表：关闭行/列选择与排序（开启列宽拖拽自适应）。
                        state.col_selectable = false;
                        state.row_selectable = false;
                        state.col_movable = false;
                        state.col_resizable = true;
                        state.sortable = false;
                        state
                    }));
                }
            }
            Err(e) => window.push_notification(Notification::error(format!("错误：{e}")), cx),
        }
    }

    /// 点击「生成观看计划」。
    pub(super) fn regenerate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let days = match self.days(cx) {
            Ok(days) => days,
            Err(e) => {
                window.push_notification(Notification::warning(e), cx);
                return;
            }
        };
        if let Phase::Ready(rd) = &mut self.phase {
            Self::run_generate(
                rd,
                self.mode,
                days,
                &mut self.plan_table,
                &mut self.window_expanded,
                window,
                cx,
            );
        }
        cx.notify();
    }

    /// 切换科目统计范围；沿用旧行为：天数有效时立即重新生成。
    pub(super) fn set_selection(
        &mut self,
        sel: Selection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Phase::Ready(rd) = &mut self.phase {
            rd.selection = sel;
            rd.plan = None;
            self.plan_table = None;
        }
        if let Ok(days) = self.days(cx) {
            if let Phase::Ready(rd) = &mut self.phase {
                Self::run_generate(
                    rd,
                    self.mode,
                    days,
                    &mut self.plan_table,
                    &mut self.window_expanded,
                    window,
                    cx,
                );
            }
        }
        cx.notify();
    }

    pub(super) fn switch_source(
        &mut self,
        source: SourceMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // IMK 修复：切换前先让链接输入框失焦，避免 macOS 输入法框架
        // 在 placeholder 变更时唤醒已挂起的 IMKCFRunLoop（stderr 噪声来源）。
        if source != self.source {
            self.link_input.update(cx, |state, cx| {
                if state.focus_handle(cx).is_focused(window) {
                    window.blur();
                }
            });
        }
        if source != self.source && matches!(self.phase, Phase::Loading) {
            self.fetch_generation = self.fetch_generation.wrapping_add(1);
            self.phase = Phase::Input;
        }
        self.source = source;
        self.last_error = None;
        // 链接输入的 placeholder 随来源切换；仅在确实需要时设置，
        // 多余的 set_placeholder 会触发一次 IMK 标记文本重建。
        let placeholder = match source {
            SourceMode::Bilibili => {
                "https://www.bilibili.com/video/BV1ps4y1d73V 或 BV 号 或 sid=6789"
            }
            SourceMode::Jellyfin => "https://host/web/#!/details?id=xxx 或直接粘贴 item ID",
            SourceMode::FnOs => "http://host:5666/v/video?guid=fv_xxxx 或直接粘贴条目 guid",
        };
        self.link_input.update(cx, |state, cx| {
            state.set_placeholder(placeholder, window, cx)
        });
        // 来源切换：已识别的合集结构不再适用，丢弃旧的 Ready 状态。
        if matches!(self.phase, Phase::Ready(_)) {
            self.phase = Phase::Input;
            self.plan_table = None;
        }
        cx.notify();
    }

    pub(super) fn switch_mode(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        self.mode = mode;
        // 先取天数再进入 phase 借用，避免可变借用冲突。
        let days = self.days(cx).ok();
        // 模式变化后，旧计划不再适用；天数有效时立即按新模式重建。
        if let Phase::Ready(rd) = &mut self.phase {
            rd.plan = None;
            self.plan_table = None;
            if let Some(days) = days {
                Self::run_generate(
                    rd,
                    mode,
                    days,
                    &mut self.plan_table,
                    &mut self.window_expanded,
                    window,
                    cx,
                );
            }
        }
        cx.notify();
    }

    // -----------------------------------------------------------------------
    // 进度打卡与多科目管理交互动作
    // -----------------------------------------------------------------------
}
