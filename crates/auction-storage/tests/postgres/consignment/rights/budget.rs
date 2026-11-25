//! 预算（J11）：每浏览器绑定每 10 分钟 10 次、每可信 IP 每 10 分钟 30 次，超限 429，与目标真实与否无关。

use auction_application::consignment::{RightsStore, RightsStoreError};
use uuid::Uuid;

use super::fixture::{IDENTITY, Rig, locate_command};

#[tokio::test]
async fn postgres_consignment_rights_browser_budget_is_ten_per_window_and_blind_to_the_target() {
    let rig = Rig::new().await;
    rig.seed_right(1, "claim_payment", &["claim_payment"]).await;
    for index in 0..10 {
        let ip = format!("198.51.100.{}", 10 + index);
        rig.store
            .locate(locate_command(Uuid::new_v4(), 5, &ip, "NoSuchPlayer"))
            .await
            .expect("预算内受理");
    }
    let rows = rig.count("SELECT count(*) FROM public.consignment_commands").await;
    // 反例：第 11 次超限；无论目标真实与否都是同一个错误，且不写任何行。
    for identity in ["NoSuchPlayer", IDENTITY] {
        let result =
            rig.store.locate(locate_command(Uuid::new_v4(), 5, "198.51.100.99", identity)).await;
        assert_eq!(result.err(), Some(RightsStoreError::BudgetExceeded), "{identity}");
    }
    assert_eq!(rig.count("SELECT count(*) FROM public.consignment_commands").await, rows);
    // 正例：窗口过后恢复；别的浏览器不受影响。
    rig.store
        .locate(locate_command(Uuid::new_v4(), 6, "198.51.100.98", IDENTITY))
        .await
        .expect("别的浏览器不受影响");
    rig.advance_minutes(11);
    rig.store
        .locate(locate_command(Uuid::new_v4(), 5, "198.51.100.97", IDENTITY))
        .await
        .expect("窗口过后恢复");
}

#[tokio::test]
async fn postgres_consignment_rights_ip_budget_is_thirty_per_window_across_browsers() {
    let rig = Rig::new().await;
    for browser_byte in 0..30_u8 {
        rig.store
            .locate(locate_command(
                Uuid::new_v4(),
                100 + browser_byte,
                "203.0.113.5",
                "NoSuchPlayer",
            ))
            .await
            .expect("预算内受理");
    }
    let result =
        rig.store.locate(locate_command(Uuid::new_v4(), 200, "203.0.113.5", "NoSuchPlayer")).await;
    assert_eq!(result.err(), Some(RightsStoreError::BudgetExceeded));
    // 正例：另一个 IP 的新浏览器不受影响。
    rig.store
        .locate(locate_command(Uuid::new_v4(), 201, "203.0.113.6", "NoSuchPlayer"))
        .await
        .expect("别的 IP 不受影响");
}
