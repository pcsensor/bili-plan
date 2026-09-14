//! 飞牛影视接口自检工具（诊断用，不参与应用逻辑）。
//!
//! 当出现「只识别到几个视频」时，一次运行即可验证三种可能成因：
//!
//! 1. **`exclude_folder` 语义相反** → 文件夹被服务端过滤掉，只返回根下的散件
//! 2. **`item/list` 分页截断** → 响应里的 `total` 大于实际 `list` 条数
//! 3. **字段名 / duration 与假设不符** → 条目被判为"无时长"而静默丢弃
//!
//! # 用法
//!
//! ```bash
//! # 方式一：位置参数
//! cargo run --example fnos_probe -- http://192.168.1.10:5666 admin 密码 "guid 或网页链接"
//!
//! # 方式二：环境变量（推荐，避免密码出现在程序命令行参数中）
//! FNOS_URL=http://192.168.1.10:5666 FNOS_USER=admin FNOS_PASS=密码 \
//!   cargo run --example fnos_probe -- "guid 或网页链接"
//! ```
//!
//! 输出与 `fnos-probe-dump.json` 含私有媒体名称、NAS 路径和 ID，分享前需脱敏。
//! 在终端直接输入带密码的环境变量赋值也可能进入 shell 历史。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use bili_planner::fnos::{duration_of, episode_from};
use bili_planner::fnos::{extract_guid, item_list_body, FnItem, FnOsClient, ItemListData};
use serde_json::{json, Value};

/// 递归走查的最大深度（生产代码上限是 8，这里只到 4 以免输出过长）。
const WALK_DEPTH: usize = 4;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (url, user, pass, input) = match resolve_args(&args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let guid = match extract_guid(&input) {
        Some(g) => g,
        None => {
            eprintln!(
                "✗ 无法从 `{input}` 中识别 guid。\n\
                 请贴飞牛影视网页链接（含 ?guid= 参数或路径末段），或直接给 guid。"
            );
            std::process::exit(2);
        }
    };

    println!("=== 飞牛影视接口自检 ===");
    println!("服务器：{url}");
    println!("账号：{user}");
    println!("条目 guid：{guid}");

    let client = FnOsClient::new(url, user, pass);
    let mut dump: Vec<(String, Value)> = Vec::new();

    // -----------------------------------------------------------------
    // 第一步：登录 + 首次取数（同时验证签名与凭据）
    // -----------------------------------------------------------------
    println!("\n=== 1. 登录与首次取数 ===");
    match client.item_list_raw(&guid) {
        Ok(data) => {
            dump.push(("baseline".to_string(), data.clone()));
            println!("✓ 登录并取数成功（说明 authx 签名与账号密码都正确）");
            report(&data);
        }
        Err(e) => {
            println!("✗ 失败：{}", e.message());
            println!("\n如果错误含「invalid sign」，说明飞牛的签名密钥已变更；");
            println!("如果错误含「登录」，请确认账号是飞牛影视账号（不是飞牛系统账号）。");
            return;
        }
    }

    // 保留一个不带分页参数的对照组，生产默认请求已包含分页。
    let mut base = item_list_body(&guid);
    if let Some(object) = base.as_object_mut() {
        object.remove("page");
        object.remove("page_size");
    }

    // -----------------------------------------------------------------
    // 第二步：参数语义对照实验
    // -----------------------------------------------------------------
    println!("\n=== 2. 参数对照实验（同一 guid，只改请求体） ===");
    println!("目的：确认 exclude_folder 语义，以及 item/list 是否分页。");
    println!("判读方法：比较各变体的「首页指纹」（条数 + 首条 guid）。");
    println!("  · exclude_folder=1 的条数明显少于基线 → 0 才是保留文件夹，当前实现正确");
    println!("  · 带 page_size 后条数变多 → 存在分页，需要补分页逻辑");
    println!("  · page=2 的首条 guid 与基线不同 → 确认了分页参数名");

    let mut variants: Vec<(&str, Value)> = Vec::new();

    let mut exclude_one = base.clone();
    exclude_one["exclude_folder"] = json!(1);
    variants.push(("exclude_folder=1", exclude_one));

    let mut without_field = base.clone();
    if let Some(obj) = without_field.as_object_mut() {
        obj.remove("exclude_folder");
    }
    variants.push(("不带 exclude_folder", without_field));

    let mut big_page = base.clone();
    big_page["page_size"] = json!(200);
    variants.push(("page_size=200", big_page));

    let mut page_two = base.clone();
    page_two["page_size"] = json!(20);
    page_two["page"] = json!(2);
    variants.push(("page=2&page_size=20", page_two));

    let mut page_one = base.clone();
    page_one["page_size"] = json!(20);
    page_one["page"] = json!(1);
    variants.push(("page=1&page_size=20", page_one));

    let mut page_index_two = base.clone();
    page_index_two["page_size"] = json!(200);
    page_index_two["page_index"] = json!(2);
    variants.push(("page_index=2&page_size=200", page_index_two));

    for (label, body) in variants {
        probe_variant(&client, label, body, &mut dump);
    }

    // -----------------------------------------------------------------
    // 第三步：递归走查
    // -----------------------------------------------------------------
    println!("\n=== 3. 原始列表走查（未分页合并、未补全媒体时长） ===");
    println!("说明：▸ 表示容器，· 表示已有时长的叶子，✗ 表示需要补全或无法展开。");
    walk(&client, &guid, 0, &mut dump);

    // -----------------------------------------------------------------
    // 第四步：落盘
    // -----------------------------------------------------------------
    let dump_path = "fnos-probe-dump.json";
    match serde_json::to_string_pretty(&dump_to_value(&dump)) {
        Ok(text) => match fs::write(dump_path, text) {
            Ok(()) => println!("\n✓ 原始响应已写入 {dump_path}（含私有媒体信息，分享前请脱敏）"),
            Err(e) => println!("\n✗ 写入 {dump_path} 失败：{e}"),
        },
        Err(e) => println!("\n✗ 序列化 dump 失败：{e}"),
    }
}

