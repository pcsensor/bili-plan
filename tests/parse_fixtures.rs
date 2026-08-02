//! 结构识别测试：覆盖全部 5 类结构 + 分栏缺失回退 + pages 截断回退。

use bili_planner::model::ViewData;
use bili_planner::parse::{episode_items, extract_bvid, extract_sid, parse_groups};
use bili_planner::Result;

fn view(json: &str) -> ViewData {
    serde_json::from_str(json).expect("fixture parses")
}

fn fallback_ok(_sid: i64, _cookie: Option<&str>) -> Result<Vec<bili_planner::model::ArchiveItem>> {
    Ok(vec![
        bili_planner::model::ArchiveItem {
            title: "归档视频1".into(),
            duration: 600,
        },
        bili_planner::model::ArchiveItem {
            title: "归档视频2".into(),
            duration: 900,
        },
    ])
}

#[test]
fn multi_section() {
    let v = view(
        r#"{
        "title": "合集A",
        "ugc_season": {
            "id": 1,
            "title": "合集A",
            "sections": [
                {"title": "第一章", "episodes": [
                    {"title": "1-1", "duration": 100, "pages": [{"page": 1, "part": "a", "duration": 100}]},
                    {"title": "1-2", "duration": 200, "pages": [{"page": 1, "part": "b", "duration": 200}]}
                ]},
                {"title": "第二章", "episodes": [
                    {"title": "2-1", "duration": 300, "pages": [{"page": 1, "part": "c", "duration": 300}]}
                ]},
                {"title": "", "episodes": [
                    {"title": "3-1", "duration": 400, "pages": [{"page": 1, "part": "d", "duration": 400}]}
                ]}
            ]
        }
    }"#,
    );
    let fb = fallback_ok;
    let r = parse_groups(&v, None, &fb).expect("ok");
    assert_eq!(r.season_title, "合集A");
    assert_eq!(r.structure, "多分栏合集（每个分栏视为一门科目）");
    assert_eq!(r.groups.len(), 3);
    assert_eq!(r.groups[0].name, "第一章");
    assert_eq!(r.groups[2].name, "分栏3");
    assert_eq!(r.groups[0].episodes.len(), 2);
}

#[test]
fn single_section_multi_p() {
    let v = view(
        r#"{
        "ugc_season": {
            "id": 2,
            "title": "语法精讲",
            "sections": [{"title": "全部", "episodes": [
                {"title": "动词", "duration": 1000, "pages": [
                    {"page": 1, "part": "现在时", "duration": 500},
                    {"page": 2, "part": "过去时", "duration": 500}
                ]},
                {"title": "名词", "duration": 800, "pages": [
                    {"page": 1, "part": "单数", "duration": 400},
                    {"page": 2, "part": "复数", "duration": 400}
                ]},
                {"title": "形容词", "duration": 600, "pages": [
                    {"page": 1, "part": "比较级", "duration": 300},
                    {"page": 2, "part": "最高级", "duration": 300}
                ]}
            ]}]
        }
    }"#,
    );
    let fb = fallback_ok;
    let r = parse_groups(&v, None, &fb).expect("ok");
    assert_eq!(
        r.structure,
        "单分栏合集（分栏内每个多P视频视为一门科目，含多个分P）"
    );
    assert_eq!(r.groups.len(), 3);
    assert_eq!(r.groups[0].name, "动词");
    assert_eq!(r.groups[0].episodes.len(), 2);
    assert_eq!(r.groups[0].episodes[0].title, "P1 现在时");
}

#[test]
fn single_section_normal() {
    let v = view(
        r#"{
        "ugc_season": {
            "id": 3,
            "title": "合集B",
            "sections": [{"title": "主线", "episodes": [
                {"title": "ep1", "duration": 120},
                {"title": "ep2", "duration": 240}
            ]}]
        }
    }"#,
    );
    let fb = fallback_ok;
    let r = parse_groups(&v, None, &fb).expect("ok");
    assert_eq!(r.structure, "单分栏合集（整个合集视为一门课程）");
    assert_eq!(r.groups.len(), 1);
    assert_eq!(r.groups[0].name, "主线");
    assert_eq!(r.groups[0].episodes.len(), 2);
}

