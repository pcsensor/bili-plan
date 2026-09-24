use super::*;

/// `item/list` 的默认请求体。
///
/// 单独提出，便于诊断工具在它的基础上做变体实验。
pub fn item_list_body(parent_guid: &str) -> serde_json::Value {
    serde_json::json!({
        "parent_guid": parent_guid,
        // 0 = 保留文件夹子项，遍历时需要下钻。
        "exclude_folder": 0,
        "sort_column": "sort_title",
        "sort_type": "ASC",
        "page": 1,
        "page_size": PAGE_SIZE,
    })
}

/// 飞牛的页码从 1 开始。参考客户端：
/// https://github.com/jxxghp/MoviePilot/blob/v3/app/modules/trimemedia/api.py
/// 以 total 为准，不能根据请求的 page_size 判断末页（服务端可能限制页长）。
/// 无 total 时继续到空页；分页不前进、提前空页均报错，避免返回残缺结果。
pub(super) fn collect_pages<F>(parent_guid: &str, mut fetch: F) -> Result<ItemListData>
where
    F: FnMut(usize) -> Result<ItemListData>,
{
    let mut result = ItemListData::default();
    let mut seen = HashSet::new();
    let mut seen_pages = HashSet::new();
    for page in 1..=MAX_PAGES {
        let data = fetch(page)?;
        if result.mdb_name.is_none() {
            result.mdb_name = data.mdb_name;
        }
        if let Some(total) = data.total.filter(|n| *n >= 0) {
            if result.total.is_some_and(|previous| previous != total) {
                return Err(Error::data(
                    "飞牛影视目录总数在读取期间发生变化，请重新获取。",
                ));
            }
            result.total = Some(total);
        }
        if data.list.is_empty() {
            if result
                .total
                .is_some_and(|total| result.list.len() < total as usize)
            {
                return Err(Error::data(format!(
                    "飞牛影视目录 {parent_guid} 返回不完整：应有 {} 项，仅获取 {} 项，第 {page} 页为空。",
                    result.total.unwrap(), result.list.len()
                )));
            }
            return Ok(result);
        }
        if !seen_pages.insert(format!("{:?}", data.list)) {
            return Err(Error::data(format!(
                "飞牛影视目录 {parent_guid} 第 {page} 页重复，分页未前进。"
            )));
        }
        let before = result.list.len();
        for item in data.list {
            // 不按标题去重：同名视频是合法的不同文件。
            // 缺 guid 时保留条目，整页指纹另行检测重复页。
            if clean(item.guid.as_deref()).is_none_or(|guid| seen.insert(guid)) {
                result.list.push(item);
            }
        }
        if result.list.len() == before {
            return Err(Error::data(format!(
                "飞牛影视目录 {parent_guid} 第 {page} 页重复，分页未前进（已获取 {} 项）。请检查服务端分页接口。",
                result.list.len()
            )));
        }
        if let Some(total) = result.total {
            if result.list.len() > total as usize {
                return Err(Error::data("飞牛影视返回条数超过目录总数，请重新获取。"));
            }
            if result.list.len() == total as usize {
                return Ok(result);
            }
        }
    }
    Err(Error::data(
        "飞牛影视目录分页超过安全上限，已停止读取，未返回部分结果。",
    ))
}

pub(super) fn truncate(text: &str, max_chars: usize) -> String {
    let mut out: String = text.chars().take(max_chars).collect();
    if text.chars().count() > max_chars {
        out.push('…');
    }
    out
}
