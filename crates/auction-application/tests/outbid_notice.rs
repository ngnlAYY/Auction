#![allow(clippy::expect_used, clippy::unwrap_used)]

//! W14.2 被超提醒正文：登录网站前也能识别 lot 代码、装备、金额、有效截止与链接。
//!
//! 只验证纯函数生成的冻结正文与摘要；不连接数据库或外部服务。

use auction_application::public::{
    OutbidNotice, lot_watch_notification, lot_watch_pm_notification,
};
use chrono::{TimeZone, Utc};
use uuid::Uuid;

fn lot() -> Uuid {
    Uuid::from_u128(0x1111_1111_1111_1111_1111_1111_1111_1111)
}

fn notice() -> OutbidNotice<'static> {
    OutbidNotice {
        lot_code: "Clo16",
        item_name: "Phase Robe of Niflheim",
        deadline: Some(Utc.with_ymd_and_hms(2026, 11, 5, 12, 30, 0).unwrap()),
        site_url: Some("https://auction.example.com/"),
    }
}

#[test]
fn moogle_mail_outbid_notice_identifies_item_amount_deadline_and_link() {
    let command = lot_watch_notification(
        lot(),
        Uuid::max(),
        3,
        5_252_000,
        notice(),
        "main".to_owned(),
        "Restia".to_owned(),
    )
    .expect("构造 MoogleMail 被超提醒");
    let body = command.notification_body.expect("通知正文");
    assert!(body.contains("Item: Phase Robe of Niflheim"));
    assert!(body.contains("Lot: Clo16"));
    assert!(body.contains("Accepted bid: 5,252,000 Credits"));
    assert!(body.contains("Bidding closes: 2026-11-05T12:30:00Z"));
    assert!(body.contains("View the lot: https://auction.example.com/lots/Clo16"));
    assert!(!body.contains("proxy"));
    assert!(!body.to_ascii_lowercase().contains("maximum"));
}

#[test]
fn pm_outbid_notice_uses_same_public_fields() {
    let notification = lot_watch_pm_notification(
        lot(),
        Uuid::max(),
        3,
        120_000,
        notice(),
        Some("9001".to_owned()),
    )
    .expect("构造 PM 被超提醒");
    assert_eq!(notification.recipient.as_deref(), Some("9001"));
    let body = notification.payload["body"].as_str().unwrap();
    let title = notification.payload["title"].as_str().unwrap();
    assert_eq!(title, "Outbid Clo16 - Phase Robe of Niflheim");
    assert!(body.contains("Lot: Clo16"));
    assert!(body.contains("Accepted bid: 120,000 Credits"));
    assert!(body.contains("Bidding closes: 2026-11-05T12:30:00Z"));
    assert!(body.contains("View the lot: https://auction.example.com/lots/Clo16"));
}

#[test]
fn notice_omits_link_and_deadline_when_not_configured() {
    let command = lot_watch_notification(
        lot(),
        Uuid::max(),
        1,
        1_000,
        OutbidNotice {
            lot_code: "Sta02",
            item_name: "Staff of Heimdall",
            deadline: None,
            site_url: None,
        },
        "main".to_owned(),
        "Restia".to_owned(),
    )
    .expect("构造 MoogleMail 被超提醒");
    let body = command.notification_body.expect("通知正文");
    assert!(!body.contains("View the lot:"));
    assert!(!body.contains("Bidding closes:"));
    assert!(body.contains("Lot: Sta02"));
}

#[test]
fn empty_lot_identity_is_rejected() {
    assert!(
        lot_watch_notification(
            lot(),
            Uuid::max(),
            1,
            1_000,
            OutbidNotice { lot_code: "", item_name: "x", deadline: None, site_url: None },
            "main".to_owned(),
            "Restia".to_owned(),
        )
        .is_err()
    );
}
