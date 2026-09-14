//! 飞牛影视适配器测试：guid 解析、authx 签名、响应字段契约、分组规则。
//!
//! 与 `jellyfin_fixtures.rs` / `parse_fixtures.rs` 风格一致——纯逻辑、不触网。
//! 因为飞牛影视没有公开文档，这里的 fixture 就是**我方对响应形状的假设**：
//! 一旦真实抓包与此不符，从失败用例即可定位是哪一层的字段理解错了。

use bili_planner::fnos::{
    classify_children, derive_root_title, duration_of, episode_from, episode_title_of,
    extract_guid, gen_authx, md5_hex, order_items, FnItem, FnOsClient, ItemListData,
};
use bili_planner::parse::EpisodeItem;

/// 构造一个条目，减少每个用例的样板。
fn item(kind: &str, title: &str, duration: Option<i64>) -> FnItem {
    FnItem {
        kind: Some(kind.to_string()),
        title: Some(title.to_string()),
        duration,
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// 签名
// ---------------------------------------------------------------------------

#[test]
fn md5_matches_rfc1321_vectors() {
    // RFC 1321 附录 A.5 的测试向量。
    assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
    assert_eq!(md5_hex(b"a"), "0cc175b9c0f1b6a831c399e269772661");
    assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
    assert_eq!(
        md5_hex(b"message digest"),
        "f96b697d7cb7938d525a2f31aaf161d0"
    );
    assert_eq!(
        md5_hex(b"The quick brown fox jumps over the lazy dog"),
        "9e107d9d372bb6826bd81d3542a419d6"
    );
}

#[test]
fn authx_is_deterministic_and_covers_every_signed_input() {
    let path = "/v/api/v1/item/list";
    let body = br#"{"parent_guid":"fv_x","nonce":"123456"}"#;
    let baseline = gen_authx(path, body, "654321", 1_700_000_000_000);

    // 同一组输入必须得到同一签名。
    assert_eq!(baseline, gen_authx(path, body, "654321", 1_700_000_000_000));
    assert!(baseline.starts_with("nonce=654321&timestamp=1700000000000&sign="));

    // 路径、body、nonce、timestamp 任一变化都必须改变签名。
    assert_ne!(
        baseline,
        gen_authx("/v/api/v1/login", body, "654321", 1_700_000_000_000)
    );
    assert_ne!(
        baseline,
        gen_authx(path, b"{}", "654321", 1_700_000_000_000)
    );
    assert_ne!(baseline, gen_authx(path, body, "111111", 1_700_000_000_000));
    assert_ne!(baseline, gen_authx(path, body, "654321", 1_700_000_000_001));
}

#[test]
fn authx_signs_the_exact_bytes_that_get_sent() {
    // serde_json::Value 会按键名排序，JS 的 JSON.stringify 保留插入序。
    // 因此必须「签名哪份字节就发哪份字节」——本用例把这条不变式钉住：
    // 键序不同 → 字节不同 → 签名不同，二者必须一起走。
    let asc = gen_authx("/v/api/v1/item/list", br#"{"a":1,"b":2}"#, "111111", 1);
    let desc = gen_authx("/v/api/v1/item/list", br#"{"b":2,"a":1}"#, "111111", 1);
    assert_ne!(asc, desc);

    // 反过来：只要字节一致，无论语义如何，签名一致。
    let same_bytes_again = gen_authx("/v/api/v1/item/list", br#"{"a":1,"b":2}"#, "111111", 1);
    assert_eq!(asc, same_bytes_again);
}

// ---------------------------------------------------------------------------
// 输入解析
// ---------------------------------------------------------------------------

#[test]
fn extract_guid_accepts_links_and_bare_ids() {
    let guid = "fv_30006e2fdaa44c7aac2c3cb25c10121d";
    for input in [
        format!("http://10.0.0.6:5666/v/video?guid={guid}"),
        format!("http://10.0.0.6:5666/v/video?guid={guid}&x=1"),
        format!("http://10.0.0.6:5666/v/index/#!/details?guid={guid}"),
        format!("http://10.0.0.6:5666/v/video/{guid}"),
        format!("{guid}/"),
        guid.to_string(),
    ] {
        assert_eq!(
            extract_guid(&input),
            Some(guid.to_string()),
            "输入 {input} 应识别出 guid"
        );
    }
}

#[test]
fn extract_guid_handles_html_escaped_query_and_other_id_keys() {
    // HTML 转义后的分隔符。
    assert_eq!(
        extract_guid("http://h:5666/v/video?guid=fv_abcdef0123456789&amp;from=share"),
        Some("fv_abcdef0123456789".to_string())
    );
    // item_guid / parent_guid 同样可用。
    assert_eq!(
        extract_guid("http://h:5666/v/video?parent_guid=abcdef0123456789"),
        Some("abcdef0123456789".to_string())
    );
}

#[test]
fn extract_guid_rejects_navigation_urls_and_junk() {
    for input in [
        "http://host:5666/v/index/#!/details",
        "http://host:5666/v/video",
        "http://host:5666/",
        "",
        "   ",
        "https://example.com/some/long/path",
    ] {
        assert_eq!(extract_guid(input), None, "输入 {input} 不应识别出 guid");
    }
}

// ---------------------------------------------------------------------------
// 响应字段契约
// ---------------------------------------------------------------------------

/// 与 `types.ts` 里 `ItemListResponse` 形状一致的 fixture。
const ITEM_LIST_SEASON: &str = r#"
{
  "mdb_name": "番剧库",
  "mdb_category": "Others",
  "top_dir": "",
  "dir": "/vol1/Media/番剧/某番剧/Season 1",
  "total": 3,
  "list": [
    {
      "guid": "fv_ep1",
      "type": "Episode",
      "title": "起点",
      "tv_title": "某番剧",
      "parent_title": "第 1 季",
      "parent_guid": "fv_season1",
      "season_number": 1,
      "episode_number": 1,
      "duration": 1440,
      "runtime": 24
    },
    {
      "guid": "fv_ep2",
      "type": "Episode",
      "title": "转折",
      "parent_title": "第 1 季",
      "season_number": 1,
      "episode_number": 2,
      "duration": 1500,
      "runtime": 25
    },
    {
      "guid": "fv_ep0",
      "type": "Episode",
      "title": "第0集特别篇",
      "episode_number": 0,
      "duration": "600"
    }
  ]
}
"#;

#[test]
fn item_list_fixture_maps_fields_as_expected() {
    let data: ItemListData = serde_json::from_str(ITEM_LIST_SEASON).expect("fixture 可解析");
    assert_eq!(data.mdb_name.as_deref(), Some("番剧库"));
    assert_eq!(data.total, Some(3));
    assert_eq!(data.list.len(), 3);

    let first = &data.list[0];
    assert_eq!(first.guid.as_deref(), Some("fv_ep1"));
    assert_eq!(first.kind.as_deref(), Some("Episode"));
    assert_eq!(first.title.as_deref(), Some("起点"));
    // duration 单位是秒（1440s = 24min，与 runtime 自洽）。
    assert_eq!(duration_of(first), 1440);
    assert_eq!(first.episode_number, Some(1));
    assert_eq!(first.season_number, Some(1));

    // 集号为 0 时不拼「第0集」前缀。
    let special = &data.list[2];
    assert_eq!(episode_title_of(special), "第0集特别篇");
    // 数字以字符串返回也能解析。
    assert_eq!(duration_of(special), 600);
}

#[test]
fn runtime_is_used_only_when_duration_is_missing_or_zero() {
    let mut it = item("Episode", "甲", Some(1440));
    it.runtime = Some(999);
    assert_eq!(duration_of(&it), 1440, "duration 优先");

    it.duration = Some(0);
    assert_eq!(
        duration_of(&it),
        999 * 60,
        "duration 为 0 时用 runtime 分钟换算"
    );

    it.runtime = None;
    assert_eq!(duration_of(&it), 0, "两者都缺时为 0");

    it.duration = None;
    assert_eq!(duration_of(&it), 0);
}

#[test]
fn episode_title_prefers_numbered_form() {
    let mut it = item("Episode", "起点", Some(1440));
    it.episode_number = Some(3);
    assert_eq!(episode_title_of(&it), "第3集 起点");

    it.title = Some("   ".to_string());
    assert_eq!(episode_title_of(&it), "第3集", "标题空白时只留集号");

    it.episode_number = None;
    it.title = Some("起点".to_string());
    assert_eq!(episode_title_of(&it), "起点");
}

#[test]
fn episode_from_skips_items_without_duration_but_keeps_untitled_ones() {
    assert!(episode_from(&item("Episode", "无时长", Some(0))).is_none());
    assert!(episode_from(&item("Episode", "无时长", None)).is_none());

    // 标题缺失但有duration：用占位名而不是静默丢弃。
    let untitled = item("Video", "   ", Some(600));
    let episode = episode_from(&untitled).expect("有时长就应纳入");
    assert_eq!(episode.title, "未命名条目");
    assert_eq!(episode.duration, 600);
}

// ---------------------------------------------------------------------------
// 类型判定与排序
// ---------------------------------------------------------------------------

#[test]
fn container_types_are_recognized_case_insensitively() {
    for kind in [
        "Directory",
        "Folder",
        "TV",
        "Season",
        "collectionfolder",
        "  BoxSet  ",
    ] {
        assert!(
            item(kind, "容器", Some(9999)).is_container(),
            "{kind} 应视为容器"
        );
    }
    for kind in ["Movie", "Episode", "Video", "movie"] {
        assert!(
            !item(kind, "叶子", Some(0)).is_container(),
            "{kind} 应视为叶子"
        );
    }
}

#[test]
fn unknown_type_falls_back_to_duration_heuristic() {
    // 飞牛新增类型时：有时长当叶子，无时长当容器继续下钻。
    assert!(!item("BrandNewType", "x", Some(600)).is_container());
    assert!(item("BrandNewType", "x", None).is_container());
    assert!(item("", "x", None).is_container());
}

#[test]
fn ordering_only_applies_when_every_item_is_numbered() {
    let mut a = item("Episode", "甲", Some(100));
    a.episode_number = Some(2);
    a.season_number = Some(1);
    let mut b = item("Episode", "乙", Some(100));
    b.episode_number = Some(1);
    b.season_number = Some(1);

    let ordered = order_items(vec![a.clone(), b.clone()]);
    assert_eq!(ordered[0].title.as_deref(), Some("乙"));

    // 有一条缺集号 → 保持服务端顺序不动。
    let mut no_number = item("Episode", "丙", Some(100));
    no_number.episode_number = None;
    let kept = order_items(vec![a, no_number]);
    assert_eq!(kept[0].title.as_deref(), Some("甲"));
}

// ---------------------------------------------------------------------------
// 分组（`expand` 用闭包注入，无需网络）
// ---------------------------------------------------------------------------

/// 把 guid 映射为固定叶子，验证分组时「谁被展开」。
fn expand_stub<'a>(
    mapping: &'a [(&'a str, &'a str, i64)],
) -> impl FnMut(&str) -> bili_planner::Result<Vec<EpisodeItem>> + 'a {
    move |guid: &str| {
        Ok(mapping
            .iter()
            .filter(|(g, _, _)| *g == guid)
            .map(|(_, title, duration)| EpisodeItem {
                title: (*title).to_string(),
                duration: *duration,
            })
            .collect())
    }
}

#[test]
fn multi_season_series_becomes_one_group_per_season() {
    let mut season1 = item("Season", "第 1 季", None);
    season1.guid = Some("g1".into());
    season1.parent_title = Some("某番剧".into());
    let mut season2 = item("Season", "第 2 季", None);
    season2.guid = Some("g2".into());

    let expand = expand_stub(&[("g1", "S1E1", 1440), ("g2", "S2E1", 1500)]);
    let (title, groups, structure) =
        classify_children(vec![season1, season2], Some("番剧库"), expand).unwrap();

    assert_eq!(title, "某番剧");
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].name, "第 1 季");
    assert_eq!(groups[0].episodes[0].title, "S1E1");
    assert_eq!(groups[1].name, "第 2 季");
    assert_eq!(groups[1].episodes[0].duration, 1500);
    assert!(structure.contains("多层级合集"));
}

#[test]
fn flat_episode_list_becomes_a_single_course_named_after_the_parent() {
    let mut e1 = item("Episode", "起点", Some(1440));
    e1.episode_number = Some(1);
    e1.parent_title = Some("某番剧".into());
    let mut e2 = item("Episode", "转折", Some(1500));
    e2.episode_number = Some(2);

    let (title, groups, structure) =
        classify_children(vec![e2, e1], Some("番剧库"), |_| unreachable!()).unwrap();

    assert_eq!(title, "某番剧");
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].name, "某番剧");
    // 集号齐备 → 调用方（order_items）应先排序；此处验证标题形态。
    assert_eq!(groups[0].episodes.len(), 2);
    assert!(structure.contains("单合集"));
}

