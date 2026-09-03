//! 计划算法与格式化测试：与 Python 脚本生成的期望向量逐项对比。
//! 向量文件由 bilibili_collection_planner.py 对同一合成数据生成。

use bili_planner::parse::EpisodeItem;
use bili_planner::plan::{
    build_plan, disp_width, fmt_human, fmt_seconds, note_for, render_plan, trunc, Mode,
};
use serde_json::Value;

fn items_from(v: &Value) -> Vec<EpisodeItem> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|it| EpisodeItem {
            title: it[0].as_str().unwrap().to_string(),
            duration: it[1].as_i64().unwrap(),
        })
        .collect()
}

fn plan_json(plan: &[Vec<bili_planner::plan::PlanEntry>]) -> Value {
    serde_json::to_value(plan).unwrap()
}

#[test]
fn matches_python_vectors() {
    let vectors: Value =
        serde_json::from_str(include_str!("vectors.json")).expect("vectors.json parses");
    let items = items_from(&vectors);
    let total = vectors["total"].as_i64().unwrap();
    assert_eq!(total, items.iter().map(|i| i.duration).sum::<i64>());

    for days in [3i64, 5, 20] {
        let key = format!("split_days{days}");
        let out = build_plan(&items, days, Mode::Split).expect("split plan ok");
        assert_eq!(out.total, total, "{key}: total mismatch");
        assert_eq!(
            serde_json::to_value(&out.capacities).unwrap(),
            vectors[key.as_str()]["capacities"],
            "{key}: capacities mismatch"
        );
        assert_eq!(
            plan_json(&out.plan),
            vectors[key.as_str()]["plan"],
            "{key}: plan mismatch"
        );
        let render = render_plan(&out.plan, &out.capacities, out.total, days, Mode::Split);
        assert_eq!(
            render,
            vectors[key.as_str()]["render"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap().to_string())
                .collect::<Vec<_>>(),
            "{key}: render mismatch"
        );
    }

    // 整集模式修正后不再沿用旧 Python 向量中的“容量不足即遗失视频”行为。
    for days in [3i64, 5, 20] {
        let out = build_plan(&items, days, Mode::Whole).expect("whole plan ok");
        assert_eq!(out.total, total, "whole_days{days}: total mismatch");
        assert_eq!(
            out.plan
                .iter()
                .flatten()
                .map(|entry| entry.portion)
                .sum::<i64>(),
            total,
            "whole_days{days}: every video must be scheduled"
        );
        for entry in out.plan.iter().flatten() {
            assert_eq!(entry.remainder, 0, "whole mode must not split videos");
        }
    }
}

#[test]
fn fmt_helpers_match_python() {
    let vectors: Value =
        serde_json::from_str(include_str!("vectors.json")).expect("vectors.json parses");
    let f = &vectors["fmt"];
    for (key, sec) in [
        ("0", 0.0),
        ("59", 59.0),
        ("60", 60.0),
        ("3599", 3599.0),
        ("3600", 3600.0),
        ("3725", 3725.0),
    ] {
        assert_eq!(
            fmt_seconds(sec, true),
            f[key].as_str().unwrap(),
            "fmt_seconds({key})"
        );
    }
    for (key, sec) in [("human_3725", 3725.0), ("human_0", 0.0), ("human_61", 61.0)] {
        assert_eq!(fmt_human(sec), f[key].as_str().unwrap(), "fmt_human({key})");
    }
    assert_eq!(
        trunc("这是一个非常长的标题用来测试截断功能看看效果如何", 20),
        f["trunc"].as_str().unwrap()
    );
    assert_eq!(
        disp_width("abc中文字符123"),
        f["disp_width"].as_u64().unwrap() as usize
    );
}

#[test]
fn notes_match_python() {
    let vectors: Value =
        serde_json::from_str(include_str!("vectors.json")).expect("vectors.json parses");
    let items = items_from(&vectors);
    let out = build_plan(&items, 3, Mode::Split).expect("build_plan ok");
    let want = vectors["notes_days3_split"].as_array().unwrap();
    let mut idx = 0usize;
    for (di, day) in out.plan.iter().enumerate() {
        for e in day {
            let note = note_for(e, di);
            assert_eq!(
                note,
                want[idx]["note"].as_str().unwrap(),
                "note #{idx} (day {})",
                di + 1
            );
            idx += 1;
        }
    }
    assert_eq!(idx, want.len());
}

#[test]
fn plan_errors() {
    let items = vec![EpisodeItem {
        title: "x".into(),
        duration: 100,
    }];
    assert_eq!(
        build_plan(&items, 0, Mode::Split).unwrap_err(),
        "目标天数必须为正整数"
    );
    let empty = vec![EpisodeItem {
        title: "x".into(),
        duration: 0,
    }];
    assert_eq!(
        build_plan(&empty, 7, Mode::Split).unwrap_err(),
        "总时长为 0，无法生成计划"
    );
}

#[test]
fn whole_mode_keeps_an_episode_that_exceeds_daily_target() {
    let items = vec![
        EpisodeItem {
            title: "长课".to_string(),
            duration: 1_000,
        },
        EpisodeItem {
            title: "短课".to_string(),
            duration: 200,
        },
    ];

    let out = build_plan(&items, 3, Mode::Whole).unwrap();
    let scheduled: i64 = out.plan.iter().flatten().map(|entry| entry.portion).sum();

    assert_eq!(scheduled, out.total);
    assert_eq!(out.plan[0][0].title, "长课");
    assert_eq!(out.plan[0][0].portion, 1_000);
    assert_eq!(out.plan[1][0].title, "短课");
}
