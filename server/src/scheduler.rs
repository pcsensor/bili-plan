use crate::card::build_today_study_card;
use crate::feishu::FeishuClient;
use crate::store::Store;
use chrono::{Local, Timelike};
use std::time::Duration;
use tokio::time::sleep;
use tracing::{error, info};

pub fn start_scheduler(store: Store, feishu: FeishuClient) {
    tokio::spawn(async move {
        info!("🕒 定时推送调度器已启动 (每日 08:30 早报 / 21:30 晚间督促)");
        loop {
            let now = Local::now();
            let hour = now.hour();
            let minute = now.minute();
            let date_str = now.format("%Y-%m-%d").to_string();

            // 1. 晨间早报 08:30 - 08:31
            if hour == 8 && minute == 30 {
                let bound_users = store.get_all_bound_users().await;
                for (user, plans) in bound_users {
                    if let Some(open_id) = &user.feishu_open_id {
                        if store.record_pushed_date(open_id, "morning", &date_str).await {
                            info!("向用户 {} 触发晨间计划推送", open_id);
                            let card = build_today_study_card(&plans, &date_str);
                            if let Err(e) = feishu.send_card_message(open_id, card).await {
                                error!("推送晨间卡片失败: {}", e);
                            }
                        }
                    }
                }
            }

            // 2. 晚间督促 21:30 - 21:31
            if hour == 21 && minute == 30 {
                let bound_users = store.get_all_bound_users().await;
                for (user, plans) in bound_users {
                    if let Some(open_id) = &user.feishu_open_id {
                        if store.record_pushed_date(open_id, "evening", &date_str).await {
                            info!("向用户 {} 触发晚间督促推送", open_id);
                            let card = build_today_study_card(&plans, &date_str);
                            if let Err(e) = feishu.send_card_message(open_id, card).await {
                                error!("推送晚间卡片失败: {}", e);
                            }
                        }
                    }
                }
            }

            sleep(Duration::from_secs(30)).await;
        }
    });
}
