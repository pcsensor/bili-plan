use super::*;

fn page_items(ids: &[&str], total: Option<i64>) -> ItemListData {
    ItemListData {
        total,
        list: ids
            .iter()
            .map(|id| FnItem {
                guid: Some((*id).into()),
                title: Some("同名视频".into()),
                kind: Some("Video".into()),
                duration: Some(60),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}

#[test]
fn pagination_reads_short_pages_until_total_and_preserves_same_titles() {
    let mut calls = Vec::new();
    let result = collect_pages("root", |page| {
        calls.push(page);
        Ok(match page {
            1 => page_items(&["a", "b"], Some(4)),
            2 => page_items(&["b", "c"], Some(4)),
            3 => page_items(&["d"], Some(4)),
            _ => panic!("不应再请求"),
        })
    })
    .unwrap();
    assert_eq!(result.list.len(), 4);
    assert_eq!(calls, [1, 2, 3]);
    assert_eq!(result.list[3].guid.as_deref(), Some("d"));
}

#[test]
fn pagination_without_total_reads_until_empty() {
    let result = collect_pages("root", |page| {
        Ok(match page {
            1 => page_items(&["a"], None),
            2 => page_items(&["b"], None),
            3 => page_items(&[], None),
            _ => panic!("不应再请求"),
        })
    })
    .unwrap();
    assert_eq!(result.list.len(), 2);
}

#[test]
fn pagination_rejects_repeated_pages_and_premature_end() {
    for total in [None, Some(3)] {
        let err = collect_pages("root", |_| Ok(page_items(&["a"], total))).unwrap_err();
        assert!(err.message().contains("分页未前进"));
    }
    let err = collect_pages("root", |page| {
        Ok(page_items(if page == 1 { &["a"] } else { &[] }, Some(3)))
    })
    .unwrap_err();
    assert!(err.message().contains("应有 3 项，仅获取 1 项"));
}

#[test]
fn pagination_propagates_later_failure_and_detects_changed_total() {
    let err = collect_pages("root", |page| {
        if page == 1 {
            Ok(page_items(&["a"], Some(2)))
        } else {
            Err(Error::network("断线"))
        }
    })
    .unwrap_err();
    assert_eq!(err.message(), "断线");
    let err = collect_pages("root", |page| {
        Ok(match page {
            1 => page_items(&["a"], Some(2)),
            _ => page_items(&["b"], Some(3)),
        })
    })
    .unwrap_err();
    assert!(err.message().contains("总数在读取期间发生变化"));
}

#[test]
fn missing_list_is_an_error_and_total_count_is_supported() {
    assert!(serde_json::from_str::<ItemListData>(r#"{"total":32}"#).is_err());
    let data: ItemListData = serde_json::from_str(r#"{"total_count":"32","list":[]}"#).unwrap();
    assert_eq!(data.total, Some(32));
}

#[test]
fn cloud_folder_resolves_22_zero_durations_and_keeps_all_32_videos() {
    // 真实故障的脱敏契约：32 个 Video，22 个 duration=0，均有 media_guid。
    let mut items: Vec<FnItem> = (1..=32)
        .map(|n| FnItem {
            guid: Some(format!("item-{n}")),
            media_guid: Some(format!("file-{n}")),
            kind: Some("Video".into()),
            title: Some(format!("课程 {n}")),
            duration: Some(if (20..=29).contains(&n) { 600 } else { 0 }),
            ..Default::default()
        })
        .collect();
    let probes = AtomicUsize::new(0);
    complete_durations(
        &mut items,
        |item| {
            assert_eq!(duration_of(item), 0);
            probes.fetch_add(1, Ordering::Relaxed);
            stream_duration(serde_json::json!({"video_stream":{"duration":1692}}))
        },
        &mut |_| {},
    )
    .unwrap();
    let (_, groups, _) =
        classify_children(items, Some("课程"), |_| panic!("视频不应下钻")).unwrap();
    assert_eq!(probes.load(Ordering::Relaxed), 22);
    assert_eq!(groups[0].episodes.len(), 32);
    assert_eq!(groups[0].episodes[0].duration, 1692);
    assert_eq!(groups[0].episodes[19].duration, 600);
}

#[test]
fn pending_stream_duration_is_retried_but_failure_is_bounded() {
    let mut values = [0, 0, 2338].into_iter();
    let mut waits = 0;
    assert_eq!(
        wait_for_duration(|| Ok(values.next().unwrap()), || waits += 1).unwrap(),
        2338
    );
    assert_eq!(waits, 2);
    let mut calls = 0;
    let err = wait_for_duration(
        || {
            calls += 1;
            Ok(0)
        },
        || {},
    )
    .unwrap_err();
    assert_eq!(calls, DURATION_POLL_ATTEMPTS);
    assert!(err.message().contains("仍未准备就绪"));
    let err =
        wait_for_duration(|| Err(Error::api("无权限")), || panic!("不应重试业务错误")).unwrap_err();
    assert_eq!(err.message(), "无权限");
}

#[test]
fn concurrent_duration_completion_preserves_order_and_reports_progress() {
    let mut items: Vec<_> = (0..9)
        .map(|n| FnItem {
            title: Some(n.to_string()),
            kind: Some("Video".into()),
            duration: Some(0),
            ..Default::default()
        })
        .collect();
    let active = AtomicUsize::new(0);
    let peak = AtomicUsize::new(0);
    let mut messages = Vec::new();
    complete_durations(
        &mut items,
        |item| {
            let n: u64 = item.title.as_ref().unwrap().parse().unwrap();
            let running = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(running, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(10 * (3 - n % 3)));
            active.fetch_sub(1, Ordering::SeqCst);
            Ok(100 + n as i64)
        },
        &mut |message| messages.push(message),
    )
    .unwrap();
    assert!(peak.load(Ordering::SeqCst) > 1);
    assert!(peak.load(Ordering::SeqCst) <= DURATION_WORKERS);
    assert_eq!(
        items.iter().map(duration_of).collect::<Vec<_>>(),
        (100..109).collect::<Vec<_>>()
    );
    for ready in 0..=9 {
        assert!(messages
            .iter()
            .any(|s| s == &format!("当前目录：{ready} / 9 个视频时长已就绪")));
    }
}

#[test]
fn concurrent_duration_failure_does_not_commit_partial_results() {
    let mut items = vec![
        FnItem {
            title: Some("正常".into()),
            kind: Some("Video".into()),
            duration: Some(0),
            ..Default::default()
        },
        FnItem {
            title: Some("故障".into()),
            kind: Some("Video".into()),
            duration: Some(0),
            ..Default::default()
        },
    ];
    let err = complete_durations(
        &mut items,
        |item| {
            if item.title.as_deref() == Some("故障") {
                Err(Error::network("超时"))
            } else {
                Ok(100)
            }
        },
        &mut |_| {},
    )
    .unwrap_err();
    assert!(err.message().contains("故障"));
    assert!(err.message().contains("超时"));
    assert!(items.iter().all(|item| duration_of(item) == 0));
}

#[test]
fn duration_completion_skips_containers_and_runtime_and_fails_explicitly() {
    let mut items = vec![
        FnItem {
            kind: Some("Directory".into()),
            guid: Some("folder".into()),
            ..Default::default()
        },
        FnItem {
            kind: Some("Video".into()),
            runtime: Some(10),
            ..Default::default()
        },
    ];
    complete_durations(&mut items, |_| panic!("不需要探测"), &mut |_| {}).unwrap();
    let mut items = vec![FnItem {
        kind: Some("Video".into()),
        title: Some("缺失课时".into()),
        ..Default::default()
    }];
    let err =
        complete_durations(&mut items, |_| Err(Error::api("探测失败")), &mut |_| {}).unwrap_err();
    assert!(err.message().contains("缺失课时"));
    assert!(err.message().contains("当前结果不完整"));
    assert_eq!(
        stream_duration(serde_json::json!({"video_stream":{"duration":0}})).unwrap(),
        0
    );
    assert!(stream_duration(serde_json::json!({})).is_err());
    assert_eq!(
        stream_duration(serde_json::json!({"video_stream":{"duration":"1692"}})).unwrap(),
        1692
    );
}

#[test]
fn md5_matches_rfc1321_vectors() {
    assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
    assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
    assert_eq!(
        md5_hex(b"The quick brown fox jumps over the lazy dog"),
        "9e107d9d372bb6826bd81d3542a419d6"
    );
}

#[test]
fn authx_uses_exactly_the_signed_bytes() {
    // 签名与发送共用同一份字节：同一字节 → 同一签名。
    let a = gen_authx(
        "/v/api/v1/item/list",
        b"{\"a\":1}",
        "123456",
        1_700_000_000_000,
    );
    let b = gen_authx(
        "/v/api/v1/item/list",
        b"{\"a\":1}",
        "123456",
        1_700_000_000_000,
    );
    assert_eq!(a, b);
    // 键序不同 → 字节不同 → 签名不同（因此必须发送被签名的那份字节）。
    let c = gen_authx(
        "/v/api/v1/item/list",
        b"{\"a\":1,\"b\":2}",
        "123456",
        1_700_000_000_000,
    );
    let d = gen_authx(
        "/v/api/v1/item/list",
        b"{\"b\":2,\"a\":1}",
        "123456",
        1_700_000_000_000,
    );
    assert_ne!(c, d);
    assert!(a.starts_with("nonce=123456&timestamp=1700000000000&sign="));
}

#[test]
fn extract_guid_handles_urls_and_bare_ids() {
    assert_eq!(
        extract_guid("http://10.0.0.6:5666/v/video?guid=fv_30006e2fdaa44c7aac2c3cb25c10121d"),
        Some("fv_30006e2fdaa44c7aac2c3cb25c10121d".to_string())
    );
    assert_eq!(
        extract_guid("http://host:5666/v/index/#!/details?id=abcdef0123456789abcdef0123456789"),
        Some("abcdef0123456789abcdef0123456789".to_string())
    );
    assert_eq!(
        extract_guid("http://host:5666/v/video/fv_30006e2fdaa44c7aac2c3cb25c10121d"),
        Some("fv_30006e2fdaa44c7aac2c3cb25c10121d".to_string())
    );
    assert_eq!(
        extract_guid("fv_30006e2fdaa44c7aac2c3cb25c10121d"),
        Some("fv_30006e2fdaa44c7aac2c3cb25c10121d".to_string())
    );
    // 路由词不能被当成 guid。
    assert_eq!(extract_guid("http://host:5666/v/index/#!/details"), None);
    assert_eq!(extract_guid("http://host:5666/v/video"), None);
    assert_eq!(extract_guid(""), None);
    assert_eq!(extract_guid("   "), None);
}

#[test]
fn duration_prefers_seconds_and_falls_back_to_runtime() {
    let mut item = FnItem {
        duration: Some(1500),
        runtime: Some(99),
        ..Default::default()
    };
    assert_eq!(duration_of(&item), 1500);
    // duration 为 0 时用 runtime（分钟）换算。
    item.duration = Some(0);
    assert_eq!(duration_of(&item), 99 * 60);
    // 两者都缺 → 0。
    item.runtime = None;
    assert_eq!(duration_of(&item), 0);
    // 数字以字符串返回时也能解析。
    let lenient: FnItem =
        serde_json::from_str(r#"{"duration":"600","episode_number":"3"}"#).unwrap();
    assert_eq!(duration_of(&lenient), 600);
    assert_eq!(lenient.episode_number, Some(3));
}

#[test]
fn container_detection_follows_type_then_duration() {
    let container = FnItem {
        kind: Some("Season".to_string()),
        duration: Some(9999),
        ..Default::default()
    };
    assert!(container.is_container());

    let leaf = FnItem {
        kind: Some("Episode".to_string()),
        duration: Some(0),
        ..Default::default()
    };
    assert!(!leaf.is_container());

    // 未知类型：有时长当叶子，无时长当容器继续下钻。
    let unknown_with_duration = FnItem {
        kind: Some("NewKind".to_string()),
        duration: Some(120),
        ..Default::default()
    };
    assert!(!unknown_with_duration.is_container());
    let unknown_without_duration = FnItem {
        kind: Some("NewKind".to_string()),
        ..Default::default()
    };
    assert!(unknown_without_duration.is_container());
}

#[test]
fn order_items_sorts_only_when_numbering_is_complete() {
    let numbered = vec![
        FnItem {
            episode_number: Some(2),
            season_number: Some(1),
            title: Some("二".into()),
            ..Default::default()
        },
        FnItem {
            episode_number: Some(1),
            season_number: Some(1),
            title: Some("一".into()),
            ..Default::default()
        },
    ];
    let ordered = order_items(numbered);
    assert_eq!(ordered[0].title.as_deref(), Some("一"));

    // 缺集号时保持原顺序，避免把服务端有意排好的顺序打乱。
    let partial = vec![
        FnItem {
            episode_number: Some(2),
            title: Some("keep-a".into()),
            ..Default::default()
        },
        FnItem {
            episode_number: None,
            title: Some("keep-b".into()),
            ..Default::default()
        },
    ];
    let ordered = order_items(partial);
    assert_eq!(ordered[0].title.as_deref(), Some("keep-a"));
}

#[test]
fn classify_groups_containers_and_loose_leaves() {
    let children = vec![
        FnItem {
            guid: Some("g-season-1".into()),
            title: Some("第 1 季".into()),
            kind: Some("Season".into()),
            parent_title: Some("示例剧集".into()),
            ..Default::default()
        },
        FnItem {
            guid: Some("g-season-2".into()),
            title: Some("第 2 季".into()),
            kind: Some("Season".into()),
            ..Default::default()
        },
        FnItem {
            title: Some("花絮".into()),
            kind: Some("Video".into()),
            duration: Some(300),
            ..Default::default()
        },
    ];

    let (title, groups, structure) = classify_children(children, Some("测试库"), |guid| {
        Ok(vec![EpisodeItem {
            title: format!("{guid}-ep"),
            duration: 600,
        }])
    })
    .unwrap();

    // 根标题由子项的 parent_title 反推。
    assert_eq!(title, "示例剧集");
    assert_eq!(groups.len(), 3);
    // 散件组排在容器组之前。
    assert_eq!(groups[0].name, "示例剧集（散件）");
    assert_eq!(groups[0].episodes[0].duration, 300);
    assert_eq!(groups[1].name, "第 1 季");
    assert_eq!(groups[1].episodes[0].title, "g-season-1-ep");
    assert_eq!(groups[2].name, "第 2 季");
    assert!(structure.contains("多层级合集"));
}

#[test]
fn classify_single_flat_season_becomes_one_course() {
    let children = vec![
        FnItem {
            title: Some("第一课".into()),
            kind: Some("Episode".into()),
            episode_number: Some(1),
            duration: Some(600),
            parent_title: Some("示例课程".into()),
            ..Default::default()
        },
        FnItem {
            title: Some("第二课".into()),
            kind: Some("Episode".into()),
            episode_number: Some(2),
            duration: Some(900),
            ..Default::default()
        },
    ];
    let (title, groups, structure) =
        classify_children(children, Some("测试库"), |_| unreachable!()).unwrap();
    assert_eq!(title, "示例课程");
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].name, "示例课程");
    assert_eq!(groups[0].episodes[0].title, "第1集 第一课");
    assert_eq!(groups[0].episodes[1].duration, 900);
    assert!(structure.contains("单合集"));
}

#[test]
fn classify_rejects_content_without_duration() {
    let children = vec![FnItem {
        title: Some("无时长".into()),
        kind: Some("Episode".into()),
        duration: Some(0),
        ..Default::default()
    }];
    assert!(classify_children(children, None, |_| Ok(Vec::new())).is_err());
}

#[test]
fn root_title_degrades_gracefully() {
    let empty: Vec<FnItem> = Vec::new();
    assert_eq!(derive_root_title(&empty, Some("媒体库")), "媒体库");
    assert_eq!(derive_root_title(&empty, None), "飞牛影视合集");
}
