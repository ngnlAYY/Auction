#![allow(clippy::expect_used, clippy::unwrap_used)]

//! 被超提醒正文：默认模板与自定义模板都在创建时冻结，摘要绑定冻结正文。
//!
//! 只验证纯函数生成的冻结正文与摘要；不连接数据库或外部服务。

use auction_application::admin::AdminRuntimeMailTemplatesConfig;
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

fn templates() -> AdminRuntimeMailTemplatesConfig {
    AdminRuntimeMailTemplatesConfig::default()
}

fn mail_body(custom: &AdminRuntimeMailTemplatesConfig) -> String {
    lot_watch_notification(
        lot(),
        Uuid::max(),
        3,
        5_252_000,
        notice(),
        custom,
        "main".to_owned(),
        "Restia".to_owned(),
    )
    .expect("构造 MoogleMail 被超提醒")
    .notification_body
    .expect("通知正文")
}

#[test]
fn moogle_mail_outbid_notice_identifies_item_amount_deadline_and_link() {
    let body = mail_body(&templates());
    assert!(body.contains("Item: Phase Robe of Niflheim"));
    assert!(body.contains("Lot: Clo16"));
    assert!(body.contains("Accepted bid: 5,252,000 Credits"));
    assert!(body.contains("Bidding closes: 2026-11-05T12:30:00Z"));
    assert!(body.contains("View the lot: https://auction.example.com/lots/Clo16"));
    assert!(!body.contains("proxy"));
    assert!(!body.to_ascii_lowercase().contains("maximum"));
    // 默认模板必须逐字保留既有冻结正文，模板化不能改变现有内容。
    assert_eq!(
        body,
        "Your leading bid has been outbid.\n\n\
         Item: Phase Robe of Niflheim\n\
         Lot: Clo16\n\
         Accepted bid: 5,252,000 Credits\n\
         Bidding closes: 2026-11-05T12:30:00Z\n\
         View the lot: https://auction.example.com/lots/Clo16\n\n\
         Check the auction website for the latest price\n\
         and place another bid if you wish to continue."
    );
}

#[test]
fn custom_template_changes_appear_in_newly_frozen_mail_and_pm_bodies() {
    let custom = AdminRuntimeMailTemplatesConfig {
        lot_watch: "Outbid notice for {item_name} ({lot_code}) at {amount} Credits\n\
                    {deadline_line}{lot_url_line}\nBid again to stay in the lead."
            .to_owned(),
        lot_watch_pm: "PM outbid: {item_name} / {lot_code} / {amount}\n\
                       {deadline_line}{lot_url_line}Notification: {notification_id}."
            .to_owned(),
        ..AdminRuntimeMailTemplatesConfig::default()
    };
    let body = mail_body(&custom);
    assert!(
        body.starts_with("Outbid notice for Phase Robe of Niflheim (Clo16) at 5,252,000 Credits\n")
    );
    assert!(body.contains("Bidding closes: 2026-11-05T12:30:00Z\n"));
    assert!(body.contains("View the lot: https://auction.example.com/lots/Clo16\n"));
    assert!(body.contains("Bid again to stay in the lead."));
    assert!(!body.contains("Your leading bid has been outbid."));

    let notification = lot_watch_pm_notification(
        lot(),
        Uuid::max(),
        3,
        120_000,
        notice(),
        &custom,
        Some("9001".to_owned()),
    )
    .expect("构造 PM 被超提醒");
    let pm_body = notification.payload["body"].as_str().unwrap();
    assert!(pm_body.starts_with("PM outbid: Phase Robe of Niflheim / Clo16 / 120,000\n"));
    assert!(
        pm_body.ends_with(&format!("Notification: {}.", notification.notification_id)),
        "{pm_body}"
    );
    assert!(!pm_body.contains("Your leading bid has been outbid."));
}

#[test]
fn frozen_hash_binds_the_rendered_body() {
    // 同一领先周期、同一收件人，正文不同则摘要必须不同：摘要覆盖冻结正文而非可变模板。
    let custom = AdminRuntimeMailTemplatesConfig {
        lot_watch: "Replacement body for {lot_code} {item_name} {amount}\n\
                    {deadline_line}{lot_url_line}Bid again if you wish to continue."
            .to_owned(),
        ..AdminRuntimeMailTemplatesConfig::default()
    };
    let default_command = lot_watch_notification(
        lot(),
        Uuid::max(),
        3,
        5_252_000,
        notice(),
        &templates(),
        "main".to_owned(),
        "Restia".to_owned(),
    )
    .expect("默认模板");
    let custom_command = lot_watch_notification(
        lot(),
        Uuid::max(),
        3,
        5_252_000,
        notice(),
        &custom,
        "main".to_owned(),
        "Restia".to_owned(),
    )
    .expect("自定义模板");
    assert_eq!(default_command.command_id, custom_command.command_id);
    assert_ne!(default_command.request_hash, custom_command.request_hash);
}

#[test]
fn pm_outbid_notice_uses_same_public_fields() {
    let notification = lot_watch_pm_notification(
        lot(),
        Uuid::max(),
        3,
        120_000,
        notice(),
        &templates(),
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
    assert!(body.contains(&format!("Notification: {}.", notification.notification_id)));
    assert_eq!(
        body,
        format!(
            "Your leading bid has been outbid.\n\n\
             Item: Phase Robe of Niflheim\n\
             Lot: Clo16\n\
             Accepted bid: 120,000 Credits\n\
             Bidding closes: 2026-11-05T12:30:00Z\n\
             View the lot: https://auction.example.com/lots/Clo16\n\n\
             Notification: {}.",
            notification.notification_id
        )
    );
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
        &templates(),
        "main".to_owned(),
        "Restia".to_owned(),
    )
    .expect("构造 MoogleMail 被超提醒");
    let body = command.notification_body.expect("通知正文");
    assert!(!body.contains("View the lot:"));
    assert!(!body.contains("Bidding closes:"));
    assert!(body.contains("Lot: Sta02"));
    assert!(!body.contains("\n\n\n"), "缺失的可选行不能留下空行：{body}");
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
            &templates(),
            "main".to_owned(),
            "Restia".to_owned(),
        )
        .is_err()
    );
}
