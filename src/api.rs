//! HTTP 层：B 站 API 请求、重试、错误映射（中文提示）。

use std::time::Duration;

use crate::error::{Error, Result};
use crate::model::{ArchiveItem, ArchivesResponse, ViewData, ViewResponse};

pub const API_VIEW: &str = "https://api.bilibili.com/x/web-interface/view";
pub const API_SEASON_ARCHIVES: &str =
    "https://api.bilibili.com/x/polymer/web-space/seasons_archives_list";
pub const DEFAULT_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
(KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";
pub const DEFAULT_TIMEOUT_SECS: u64 = 15;
pub const DEFAULT_RETRIES: u32 = 3;

/// GET 请求并解析 JSON（失败重试 `retries` 次，间隔 1.2s * attempt）。
pub fn http_get_json(
    url: &str,
    cookie: Option<&str>,
    timeout_secs: u64,
    retries: u32,
    referer: &str,
) -> Result<serde_json::Value> {
    let mut last_err = String::from("未知错误");
    for attempt in 1..=retries {
        match try_get_json(url, cookie, timeout_secs, referer) {
            Ok(v) => return Ok(v),
            Err(e) => {
                last_err = e;
                if attempt < retries {
                    std::thread::sleep(Duration::from_secs_f64(1.2 * attempt as f64));
                }
            }
        }
    }
    Err(Error::network(format!(
        "网络请求失败（已重试 {retries} 次）：{last_err}"
    )))
}

fn try_get_json(
    url: &str,
    cookie: Option<&str>,
    timeout_secs: u64,
    referer: &str,
) -> std::result::Result<serde_json::Value, String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(timeout_secs)))
        .build()
        .new_agent();
    let mut req = agent
        .get(url)
        .header("User-Agent", DEFAULT_UA)
        .header("Referer", referer)
        .header("Accept", "application/json, text/plain, */*")
        .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8");
    if let Some(c) = cookie {
        if !c.trim().is_empty() {
            req = req.header("Cookie", c);
        }
    }
    let mut resp = req.call().map_err(|e| e.to_string())?;
    let raw = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    let obj: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("JSON 解析失败：{e}"))?;
    if !obj.is_object() {
        return Err("API 返回的不是 JSON 对象".to_string());
    }
    Ok(obj)
}

/// 检查 API 响应的 data 是否为非空对象。
fn data_object(value: &serde_json::Value, data_err: &str) -> Result<serde_json::Value> {
    let data = value
        .get("data")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let empty = data.as_object().is_none_or(|o| o.is_empty());
    if data.is_null() || empty {
        return Err(Error::data(data_err));
    }
    Ok(data)
}

/// 获取视频详情（内含 ugc_season 结构）。
pub fn fetch_view(bvid: &str, cookie: Option<&str>) -> Result<ViewData> {
    let url = format!("{API_VIEW}?bvid={bvid}");
    let obj = http_get_json(
        &url,
        cookie,
        DEFAULT_TIMEOUT_SECS,
        DEFAULT_RETRIES,
        &format!("https://www.bilibili.com/video/{bvid}"),
    )?;
    let resp: ViewResponse = serde_json::from_value(obj.clone())
        .map_err(|e| Error::network(format!("view 响应解析失败：{e}")))?;
    if resp.code != 0 {
        let msg = resp.message.unwrap_or_else(|| "未知错误".to_string());
        let hint = match resp.code {
            -412 | -403 => "（疑似触发风控，可尝试传入 SESSDATA 等登录信息）",
            -101 | -400 => "（可能需要登录信息）",
            -404 => "（视频不存在或已失效）",
            _ => "",
        };
        return Err(Error::api(format!(
            "Bilibili API 返回错误：code={} message={}{}",
            resp.code, msg, hint
        )));
    }
    let data = data_object(&obj, "Bilibili API 未返回有效数据")?;
    let view: ViewData = serde_json::from_value(data)
        .map_err(|e| Error::network(format!("view 响应解析失败：{e}")))?;
    Ok(view)
}

/// 分页拉取合集全部视频（扁平列表）。
pub fn fetch_season_archives(season_id: i64, cookie: Option<&str>) -> Result<Vec<ArchiveItem>> {
    let mut archives: Vec<ArchiveItem> = Vec::new();
    let mut page_num: i64 = 1;
    let page_size: i64 = 100;
    while page_num <= 500 {
        let url = format!(
            "{API_SEASON_ARCHIVES}?season_id={season_id}&page_num={page_num}&page_size={page_size}&sort_reverse=false"
        );
        let obj = http_get_json(
            &url,
            cookie,
            DEFAULT_TIMEOUT_SECS,
            DEFAULT_RETRIES,
            "https://www.bilibili.com/",
        )?;
        let resp: ArchivesResponse = serde_json::from_value(obj)
            .map_err(|e| Error::network(format!("合集归档响应解析失败：{e}")))?;
        if resp.code != 0 {
            let msg = resp.message.unwrap_or_else(|| "未知错误".to_string());
            return Err(Error::api(format!(
                "合集归档接口错误：code={} message={}",
                resp.code, msg
            )));
        }
        let d = resp.data.unwrap_or_default();
        let items = d.archives.unwrap_or_default();
        let items_len = items.len();
        archives.extend(items);
        let page = d.page.unwrap_or_default();
        let count = if page.total > 0 {
            page.total
        } else {
            page.count
        };
        let size = if page.page_size > 0 {
            page.page_size
        } else if page.size > 0 {
            page.size
        } else {
            page_size
        };
        let num = if page.page_num > 0 {
            page.page_num
        } else if page.num > 0 {
            page.num
        } else {
            page_num
        };
        if items_len == 0 {
            break;
        }
        if count > 0 {
            let total_pages = if size > 0 {
                (count + size - 1) / size
            } else {
                num
            };
            if num >= total_pages {
                break;
            }
        } else if (items_len as i64) < size {
            break;
        }
        page_num = num + 1;
    }
    Ok(archives)
}