fn dump_to_value(dump: &[(String, Value)]) -> Value {
    Value::Array(
        dump.iter()
            .map(|(label, data)| json!({ "label": label, "data": data }))
            .collect(),
    )
}

/// 解析命令行参数：4 个位置参数，或「1 个位置参数 + 三个环境变量」。
fn resolve_args(args: &[String]) -> Result<(String, String, String, String), String> {
    let env = |names: &[&str]| -> Option<String> {
        names
            .iter()
            .find_map(|n| std::env::var(n).ok())
            .filter(|v| !v.trim().is_empty())
    };
    match args.len() {
        4 => Ok((
            args[0].clone(),
            args[1].clone(),
            args[2].clone(),
            args[3].clone(),
        )),
        1 => {
            let url = env(&["FNOS_URL", "FNOS_BASE_URL"]);
            let user = env(&["FNOS_USER", "FNOS_USERNAME"]);
            let pass = env(&["FNOS_PASS", "FNOS_PASSWORD"]);
            match (url, user, pass) {
                (Some(url), Some(user), Some(pass)) => {
                    Ok((url, user, pass, args[0].clone()))
                }
                _ => Err(
                    "只给了 1 个参数时，需要同时设置 FNOS_URL / FNOS_USER / FNOS_PASS 环境变量。"
                        .to_string(),
                ),
            }
        }
        _ => Err("用法：\n\
                   cargo run --example fnos_probe -- <base_url> <username> <password> <guid|链接>\n\
                   FNOS_URL=... FNOS_USER=... FNOS_PASS=... cargo run --example fnos_probe -- <guid|链接>"
            .to_string()),
    }
}

