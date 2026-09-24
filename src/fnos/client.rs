use super::*;

// ---------------------------------------------------------------------------
// HTTP 客户端
// ---------------------------------------------------------------------------

/// 单次请求的判定结果，用于区分「该重试」和「该重新登录」。
enum Outcome {
    Data(serde_json::Value),
    /// 签名被拒：换一组 nonce/timestamp 重试即可。
    BadSign,
    /// 登录态失效：需要清缓存重新登录。
    Expired,
    /// 其它业务错误，不再重试。
    Failed(String),
}

/// 对外暴露的失败原因。
enum PostError {
    Expired,
    Fatal(String),
}

/// 飞牛影视 REST 客户端：服务器地址 + 账号密码，token 惰性登录并缓存。
pub struct FnOsClient {
    pub base_url: String,
    pub username: String,
    pub password: String,
    token: Mutex<Option<String>>,
    /// 复用同一 HTTP Agent：探测网盘视频时长会发出大量小请求，
    /// keep-alive 连接复用能避免每次请求都重建 TCP/TLS 连接。
    agent: ureq::Agent,
}

impl std::fmt::Debug for FnOsClient {
    /// 手写 Debug，避免密码进入日志。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FnOsClient")
            .field("base_url", &self.base_url)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

impl FnOsClient {
    pub fn new(
        base_url: impl Into<String>,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        Self {
            base_url: base_url.into().trim().trim_end_matches('/').to_string(),
            username: username.into(),
            password: password.into(),
            token: Mutex::new(None),
            agent: Self::build_agent(),
        }
    }

