//! 邮件模板闭集、默认正文与占位符规则的回归用例。

use self::parent::*;
use crate::ports::admin::runtime_config::mail_templates as parent;

#[test]
fn default_settlement_templates_state_deadline_and_item() {
    let templates = AdminRuntimeMailTemplatesConfig::default();
    templates.validate().expect("默认模板必须自洽");
    let cod = templates.body("settlement_winner_equipment_cod").unwrap_or_default();
    assert!(cod.contains("{payment_deadline}"), "{cod}");
    assert!(cod.contains("{item_name}"), "{cod}");
    let carrier = templates.body("settlement_payment_carrier").unwrap_or_default();
    assert!(carrier.contains("{payment_deadline}"), "{carrier}");
    assert!(carrier.contains("{item_name}"), "{carrier}");
    for key in ["settlement_seller_payout", "settlement_buyer_refund"] {
        let body = templates.body(key).unwrap_or_default();
        assert!(body.contains("{item_name}"), "{key}: {body}");
        // 期限只属于付款请求；金额邮件不得引用它。
        assert!(!body.contains("{payment_deadline}"), "{key}: {body}");
    }
}

#[test]
fn settlement_placeholder_closed_set_rejects_unknown_and_allows_new_keys() {
    // 金额邮件可写装备名，但不能引入未登记变量。
    let refund_allowed = AdminRuntimeMailTemplatesConfig {
        settlement_buyer_refund: "Refund for {item_name}: {amount} Credits\n{reference}".to_owned(),
        ..AdminRuntimeMailTemplatesConfig::default()
    };
    refund_allowed.validate().expect("item_name 是退款邮件的允许占位符");
    let refund_unknown = AdminRuntimeMailTemplatesConfig {
        settlement_buyer_refund:
            "Refund for {item_name}: {amount} Credits\n{reference}\n{payment_deadline}".to_owned(),
        ..AdminRuntimeMailTemplatesConfig::default()
    };
    assert_eq!(
        refund_unknown.validate(),
        Err(("settlement_buyer_refund", MailTemplateError::UnknownPlaceholder))
    );
    // 载体付款邮件允许冻结期限，同时仍要求金额与引用。
    let carrier_allowed = AdminRuntimeMailTemplatesConfig {
        settlement_payment_carrier:
            "Pay {amount} Credits for {item_name}\nby {payment_deadline}\n{reference}".to_owned(),
        ..AdminRuntimeMailTemplatesConfig::default()
    };
    carrier_allowed.validate().expect("载体付款邮件的期限是允许占位符");
    let carrier_missing = AdminRuntimeMailTemplatesConfig {
        settlement_payment_carrier: "Pay {amount} Credits by {payment_deadline}".to_owned(),
        ..AdminRuntimeMailTemplatesConfig::default()
    };
    assert_eq!(
        carrier_missing.validate(),
        Err(("settlement_payment_carrier", MailTemplateError::MissingPlaceholder))
    );
}

#[test]
fn default_lot_watch_templates_preserve_the_frozen_english_bodies() {
    let templates = AdminRuntimeMailTemplatesConfig::default();
    templates.validate().expect("默认模板必须自洽");
    let mail = templates.body("lot_watch").unwrap_or_default();
    for placeholder in
        ["{item_name}", "{lot_code}", "{amount}", "{deadline_line}", "{lot_url_line}"]
    {
        assert!(mail.contains(placeholder), "{placeholder}: {mail}");
    }
    assert!(!mail.contains("{notification_id}"), "MM 正文不得引用 PM 身份：{mail}");
    let pm = templates.body("lot_watch_pm").unwrap_or_default();
    assert!(pm.contains("{notification_id}"), "{pm}");
    // 两个渠道保留原有措辞，模板改写只影响新通知。
    assert!(mail.contains("Check the auction website for the latest price"), "{mail}");
    assert!(pm.contains("Notification: {notification_id}."), "{pm}");

    let rendered = templates
        .render(
            "lot_watch",
            &[
                ("item_name", "Phase Robe of Niflheim"),
                ("lot_code", "Clo16"),
                ("amount", "5,252,000"),
                ("deadline_line", "Bidding closes: 2026-11-05T12:30:00Z\n"),
                ("lot_url_line", "View the lot: https://auction.example.com/lots/Clo16\n"),
                ("notification_id", "ignored"),
            ],
        )
        .expect("默认模板必须可渲染");
    assert!(rendered.contains("Accepted bid: 5,252,000 Credits\nBidding closes:"));
    assert!(rendered.ends_with("place another bid if you wish to continue."));
}