#[test]
fn loose_videos_sit_ahead_of_containers_as_a_side_group() {
    let mut season = item("Season", "第 1 季", None);
    season.guid = Some("g1".into());
    season.parent_title = Some("某番剧".into());
    let mut bonus = item("Video", "花絮", Some(300));

    let (title, groups, _) = classify_children(
        vec![season, {
            bonus.parent_title = Some("某番剧".into());
            bonus
        }],
        None,
        expand_stub(&[("g1", "S1E1", 1440)]),
    )
    .unwrap();

    assert_eq!(title, "某番剧");
    // 散件组排在容器组之前，避免被季淹没。
    assert_eq!(groups[0].name, "某番剧（散件）");
    assert_eq!(groups[0].episodes[0].title, "花絮");
    assert_eq!(groups[1].name, "第 1 季");
}

#[test]
fn empty_container_subtrees_are_dropped_without_failing() {
    let mut empty_season = item("Season", "空季", None);
    empty_season.guid = Some("g-empty".into());
    empty_season.parent_title = Some("某番剧".into());
    let mut real = item("Episode", "正片", Some(600));

    let (_, groups, _) = classify_children(
        vec![empty_season, {
            real.parent_title = Some("某番剧".into());
            real
        }],
        None,
        |_| Ok(Vec::new()),
    )
    .unwrap();

    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].episodes[0].title, "正片");
}

