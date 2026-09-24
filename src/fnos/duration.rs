use super::*;

pub(super) fn stream_duration(data: serde_json::Value) -> Result<i64> {
    #[derive(Deserialize)]
    struct StreamData {
        video_stream: FnItem,
    }
    let data: StreamData = serde_json::from_value(data)
        .map_err(|e| Error::data(format!("飞牛影视媒体信息解析失败：{e}")))?;
    // 成功响应中的 0 可能表示网盘媒体仍在探测，交给调用方有限重查。
    Ok(duration_of(&data.video_stream))
}

pub(super) fn wait_for_duration<F, W>(mut fetch: F, mut wait: W) -> Result<i64>
where
    F: FnMut() -> Result<i64>,
    W: FnMut(),
{
    for attempt in 1..=DURATION_POLL_ATTEMPTS {
        let duration = fetch()?;
        if duration > 0 {
            return Ok(duration);
        }
        if attempt < DURATION_POLL_ATTEMPTS {
            wait();
        }
    }
    Err(Error::data(format!(
        "飞牛影视媒体信息仍未准备就绪（已查询 {DURATION_POLL_ATTEMPTS} 次），请稍后重试。"
    )))
}

/// 有限并发探测，按原索引写回，避免完成先后顺序影响课程顺序。
/// progress 始终在调用线程执行；失败时不返回部分课程。
pub(super) fn complete_durations<F>(
    items: &mut [FnItem],
    resolve: F,
    progress: &mut dyn FnMut(String),
) -> Result<()>
where
    F: Fn(&FnItem) -> Result<i64> + Sync,
{
    let mut pending = Vec::new();
    let mut total = 0;
    for (index, item) in items.iter().enumerate() {
        if item.is_container() {
            if clean(item.guid.as_deref()).is_none() {
                return Err(Error::data(format!(
                    "飞牛影视目录「{}」缺少 ID，无法展开，当前结果不完整。",
                    item.display_title("未命名目录")
                )));
            }
        } else {
            total += 1;
            if duration_of(item) <= 0 {
                pending.push(index);
            }
        }
    }
    let mut ready = total - pending.len();
    progress(format!("当前目录：{ready} / {total} 个视频时长已就绪"));
    if pending.is_empty() {
        return Ok(());
    }
    progress(format!(
        "当前目录：{ready} / {total} 个视频时长已就绪，正在读取网盘媒体信息…"
    ));
    /// 工作线程 → 汇总线程的事件：开始探测 / 探测结束。
    /// 开始事件用于在长时间探测期间持续回报「正在探测哪个视频」。
    enum ProbeEvent {
        Started(usize),
        Finished(usize, Result<i64>),
    }
    let next = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let (tx, rx) = mpsc::channel();
    let mut resolved = Vec::new();
    let mut first_error = None;
    std::thread::scope(|scope| {
        let items = &*items;
        for _ in 0..DURATION_WORKERS.min(pending.len()) {
            let (tx, pending, next, failed, resolve) =
                (tx.clone(), &pending, &next, &failed, &resolve);
            scope.spawn(move || {
                while !failed.load(Ordering::Relaxed) {
                    let Some(&index) = pending.get(next.fetch_add(1, Ordering::Relaxed)) else {
                        break;
                    };
                    let item = &items[index];
                    if tx.send(ProbeEvent::Started(index)).is_err() {
                        break;
                    }
                    let result = resolve(item)
                        .and_then(|duration| {
                            if duration > 0 {
                                Ok(duration)
                            } else {
                                Err(Error::data("媒体探测未返回有效时长。"))
                            }
                        })
                        .map_err(|e| {
                            Error::data(format!(
                        "飞牛影视视频「{}」时长补全失败：{} 当前结果不完整，请稍后重新获取。",
                        item.display_title("未命名视频"), e.message()
                    ))
                        });
                    if result.is_err() {
                        failed.store(true, Ordering::Relaxed);
                    }
                    if tx.send(ProbeEvent::Finished(index, result)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        for event in rx {
            match event {
                ProbeEvent::Started(index) => {
                    // 网盘探测单个视频可能要等很久，开始时先回报标题，
                    // 用户才能区分「正在工作」和「卡住了」。
                    if first_error.is_none() {
                        let title = truncate(&items[index].display_title("未命名视频"), 24);
                        progress(format!(
                            "当前目录：{ready} / {total} 已就绪，正在探测「{title}」…"
                        ));
                    }
                }
                ProbeEvent::Finished(index, result) => match result {
                    Ok(duration) => {
                        resolved.push((index, duration));
                        ready += 1;
                        if first_error.is_none() {
                            progress(format!("当前目录：{ready} / {total} 个视频时长已就绪"));
                        }
                    }
                    Err(error) => {
                        if first_error.is_none() {
                            progress("视频时长探测失败，正在结束剩余请求…".to_string());
                            first_error = Some(error);
                        }
                    }
                },
            }
        }
    });
    if let Some(error) = first_error {
        return Err(error);
    }
    for (index, duration) in resolved {
        items[index].duration = Some(duration);
    }
    Ok(())
}