#[test]
fn no_season_multi_p() {
    let v = view(
        r#"{
        "title": "纪录片",
        "duration": 2000,
        "pages": [
            {"page": 1, "part": "上集", "duration": 1000},
            {"page": 2, "part": "下集", "duration": 1000}
        ]
    }"#,
    );
    let fb = fallback_ok;
    let r = parse_groups(&v, None, &fb).expect("ok");
    assert_eq!(r.structure, "非合集视频（含多个分P）");
    assert_eq!(r.groups.len(), 1);
    assert_eq!(r.groups[0].name, "多P视频");
    assert_eq!(r.groups[0].episodes[0].title, "纪录片｜P1 上集");
}

#[test]
fn no_season_single() {
    let v = view(r#"{"title": "单视频", "duration": 666}"#);
    let fb = fallback_ok;
    let r = parse_groups(&v, None, &fb).expect("ok");
    assert_eq!(r.structure, "非合集视频（单视频）");
    assert_eq!(r.groups[0].name, "单视频");
    assert_eq!(
        r.groups[0].episodes,
        vec![bili_planner::parse::EpisodeItem {
            title: "单视频".into(),
            duration: 666,
        }]
    );
}

#[test]
fn empty_sections_falls_back() {
    let v = view(
        r#"{
        "ugc_season": {
            "id": 42,
            "title": "无分栏合集",
            "sections": []
        }
    }"#,
    );
    let fb = fallback_ok;
    let r = parse_groups(&v, None, &fb).expect("ok");
    assert_eq!(r.structure, "分栏缺失，已通过归档接口拉取扁平列表");
    assert_eq!(r.groups.len(), 1);
    assert_eq!(r.groups[0].name, "无分栏合集");
    assert_eq!(r.groups[0].episodes.len(), 2);
    assert_eq!(r.groups[0].episodes[0].title, "归档视频1");
}

#[test]
fn pages_sum_mismatch_falls_back_to_arc() {
    let v = view(
        r#"{
        "ugc_season": {
            "id": 5,
            "title": "T",
            "sections": [{"title": "S", "episodes": [
                {"title": "整集", "duration": 9999, "arc": {"duration": 10000}, "pages": [
                    {"page": 1, "part": "p1", "duration": 100},
                    {"page": 2, "part": "p2", "duration": 100}
                ]}
            ]}]
        }
    }"#,
    );
    let fb = fallback_ok;
    let r = parse_groups(&v, None, &fb).expect("ok");
    assert_eq!(r.groups[0].episodes.len(), 1);
    assert_eq!(r.groups[0].episodes[0].title, "整集");
    assert_eq!(r.groups[0].episodes[0].duration, 10000);
}

#[test]
fn episode_items_pages_ok() {
    let ep: bili_planner::model::Episode = serde_json::from_str(
        r#"{
        "title": "多P", "duration": 200, "pages": [
            {"page": 1, "part": "A", "duration": 100},
            {"page": 2, "part": "B", "duration": 100}
        ]
    }"#,
    )
    .unwrap();
    let items = episode_items(&ep);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].title, "P1 A");
    assert_eq!(items[1].title, "P2 B");
    assert_eq!(items.iter().map(|i| i.duration).sum::<i64>(), 200);
}

#[test]
fn extract_bvid_and_sid() {
    assert_eq!(
        extract_bvid("https://www.bilibili.com/video/BV1ps4y1d73V?p=1").unwrap(),
        "BV1ps4y1d73V"
    );
    assert_eq!(extract_bvid("BV1xx411c7mD").unwrap(), "BV1xx411c7mD");
    assert!(extract_bvid("没有BV号的文本").is_err());
    assert_eq!(
        extract_sid("https://space.bilibili.com/12345/channel/collectiondetail?sid=6789"),
        Some(6789)
    );
    assert_eq!(extract_sid("abc?season_id=123&x=1"), Some(123));
    assert_eq!(extract_sid("没有sid的文本"), None);
}