/// 打印一次响应的规模、类型分布、字段全集与时长缺失情况。
fn report(data: &Value) {
    if let Some(arr) = data.as_array() {
        println!("  ⚠️  data 是数组而非对象，长度为 {}", arr.len());
        println!("  → 与本适配器假设的 `{{total, list}}` 结构不符，需要改解析。");
        return;
    }

    let total = data.get("total").and_then(|v| v.as_i64());
    let list = data.get("list").and_then(|v| v.as_array());
    let len = list.map(|l| l.len()).unwrap_or(0);

    println!("  total={total:?}  实际返回 list.len()={len}");
    if let Some(t) = total {
        if t > len as i64 {
            println!("  ⚠️  total({t}) > 实际返回({len}) —— 存在分页截断，需要补分页");
        }
    }
    println!(
        "  data 的一级字段：{:?}",
        data.as_object()
            .map(|o| o.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default()
    );

    let Some(list) = list else {
        println!("  ⚠️  data 里没有 `list` 字段，需要改解析。");
        return;
    };

    let mut by_type: BTreeMap<String, usize> = BTreeMap::new();
    let mut keys: BTreeSet<String> = BTreeSet::new();
    let mut no_duration = 0usize;
    for item in list {
        let kind = item
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("<无 type 字段>")
            .to_string();
        *by_type.entry(kind).or_default() += 1;
        if let Some(obj) = item.as_object() {
            keys.extend(obj.keys().cloned());
        }
        let usable = serde_json::from_value::<FnItem>(item.clone())
            .map(|it| duration_of(&it) > 0)
            .unwrap_or(false);
        if !usable {
            no_duration += 1;
        }
    }
    println!("  type 分布：{by_type:?}");
    println!("  条目字段全集：{keys:?}");
    println!("  无有效时长的条目：{no_duration}/{len}");
    if no_duration == len && len > 0 {
        println!("  ⚠️  全部条目都取不到时长 —— 大概率字段名或单位与假设不符");
    }

    for (i, item) in list.iter().take(5).enumerate() {
        let title = item.get("title").and_then(|v| v.as_str()).unwrap_or("?");
        let kind = item.get("type").and_then(|v| v.as_str()).unwrap_or("?");
        let guid = item
            .get("guid")
            .and_then(|v| v.as_str())
            .unwrap_or("<无 guid>");
        let duration = item.get("duration");
        let runtime = item.get("runtime");
        println!("    [{i}] guid={guid} type={kind} duration={duration:?} runtime={runtime:?} title={title}");
    }

    // 首页指纹：跨变体比较条数与首条 guid，即可判定分页行为。
    let first = list.first();
    let first_guid = first
        .and_then(|it| it.get("guid"))
        .and_then(|v| v.as_str())
        .unwrap_or("<无 guid>");
    let first_title = first
        .and_then(|it| it.get("title"))
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    println!("  ▸ 首页指纹：len={len}  first_guid={first_guid}  first_title={first_title}");
}

/// 用变体请求体打一次，并报告结果差异。
fn probe_variant(client: &FnOsClient, label: &str, body: Value, dump: &mut Vec<(String, Value)>) {
    println!("\n--- 变体：{label} ---");
    println!(
        "  请求体：{}",
        serde_json::to_string(&body).unwrap_or_default()
    );
    match client.item_list_with_body(body) {
        Ok(data) => {
            dump.push((label.to_string(), data.clone()));
            report(&data);
        }
        Err(e) => println!("  ✗ 该变体失败：{}", e.message()),
    }
}

/// 走查原始列表；生产入口还会分页、探测并补全时长。
fn walk(client: &FnOsClient, guid: &str, depth: usize, dump: &mut Vec<(String, Value)>) {
    let indent = "  ".repeat(depth + 1);
    let data = match client.item_list_raw(guid) {
        Ok(v) => v,
        Err(e) => {
            println!("{indent}✗ item/list 失败：{}", e.message());
            return;
        }
    };
    dump.push((format!("walk-depth{depth}-{guid}"), data.clone()));

    let typed: ItemListData = serde_json::from_value(data.clone()).unwrap_or_default();
    let total = data.get("total").and_then(|v| v.as_i64());
    println!(
        "{indent}[深度 {depth}] guid={guid}  total={total:?}  返回 {} 条",
        typed.list.len()
    );
    if let Some(t) = total {
        if t > typed.list.len() as i64 {
            println!(
                "{indent}  ⚠️  本层被截断：total={t}，只拿到 {}",
                typed.list.len()
            );
        }
    }

    for item in &typed.list {
        let title = item.display_title("?");
        if item.is_container() {
            let child = item
                .guid
                .as_deref()
                .map(str::trim)
                .filter(|g| !g.is_empty());
            match child {
                Some(child) if depth < WALK_DEPTH => {
                    println!("{indent}  ▸ 容器「{title}」type={:?} → 下钻", item.kind);
                    walk(client, child, depth + 1, dump);
                }
                Some(_) => {
                    println!(
                        "{indent}  ▸ 容器「{title}」type={:?}（已达走查深度上限）",
                        item.kind
                    );
                }
                None => {
                    println!(
                        "{indent}  ✗ 容器「{title}」没有 guid 字段 —— 无法下钻，其内容会全部丢失"
                    );
                }
            }
        } else {
            match episode_from(item) {
                Some(episode) => {
                    println!(
                        "{indent}  · 叶子「{}」{} 秒",
                        episode.title, episode.duration
                    )
                }
                None => println!(
                    "{indent}  ✗ 待补全「{title}」type={:?} duration={:?} runtime={:?} —— 列表未提供时长，生产入口会探测媒体信息",
                    item.kind, item.duration, item.runtime
                ),
            }
        }
    }
}
