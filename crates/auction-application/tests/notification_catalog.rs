#![allow(clippy::expect_used, clippy::unwrap_used)]

//! W14.1 通知目录一致性：文档目录、模板注册与指标闭集必须一一对应。
//!
//! 该测试只读取仓库内文档与编译期闭集，不连接数据库或外部服务。新增通知时如果漏写目录行、
//! 在目录里留下未实现的键，或模板注册与指标种类分叉，都会在这里失败。

use std::collections::BTreeSet;

use auction_application::{
    AdminRuntimeMailTemplatesConfig, MailNotificationTemplate, NotificationKind,
};

const MAIL_FLOWS: &str = include_str!("../../../docs/guide/mail-flows.md");
const START: &str = "<!-- notification-catalog:start -->";
const END: &str = "<!-- notification-catalog:end -->";

/// 目录中不需要可编辑模板的渠道：论坛回执正文、被超提醒与投稿接收回执由代码冻结，PM 走论坛私信。
const NON_TEMPLATE_KINDS: [NotificationKind; 4] = [
    NotificationKind::ForumAuctionNotice,
    NotificationKind::LotWatchMail,
    NotificationKind::LotWatchPm,
    NotificationKind::AuctionSubmissionReceipt,
];

/// 提取目录表第一列的模板键 / 身份。
fn catalog_keys() -> BTreeSet<String> {
    let section = MAIL_FLOWS
        .split_once(START)
        .and_then(|(_, rest)| rest.split_once(END))
        .map(|(section, _)| section)
        .expect("mail-flows.md 必须包含 notification-catalog 标记区间");
    let mut keys = BTreeSet::new();
    for line in section.lines() {
        let Some(row) = line.strip_prefix('|') else { continue };
        // 跳过表头与分隔行：表头首格是名称列，分隔行没有反引号。
        let Some((first, _)) = row.split_once('|') else { continue };
        let Some(start) = first.find('`') else { continue };
        let rest = &first[start + 1..];
        let Some(end) = rest.find('`') else {
            panic!("目录行缺少闭合反引号：{line}");
        };
        let key = &rest[..end];
        assert!(!key.is_empty(), "目录模板键不能为空：{line}");
        assert!(keys.insert(key.to_owned()), "目录出现重复通知键：{key}");
    }
    keys
}

#[test]
fn catalog_covers_every_notification_kind() {
    let keys = catalog_keys();
    let expected = NotificationKind::ALL
        .into_iter()
        .map(|kind| kind.label().to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(keys, expected, "通知目录与 NotificationKind 闭集不一致");
}

#[test]
fn catalog_matches_template_registry() {
    let keys = catalog_keys();
    let templates = AdminRuntimeMailTemplatesConfig::default();
    let registered =
        templates.entries().into_iter().map(|(key, _)| key.to_owned()).collect::<BTreeSet<_>>();
    // 每个可编辑模板都必须在目录中登记。
    for key in &registered {
        assert!(keys.contains(key), "模板 {key} 未在通知目录登记");
    }
    // 目录中除冻结正文的通知外，都必须有可编辑模板。
    let frozen =
        NON_TEMPLATE_KINDS.into_iter().map(|kind| kind.label().to_owned()).collect::<BTreeSet<_>>();
    for key in &keys {
        if frozen.contains(key) {
            continue;
        }
        assert!(registered.contains(key), "目录键 {key} 缺少 Admin 模板注册");
    }
    // 冻结正文渠道的实现键也必须在模板枚举里，避免目录登记了不存在的通知。
    for kind in NotificationKind::ALL {
        if frozen.contains(kind.label()) {
            continue;
        }
        assert!(
            NotificationKind::from_template(kind.label()) == Some(kind),
            "目录键 {} 无法从模板名解析为通知种类",
            kind.label()
        );
    }
}

/// 正文模板键与 delivery payload 身份不同的已知别名；目录使用可编辑模板键。
const PAYLOAD_ALIASES: [(&str, &str); 1] =
    [("recovery_payment_reminder", "settlement_payment_reminder")];

#[test]
fn mail_notification_templates_map_to_catalog_kinds() {
    let keys = catalog_keys();
    for template in MailNotificationTemplate::ALL {
        let payload_identity = template.as_str();
        let label = PAYLOAD_ALIASES
            .into_iter()
            .find_map(|(payload, key)| (payload == payload_identity).then_some(key))
            .unwrap_or(payload_identity);
        // 结算模板属于另一套持久字符串，不在该枚举内；这里的每个键都必须是通知目录里的键。
        assert!(keys.contains(label), "MailNotificationTemplate::{label} 未在通知目录登记");
        assert_eq!(
            NotificationKind::from_template(label),
            Some(
                NotificationKind::ALL
                    .into_iter()
                    .find(|kind| kind.label() == label)
                    .unwrap_or_else(|| panic!("{label} 缺少 NotificationKind"))
            ),
            "模板 {label} 的指标种类与目录不一致"
        );
    }
}

#[test]
fn settlement_templates_are_registered_and_cataloged() {
    // 结算通知用固定字符串直接写入冻结 payload，因此单独核对它与模板注册、目录三方一致。
    const SETTLEMENT_KEYS: [&str; 6] = [
        "settlement_winner_equipment_cod",
        "settlement_payment_carrier",
        "settlement_payment_reminder",
        "settlement_equipment_handoff",
        "settlement_seller_payout",
        "settlement_buyer_refund",
    ];
    let keys = catalog_keys();
    let templates = AdminRuntimeMailTemplatesConfig::default();
    for key in SETTLEMENT_KEYS {
        assert!(keys.contains(key), "结算模板 {key} 未在通知目录登记");
        assert!(templates.body(key).is_some(), "结算模板 {key} 未在 Admin 模板注册");
        assert!(
            NotificationKind::from_template(key).is_some(),
            "结算模板 {key} 缺少 NotificationKind"
        );
    }
}