#[test]
fn lot_watch_placeholder_rules_are_closed() {
    // 未登记占位符在配置写入时被拒绝，而不是渲染时静默保留。
    let unknown = AdminRuntimeMailTemplatesConfig {
        lot_watch: "Outbid {item_name} {lot_code} {amount} {proxy_maximum}".to_owned(),
        ..AdminRuntimeMailTemplatesConfig::default()
    };
    assert_eq!(unknown.validate(), Err(("lot_watch", MailTemplateError::UnknownPlaceholder)));
    let pm_unknown = AdminRuntimeMailTemplatesConfig {
        lot_watch_pm: "Outbid {item_name} {lot_code} {amount} {reference}".to_owned(),
        ..AdminRuntimeMailTemplatesConfig::default()
    };
    assert_eq!(pm_unknown.validate(), Err(("lot_watch_pm", MailTemplateError::UnknownPlaceholder)));

    // 必填标识缺失时配置无效。
    let missing = AdminRuntimeMailTemplatesConfig {
        lot_watch: "Outbid {item_name} at {amount} Credits".to_owned(),
        ..AdminRuntimeMailTemplatesConfig::default()
    };
    assert_eq!(missing.validate(), Err(("lot_watch", MailTemplateError::MissingPlaceholder)));

    // 可选行与 PM 身份在各自键内被接受。
    let optional = AdminRuntimeMailTemplatesConfig {
        lot_watch: "Outbid {item_name} {lot_code} {amount} {deadline_line}".to_owned(),
        lot_watch_pm: "{item_name} {lot_code} {amount} {lot_url_line}\n\
                       Notification: {notification_id}."
            .to_owned(),
        ..AdminRuntimeMailTemplatesConfig::default()
    };
    optional.validate().expect("可选占位符与 PM 身份必须被接受");

    // 私信发送证明要求保留身份句；删掉即无法通过校验。
    let missing_identity = AdminRuntimeMailTemplatesConfig {
        lot_watch_pm: "{item_name} {lot_code} {amount} {notification_id}".to_owned(),
        ..AdminRuntimeMailTemplatesConfig::default()
    };
    assert_eq!(
        missing_identity.validate(),
        Err(("lot_watch_pm", MailTemplateError::MissingNotificationIdentity))
    );
}

#[test]
fn render_rejects_missing_variables_and_overflow() {
    let templates = AdminRuntimeMailTemplatesConfig::default();
    let minimal = [
        ("item_name", "item"),
        ("lot_code", "Clo01"),
        ("amount", "1,000"),
        ("deadline_line", ""),
        ("lot_url_line", ""),
    ];
    assert!(templates.render("lot_watch", &minimal).is_some());
    assert!(templates.render("lot_watch", &minimal[..3]).is_none(), "缺可选变量也必须显式传空");
    assert!(templates.render("unknown_key", &minimal).is_none());

    // 展开后超过 64 KiB 的正文不可渲染；重复占位符同样按展开后大小累计。
    let repeated = AdminRuntimeMailTemplatesConfig {
        lot_watch: "{item_name}{item_name}{item_name}{lot_code}{amount}".to_owned(),
        ..AdminRuntimeMailTemplatesConfig::default()
    };
    let huge = "x".repeat(MAX_MAIL_TEMPLATE_BODY_BYTES / 3 + 1);
    assert!(
        repeated
            .render(
                "lot_watch",
                &[
                    ("item_name", huge.as_str()),
                    ("lot_code", "Clo01"),
                    ("amount", "1,000"),
                    ("deadline_line", ""),
                    ("lot_url_line", ""),
                ],
            )
            .is_none()
    );
}

#[test]
fn oversized_lot_watch_template_body_is_rejected() {
    let oversized = AdminRuntimeMailTemplatesConfig {
        lot_watch: "x".repeat(MAX_MAIL_TEMPLATE_BODY_BYTES + 1),
        ..AdminRuntimeMailTemplatesConfig::default()
    };
    assert_eq!(oversized.validate(), Err(("lot_watch", MailTemplateError::InvalidBody)));

    // 被超提醒正文还要满足 16 KiB 的投递上限，否则创建时无法冻结为可发送载荷。
    let over_delivery_limit = AdminRuntimeMailTemplatesConfig {
        lot_watch_pm: format!(
            "{{item_name}} {{lot_code}} {{amount}} {}Notification: {{notification_id}}.",
            "x".repeat(16 << 10)
        ),
        ..AdminRuntimeMailTemplatesConfig::default()
    };
    assert_eq!(
        over_delivery_limit.validate(),
        Err(("lot_watch_pm", MailTemplateError::InvalidBody))
    );
}