    fn build_agent() -> ureq::Agent {
        ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(DEFAULT_TIMEOUT_SECS)))
            .build()
            .new_agent()
    }

    /// 当前 token（无则登录）。
    fn token(&self) -> Result<String> {
        if let Some(token) = self.token.lock().unwrap().as_ref() {
            return Ok(token.clone());
        }
        self.login()
    }

    fn login(&self) -> Result<String> {
        let body = serde_json::json!({
            "app_name": APP_NAME,
            "username": self.username,
            "password": self.password,
        });
        // 登录自身不能再触发「重新登录」，因此 allow_relogin = false。
        let value = self
            .send(API_LOGIN, body, false)
            .map_err(|e| Error::api(format!("飞牛影视登录失败：{e}")))?;
        let data: LoginData = serde_json::from_value(value)
            .map_err(|e| Error::network(format!("飞牛影视登录响应解析失败：{e}")))?;
        let token = data
            .token
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| Error::api("飞牛影视登录成功但未返回 token。"))?;
        *self.token.lock().unwrap() = Some(token.clone());
        Ok(token)
    }

    /// 带登录态重试的请求：token 失效时清缓存重登一次。
    fn send(
        &self,
        path: &str,
        body: serde_json::Value,
        allow_relogin: bool,
    ) -> std::result::Result<serde_json::Value, String> {
        let max_attempts = if allow_relogin { 2 } else { 1 };
        let mut last_err = String::from("未知错误");
        for attempt in 0..max_attempts {
            let token = if allow_relogin {
                self.token().map_err(|e| e.message().to_string())?
            } else {
                String::new()
            };
            match self.post(path, &body, &token) {
                Ok(value) => return Ok(value),
                Err(PostError::Expired) => {
                    last_err = "登录态已失效".to_string();
                    *self.token.lock().unwrap() = None;
                    if attempt + 1 >= max_attempts {
                        break;
                    }
                }
                Err(PostError::Fatal(message)) => return Err(message),
            }
        }
        Err(format!("{last_err}（已尝试 {max_attempts} 次）"))
    }

    /// 单次 POST：签名 → 发送 → 解析包裹 → 判定。
    fn post(
        &self,
        path: &str,
        body: &serde_json::Value,
        token: &str,
    ) -> std::result::Result<serde_json::Value, PostError> {
        let url = format!("{}{}", self.base_url, path);
        let mut last_err = String::from("未知错误");
        for attempt in 1..=DEFAULT_RETRIES {
            match self.try_post(&url, path, body, token) {
                Ok(Outcome::Data(value)) => return Ok(value),
                Ok(Outcome::Expired) => return Err(PostError::Expired),
                Ok(Outcome::Failed(message)) => return Err(PostError::Fatal(message)),
                Ok(Outcome::BadSign) => {
                    last_err = "服务端拒绝签名（invalid sign）".to_string();
                }
                Err(err) => last_err = err,
            }
            if attempt < DEFAULT_RETRIES {
                std::thread::sleep(Duration::from_secs_f64(1.2 * attempt as f64));
            }
        }
        Err(PostError::Fatal(format!(
            "飞牛影视请求失败（已重试 {DEFAULT_RETRIES} 次）：{last_err}"
        )))
    }

    fn try_post(
        &self,
        url: &str,
        path: &str,
        body: &serde_json::Value,
        token: &str,
    ) -> std::result::Result<Outcome, String> {
        // 1. 先注入防重放 nonce，再序列化——之后签名与发送都用这份字节。
        let mut payload = body.clone();
        if let Some(object) = payload.as_object_mut() {
            object.insert(
                "nonce".to_string(),
                serde_json::Value::String(random_nonce()),
            );
        }
        let bytes = serde_json::to_vec(&payload).map_err(|e| format!("构造请求体失败：{e}"))?;

        // 2. authx 用的 nonce 与 body 里的防重放 nonce 是两个独立随机值。
        let authx = gen_authx(path, &bytes, &random_nonce(), now_ms());

        let mut request = self
            .agent
            .post(url)
            .header("User-Agent", crate::api::DEFAULT_UA)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .header("Authx", &authx);
        if !token.is_empty() {
            request = request.header("Authorization", token);
        }

        let mut response = request
            .send(bytes)
            .map_err(|e| format!("网络请求失败：{e}{}", local_network_hint()))?;
        let raw = response
            .body_mut()
            .read_to_string()
            .map_err(|e| format!("读取响应失败：{e}"))?;

        let envelope: Envelope<serde_json::Value> = serde_json::from_str(&raw)
            .map_err(|e| format!("响应不是合法 JSON（{e}）：{}", truncate(&raw, 200)))?;

        let code = envelope.code.unwrap_or(0);
        let message = envelope.msg.clone().unwrap_or_default();
        match code {
            0 => Ok(Outcome::Data(
                envelope.data.unwrap_or(serde_json::Value::Null),
            )),
            CODE_INVALID_SIGN => Ok(Outcome::BadSign),
            CODE_UNAUTHENTICATED => Ok(Outcome::Expired),
            _ => Ok(Outcome::Failed(format!("接口返回 code={code} {message}"))),
        }
    }

    /// 分页拉取某容器（媒体库/合集/季/文件夹）的全部直属子项。
    pub fn item_list(&self, parent_guid: &str) -> Result<ItemListData> {
        self.item_list_with_progress(parent_guid, &mut |_| {})
    }

    /// 同 [`FnOsClient::item_list`]，但每翻一页就回报进度。
    ///
    /// 网盘挂载的大目录分页可能很慢，回报进度避免界面长时间静默；
    /// 单页就能读完的目录（绝大多数）不回报，避免刷屏。
    pub fn item_list_with_progress(
        &self,
        parent_guid: &str,
        progress: &mut dyn FnMut(String),
    ) -> Result<ItemListData> {
        let mut fetched = 0usize;
        collect_pages(parent_guid, |page| {
            let mut body = item_list_body(parent_guid);
            body["page"] = serde_json::json!(page);
            let value = self.item_list_with_body(body)?;
            let data: ItemListData = serde_json::from_value(value)
                .map_err(|e| Error::network(format!("飞牛影视 item/list 响应解析失败：{e}")))?;
            fetched = fetched.saturating_add(data.list.len());
            if page > 1 {
                progress(format!("正在读取目录：第 {page} 页，累计 {fetched} 项…"));
            }
            Ok(data)
        })
    }

    /// 用自定义请求体调用 `item/list`，原样返回 `data`。
    ///
    /// 仅供诊断使用：飞牛没有公开参数语义，当结构识别异常或飞牛改版时，
    /// 用它做对照实验（换 `exclude_folder`、试分页参数），比猜更快。
    pub fn item_list_with_body(&self, body: serde_json::Value) -> Result<serde_json::Value> {
        self.send(API_ITEM_LIST, body, true).map_err(Error::api)
    }

    /// 拉取某容器子项的**原始**响应，不做任何字段映射。
    pub fn item_list_raw(&self, parent_guid: &str) -> Result<serde_json::Value> {
        self.item_list_with_body(item_list_body(parent_guid))
    }

    /// 列表中的网盘视频可能尚未探测，duration 为 0。
    /// 使用官方前端 /stream 的 Required(0) 模式获取媒体信息，不启动播放。
    fn resolve_duration(&self, item: &FnItem) -> Result<i64> {
        let media_guid = match clean(item.media_guid.as_deref()) {
            Some(guid) => guid,
            None => {
                let guid = clean(item.guid.as_deref())
                    .ok_or_else(|| Error::data("视频缺少条目 ID 和文件 ID，无法探测时长。"))?;
                let info = self
                    .send(API_PLAY_INFO, serde_json::json!({"item_guid": guid}), true)
                    .map_err(Error::api)?;
                clean(info.get("media_guid").and_then(|v| v.as_str()))
                    .ok_or_else(|| Error::data("视频信息未返回文件 ID，无法探测时长。"))?
            }
        };
        wait_for_duration(
            || {
                let data = self
                    .send(
                        API_STREAM,
                        serde_json::json!({
                            "media_guid": media_guid,
                            "level": 0,
                            "ip": "bili-planner",
                            "header": {"User-Agent": [crate::api::DEFAULT_UA]},
                        }),
                        true,
                    )
                    .map_err(Error::api)?;
                stream_duration(data)
            },
            || std::thread::sleep(Duration::from_secs(1)),
        )
    }

    pub(super) fn complete_durations(
        &self,
        items: &mut [FnItem],
        progress: &mut dyn FnMut(String),
    ) -> Result<()> {
        complete_durations(items, |item| self.resolve_duration(item), progress)
    }
}
