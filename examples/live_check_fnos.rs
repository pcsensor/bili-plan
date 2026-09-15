//! 只读联调：凭据从 FNOS_URL / FNOS_USER / FNOS_PASS 环境变量读取。
//! cargo run --example live_check_fnos -- <guid|链接> [预期视频数]
use bili_planner::fnos::{fetch_groups_with_progress, FnOsClient};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let input = args.next().ok_or("缺少 guid 或链接")?;
    let expected = args.next().map(|n| n.parse::<usize>()).transpose()?;
    let client = FnOsClient::new(
        std::env::var("FNOS_URL")?,
        std::env::var("FNOS_USER")?,
        std::env::var("FNOS_PASS")?,
    );
    let started = std::time::Instant::now();
    let (title, groups, structure) = fetch_groups_with_progress(&client, &input, &mut |message| {
        eprintln!("[{} 秒] {message}", started.elapsed().as_secs());
    })?;
    let count: usize = groups.iter().map(|g| g.episodes.len()).sum();
    let seconds: i64 = groups
        .iter()
        .flat_map(|g| &g.episodes)
        .map(|e| e.duration)
        .sum();
    println!("{title}：{count} 个视频，总时长 {seconds} 秒");
    println!("{structure}");
    if expected.is_some_and(|n| n != count) {
        return Err(format!("预期 {} 个视频，实际 {count} 个", expected.unwrap()).into());
    }
    if groups
        .iter()
        .flat_map(|g| &g.episodes)
        .any(|e| e.duration <= 0)
    {
        return Err("仍有视频缺少有效时长".into());
    }
    Ok(())
}
