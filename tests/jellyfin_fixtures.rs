//! Jellyfin 适配器测试：链接解析、单位换算、季分组、Item 分派。
//! 与 parse_fixtures.rs / plan_vectors.rs 风格一致，纯逻辑（不触网）；
//! 真实端到端验证由 examples/live_check_jellyfin.rs 走真实 API。

use bili_planner::jellyfin::{
    classify_item, episode_title, extract_base_url, extract_item_id, group_episodes_by_season,
    group_episodes_by_series, ticks_to_secs, Item,
};

fn item(name: &str, type_name: &str, ticks: i64, idx: i64, season: Option<&str>) -> Item {
    Item {
        id: Some("x".into()),
        name: Some(name.into()),
        type_name: Some(type_name.into()),
        run_time_ticks: Some(ticks),
        index_number: Some(idx),
        season_name: season.map(|s| s.to_string()),
        series_name: None,
        is_folder: false,
    }
}

/// 带 `series_name` 的 Item 构造便利函数（文件夹场景）。
fn item_with_series(
    name: &str,
    type_name: &str,
    ticks: i64,
    idx: i64,
    season: Option<&str>,
    series: Option<&str>,
) -> Item {
    let mut it = item(name, type_name, ticks, idx, season);
    it.series_name = series.map(|s| s.to_string());
    it
}

#[test]
fn extract_item_id_from_link() {
    // 用户的实际链接：`#/list?parentId=...&serverId=...`——之前版本识别不到。
    assert_eq!(
        extract_item_id(
            "https://jellyfin.pcsensor.cloud/web/#/list?parentId=699396e43b7061237d5b0c43086c7e42&serverId=eebf26e5024f4fb59c9ff3eabfbadaef"
        ),
        Some("699396e43b7061237d5b0c43086c7e42".into())
    );
    // hashbang 路由 + serverId 不误匹配
    assert_eq!(
        extract_item_id(
            "https://media.example.com:8096/web/index.html#!/details?id=abc-123&serverId=zzz"
        ),
        Some("abc-123".into())
    );
    // 新路由（无 hashbang）
    assert_eq!(
        extract_item_id("https://host/web/#/details?id=xyz"),
        Some("xyz".into())
    );
    // 大小写不敏感 + Id 在 serverId 之前出现
    assert_eq!(
        extract_item_id("?Id=GUID1&serverId=GUID2"),
        Some("GUID1".into())
    );
    // seasonId / seriesId 也识别
    assert_eq!(
        extract_item_id("https://host/web/#/details?seasonId=S7"),
        Some("S7".into())
    );
    assert_eq!(extract_item_id("?seriesId=SER1"), Some("SER1".into()));
    // 裸 ID
    assert_eq!(extract_item_id("abc-123-def"), Some("abc-123-def".into()));
    // 含 / 视为非裸 ID（避免把链接当 ID）
    assert_eq!(extract_item_id("https://host/path"), None);
    assert_eq!(extract_item_id("含 空格的串"), None);
    assert_eq!(extract_item_id(""), None);
    // 仅有 serverId 不能误匹配
    assert_eq!(extract_item_id("https://host/?serverId=only"), None);
}

#[test]
fn extract_base_url_from_link() {
    assert_eq!(
        extract_base_url("https://media.example.com:8096/web/index.html#!/details?id=abc"),
        Some("https://media.example.com:8096".into())
    );
    assert_eq!(
        extract_base_url("http://host/path?x=1"),
        Some("http://host".into())
    );
    assert_eq!(extract_base_url("abc-123"), None);
    assert_eq!(extract_base_url("host/path"), None);
}

#[test]
fn ticks_conversion() {
    assert_eq!(ticks_to_secs(Some(10_000_000)), 1);
    assert_eq!(ticks_to_secs(Some(36_000_000_000)), 3600);
    assert_eq!(ticks_to_secs(None), 0);
    assert_eq!(ticks_to_secs(Some(0)), 0);
}

#[test]
fn episode_title_format() {
    assert_eq!(
        episode_title(&item("集合论", "Episode", 1, 3, Some("第一季"))),
        "第3集 集合论"
    );
    assert_eq!(episode_title(&item("", "Episode", 1, 5, None)), "第5集");
    assert_eq!(
        episode_title(&item("无集号视频", "Episode", 1, 0, None)),
        "无集号视频"
    );
}

#[test]
fn group_by_season_preserves_order() {
    let eps = vec![
        item("ep1", "Episode", 1, 1, Some("第一季")),
        item("ep2", "Episode", 1, 2, Some("第一季")),
        item("ep3", "Episode", 1, 1, Some("第二季")),
        item("ep4", "Episode", 1, 0, None),
    ];
    let g = group_episodes_by_season(&eps);
    assert_eq!(g.len(), 3);
    assert_eq!(g[0].name, "第一季");
    assert_eq!(g[0].episodes.len(), 2);
    assert_eq!(g[1].name, "第二季");
    assert_eq!(g[2].name, "未分季");
    assert_eq!(g[2].episodes[0].title, "ep4");
}

