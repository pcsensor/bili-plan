//! Jellyfin 端到端验证：命令行方式跑真实服务器，定位失败点。
//! 用法：cargo run --example live_check_jellyfin -- "<链接/item ID>" [days] [all|科目号] [split|whole]
//! 服务器地址可用环境变量 JF_SERVER 覆盖；令牌必须通过 JF_TOKEN 提供（不再内置）。

use bili_planner::app::{fetch_and_parse, generate_plan, FetchSource, Selection};
use bili_planner::export;
use bili_planner::plan::Mode;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let input = args.get(1).cloned().unwrap_or_else(|| {
        "https://jellyfin.pcsensor.cloud/web/#/list?\
             parentId=699396e43b7061237d5b0c43086c7e42\
             &serverId=eebf26e5024f4fb59c9ff3eabfbadaef"
            .to_string()
    });
    let days: i64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(30);
    let select = args.get(3).map(|s| s.as_str()).unwrap_or("all");
    let mode = if args.get(4).map(|s| s.as_str()) == Some("whole") {
        Mode::Whole
    } else {
        Mode::Split
    };
    let server =
        std::env::var("JF_SERVER").unwrap_or_else(|_| "https://jellyfin.pcsensor.cloud".into());
    let token = std::env::var("JF_TOKEN").unwrap_or_else(|_| {
        eprintln!("!! 缺少环境变量 JF_TOKEN（出于安全考虑，令牌不再内置，请设置后重试）。");
        std::process::exit(1);
    });

    eprintln!("== 输入：{input}");
    eprintln!("== 服务器：{server}");
    eprintln!("== Token：{}…", &token[..6]);

    let source = FetchSource::Jellyfin {
        server_url: server,
        token,
    };

    eprintln!("-- fetch_and_parse 开始");
    let mut rd = match fetch_and_parse(&input, &source) {
        Ok(rd) => rd,
        Err(e) => {
            eprintln!("!! fetch_and_parse 失败：{e}");
            std::process::exit(1);
        }
    };
    eprintln!("-- 结构识别：{}", rd.structure);
    eprintln!("-- 科目数：{}", rd.groups.len());
    for (i, g) in rd.groups.iter().enumerate() {
        let total: i64 = g.episodes.iter().map(|e| e.duration).sum();
        eprintln!(
            "   {}. {}（{} 个视频，共 {total} 秒）",
            i + 1,
            g.name,
            g.episodes.len()
        );
    }

    if rd.groups.len() > 1 {
        if select == "all" {
            rd.selection = Selection::All;
        } else if let Ok(i) = select.parse::<usize>() {
            rd.selection = Selection::Single(i.saturating_sub(1));
        }
    }

    eprintln!("-- generate_plan 开始（days={days}, mode={:?}）", mode);
    if let Err(e) = generate_plan(&mut rd, days, mode) {
        eprintln!("!! 生成计划失败：{e}");
        std::process::exit(2);
    }
    let p = rd.plan.as_ref().expect("plan");
    let text = export::full_text(
        &rd.season_title,
        &rd.structure,
        &p.scope_desc,
        p.total,
        p.days,
        p.avg,
        &rd.groups,
        &p.plan,
        &p.capacities,
        p.total,
        mode,
    );
    print!("{text}");
}