#[test]
fn classify_reports_actionable_errors_when_nothing_is_plannable() {
    // 全部缺时长。
    let err = classify_children(
        vec![item("Episode", "无时长", Some(0))],
        Some("库"),
        |_| Ok(Vec::new()),
    )
    .expect_err("无可用内容应报错");
    assert!(err.message().contains("未找到可规划的视频"));

    // 空列表。
    assert!(classify_children(Vec::new(), Some("库"), |_| Ok(Vec::new())).is_err());
}

#[test]
fn root_title_degrades_from_parent_title_to_mdb_name_to_generic() {
    let mut with_parent = item("Season", "第 1 季", None);
    with_parent.parent_title = Some("某番剧".into());
    assert_eq!(derive_root_title(&[with_parent], Some("番剧库")), "某番剧");

    let mut with_tv = item("Episode", "第一集", Some(600));
    with_tv.tv_title = Some("某剧".into());
    assert_eq!(derive_root_title(&[with_tv], Some("番剧库")), "某剧");

    assert_eq!(
        derive_root_title(&[item("Season", "第 1 季", None)], Some("番剧库")),
        "番剧库"
    );
    assert_eq!(derive_root_title(&[], None), "飞牛影视合集");
}

// ---------------------------------------------------------------------------
// 客户端构造
// ---------------------------------------------------------------------------

#[test]
fn client_normalizes_base_url_and_never_debug_prints_the_password() {
    let client = FnOsClient::new("  http://nas.local:5666///  ", " admin ", "s3cret");
    assert_eq!(client.base_url, "http://nas.local:5666");
    // 账号密码原样保留，由调用方决定是否 trim。
    assert_eq!(client.username, " admin ");
    assert_eq!(client.password, "s3cret");

    let debug = format!("{client:?}");
    assert!(debug.contains("http://nas.local:5666"));
    assert!(!debug.contains("s3cret"), "Debug 输出不应泄露密码：{debug}");
}