#[test]
fn classify_series_multi_season() {
    let series = item("我的课程", "Series", 0, 0, None);
    let eps = vec![
        item("ep1", "Episode", 10_000_000 * 3600, 1, Some("上")),
        item("ep2", "Episode", 10_000_000 * 3600, 2, Some("上")),
        item("ep3", "Episode", 10_000_000 * 3600, 1, Some("下")),
    ];
    let (title, groups, structure) = classify_item(&series, eps).expect("ok");
    assert_eq!(title, "我的课程");
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].name, "上");
    assert_eq!(groups[0].episodes.len(), 2);
    assert_eq!(groups[1].episodes.len(), 1);
    assert_eq!(structure, "Jellyfin 多季合集（每季视为一门科目）");
}

#[test]
fn classify_series_single_season() {
    let series = item("短课程", "Series", 0, 0, None);
    let eps = vec![item("ep1", "Episode", 10_000_000 * 1800, 1, Some("正片"))];
    let (_, groups, structure) = classify_item(&series, eps).expect("ok");
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].episodes[0].duration, 1800);
    assert_eq!(structure, "Jellyfin 单季合集（整个合集视为一门课程）");
}

#[test]
fn classify_season_input() {
    let season = item("第一季", "Season", 0, 1, None);
    let eps = vec![
        item("ep1", "Episode", 10_000_000 * 3600, 1, Some("第一季")),
        item("ep2", "Episode", 10_000_000 * 3600, 2, Some("第一季")),
    ];
    let (title, groups, structure) = classify_item(&season, eps).expect("ok");
    assert_eq!(title, "第一季");
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].name, "第一季");
    assert_eq!(groups[0].episodes.len(), 2);
    assert_eq!(structure, "Jellyfin 单季合集（整个合集视为一门课程）");
}

#[test]
fn classify_movie() {
    let movie = item("一集电影", "Movie", 10_000_000 * 5400, 0, None);
    let (title, groups, structure) = classify_item(&movie, vec![]).expect("ok");
    assert_eq!(title, "一集电影");
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].name, "单视频");
    assert_eq!(groups[0].episodes[0].duration, 5400);
    assert_eq!(structure, "Jellyfin 单视频");
}

#[test]
fn classify_episode_single() {
    let ep = item("单集", "Episode", 10_000_000 * 1500, 7, None);
    let (title, groups, _) = classify_item(&ep, vec![]).expect("ok");
    assert_eq!(title, "单集");
    assert_eq!(groups[0].episodes[0].title, "单集");
    assert_eq!(groups[0].episodes[0].duration, 1500);
}

#[test]
fn classify_missing_duration_errors() {
    let movie = item("无时长", "Movie", 0, 0, None);
    let err = classify_item(&movie, vec![]).unwrap_err();
    assert!(err.message().contains("RunTimeTicks"));
}

#[test]
fn classify_series_missing_duration_errors() {
    let series = item("空课程", "Series", 0, 0, None);
    let eps = vec![item("ep1", "Episode", 0, 1, Some("季"))];
    let err = classify_item(&series, eps).unwrap_err();
    assert!(err.message().contains("RunTimeTicks"));
}

#[test]
fn classify_unsupported_type_errors() {
    let music = item("歌单", "MusicAlbum", 0, 0, None);
    let err = classify_item(&music, vec![]).unwrap_err();
    assert!(err.message().contains("MusicAlbum"));
    assert!(err
        .message()
        .contains("Series/Season/Movie/Episode/CollectionFolder/Folder/UserView/BoxSet/Playlist"));
}

#[test]
fn group_episodes_by_series_handles_disjoint_order() {
    // 递归接口不保证按 Series 排序：测试 find-or-insert 而非连续假设。
    let eps = vec![
        item_with_series(
            "ep1",
            "Episode",
            10_000_000 * 3600,
            1,
            None,
            Some("离散数学"),
        ),
        item_with_series(
            "ep2",
            "Episode",
            10_000_000 * 3600,
            1,
            None,
            Some("高等数学"),
        ),
        item_with_series(
            "ep3",
            "Episode",
            10_000_000 * 3600,
            2,
            None,
            Some("离散数学"),
        ),
    ];
    let g = group_episodes_by_series(&eps);
    assert_eq!(g.len(), 2);
    assert_eq!(g[0].name, "离散数学");
    assert_eq!(g[0].episodes.len(), 2);
    assert_eq!(g[1].name, "高等数学");
    assert_eq!(g[1].episodes.len(), 1);
}

#[test]
fn group_episodes_by_series_falls_back_to_season() {
    let eps = vec![
        item_with_series("ep1", "Episode", 10_000_000 * 3600, 1, Some("第一季"), None),
        item_with_series("ep2", "Episode", 10_000_000 * 3600, 1, None, None),
    ];
    let g = group_episodes_by_series(&eps);
    assert_eq!(g.len(), 2);
    assert_eq!(g[0].name, "第一季");
    assert_eq!(g[1].name, "未分类");
}

#[test]
fn classify_folder_multi_series() {
    // 模拟用户场景：粘贴课程库 URL，parentId 指向 CollectionFolder，
    // 内含两门课程（高等数学与离散数学），递归拉取后按 SeriesName 分组。
    let folder = item("我的课程", "CollectionFolder", 0, 0, None);
    let eps = vec![
        item_with_series(
            "1.1 集合",
            "Episode",
            10_000_000 * 3600,
            1,
            None,
            Some("高等数学"),
        ),
        item_with_series(
            "1.2 逻辑",
            "Episode",
            10_000_000 * 3600,
            2,
            None,
            Some("高等数学"),
        ),
        item_with_series(
            "1. 图论",
            "Episode",
            10_000_000 * 5400,
            1,
            None,
            Some("离散数学"),
        ),
    ];
    let (title, groups, structure) = classify_item(&folder, eps).expect("ok");
    assert_eq!(title, "我的课程");
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].name, "高等数学");
    assert_eq!(groups[0].episodes.len(), 2);
    assert_eq!(groups[1].name, "离散数学");
    assert_eq!(groups[1].episodes[0].duration, 5400);
    assert_eq!(structure, "Jellyfin 文件夹（每个子合集视为一门科目）");
}

#[test]
fn classify_folder_single_series() {
    let folder = item("一个小库", "Folder", 0, 0, None);
    let eps = vec![item_with_series(
        "ep1",
        "Episode",
        10_000_000 * 3600,
        1,
        None,
        Some("唯一课程"),
    )];
    let (_, groups, structure) = classify_item(&folder, eps).expect("ok");
    assert_eq!(groups.len(), 1);
    assert_eq!(structure, "Jellyfin 文件夹（整个文件夹视为一门课程）");
}

#[test]
fn classify_userview_treated_as_folder() {
    let view = item("课程视图", "UserView", 0, 0, None);
    let eps = vec![item_with_series(
        "ep1",
        "Episode",
        10_000_000 * 3600,
        1,
        None,
        Some("课A"),
    )];
    let (_, _, structure) = classify_item(&view, eps).expect("ok");
    assert!(structure.starts_with("Jellyfin 文件夹"));
}

#[test]
fn classify_folder_empty_episodes_errors() {
    let folder = item("空库", "CollectionFolder", 0, 0, None);
    let err = classify_item(&folder, vec![]).unwrap_err();
    assert!(err.message().contains("未找到可规划的视频"));
}

#[test]
fn classify_folder_mixed_subfolders_and_loose_videos() {
    // 复刻用户实际场景：顶层 folder 下既有子 Folder 又有散件 Video。
    // `fetch_folder_videos` 把每个子 Folder 递归视频的 series_name 填为
    // 子 Folder 名，顶层散件填为顶层 folder 名，再交给 group_episodes_by_series。
    let folder = item("02.线性代数", "Folder", 0, 0, None);
    let eps = vec![
        // 子文件夹「01.基础精讲」下的 2 个视频
        item_with_series(
            "1.1 行列式",
            "Video",
            10_000_000 * 3600,
            0,
            None,
            Some("01.基础精讲"),
        ),
        item_with_series(
            "1.2 行列式展开",
            "Video",
            10_000_000 * 3600,
            0,
            None,
            Some("01.基础精讲"),
        ),
        // 子文件夹「02.基础习题」下的 1 个视频
        item_with_series(
            "习题 1",
            "Video",
            10_000_000 * 1800,
            0,
            None,
            Some("02.基础习题"),
        ),
        // 顶层直属散件（series_name 已被 fetch_folder_videos 填成顶层名）
        item_with_series(
            "前言",
            "Video",
            10_000_000 * 600,
            0,
            None,
            Some("02.线性代数"),
        ),
    ];
    let (_, groups, structure) = classify_item(&folder, eps).expect("ok");
    assert_eq!(groups.len(), 3);
    assert_eq!(groups[0].name, "01.基础精讲");
    assert_eq!(groups[0].episodes.len(), 2);
    assert_eq!(groups[1].name, "02.基础习题");
    assert_eq!(groups[2].name, "02.线性代数");
    assert_eq!(groups[2].episodes[0].title, "前言");
    assert_eq!(structure, "Jellyfin 文件夹（每个子合集视为一门科目）");
}
