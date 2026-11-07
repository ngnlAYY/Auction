//! MoogleMail 通知模板的唯一 owner：字段闭集、默认正文与占位符规则。
//!
//! Admin runtime 文档、Worker 部署 TOML 与 Worker 启动快照共用本模块，
//! 避免三处默认正文或占位符规则漂移。

use serde::{Deserialize, Serialize};

/// 单份模板正文的字节上限，与 MoogleMail 单封正文上限一致。
const MAX_MAIL_TEMPLATE_BODY_BYTES: usize = 64 * 1024;

/// Admin 可编辑、Worker 启动时冻结的 MoogleMail 通知模板。
/// 字段采用结构化闭集而不是自由键值表，避免拼写错误或把未审计的占位符带入远端邮件；
/// 正文只允许 `{name}` 形式的已知占位符。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeMailTemplatesConfig {
    pub auction_assets_rejected: String,
    pub auction_no_attachments: String,
    /// 原因至少保留 `{reason_code}` 或 `{reason_text}`；另支持 `{next_steps}`、`{mail_reference}`。
    pub auction_equipment_rejected: String,
    pub user_assets_rejected: String,
    /// 注册成功或恢复成功时发送登录 key；要求 `{login_key}`、`{username}`。
    pub register_key: String,
    /// 注册结果通知；要求 `{result_code}`，可选 `{username}`。
    pub register_result: String,
    /// 重绑结果通知；要求 `{result_code}`，可选 `{username}`。
    pub rebind_result: String,
    /// 登录 key 自助轮换成功时发送新 key；要求 `{login_key}`、`{username}`。
    pub rekey_key: String,
    /// 轮换失败通知；要求 `{result_code}`，可选 `{username}`。
    pub rekey_result: String,
    /// 主世界竞得装备的货到付款正文；要求 `{item_name}`、`{amount}`、`{reference}`，
    /// 可选 `{price_breakdown}`，旧载荷没有冻结结算明细时该块为空。
    pub settlement_winner_equipment_cod: String,
    /// 异世界竞得付款载体正文；要求 `{amount}`、`{reference}`。
    pub settlement_payment_carrier: String,
    /// 结算付款超时正文；要求 `{reference}`。
    pub settlement_payment_reminder: String,
    /// 结算装备交付或退回正文；要求 `{item_name}`、`{reference}`。
    pub settlement_equipment_handoff: String,
    /// 卖家收款正文；要求 `{amount}`、`{reference}`。
    pub settlement_seller_payout: String,
    /// 买家退款正文；要求 `{amount}`、`{reference}`。
    pub settlement_buyer_refund: String,
    /// 出价被超的 MoogleMail 正文（W14.1）；要求 `{item_name}`、`{lot_code}`、`{amount}`，
    /// 可选 `{deadline_line}`、`{lot_url_line}`（展开为带标签的整行，缺失时为空）。
    pub lot_watch: String,
    /// 出价被超的论坛私信正文（W14.1）；占位符与 `lot_watch` 相同，并且必须保留
    /// `Notification: {notification_id}.` 这一句以绑定发送证明。
    pub lot_watch_pm: String,
    /// 运维告警正文；要求 `{summary}`。
    pub operations_alert: String,
}

impl Default for AdminRuntimeMailTemplatesConfig {
    fn default() -> Self {
        Self {
            auction_assets_rejected: concat!(
                "Your auction submission was not accepted.\n",
                "\n",
                "Auction submissions must contain equipment only.\n",
                "Please remove Credits, CoD and other item types\n",
                "before submitting again.",
            )
            .to_owned(),
            auction_no_attachments: concat!(
                "No equipment attachments were found in your message.\n",
                "\n",
                "Attach the equipment you want to submit,\n",
                "then send a new message with the subject \"auction\".",
            )
            .to_owned(),
            auction_equipment_rejected: concat!(
                "Your auction submission was not accepted.\n",
                "\n",
                "Reason:\n",
                "{reason_text}\n",
                "\n",
                "Equipment return:\n",
                "A return has been requested for all equipment\n",
                "in your original message.\n",
                "\n",
                "If you submitted multiple items together,\n",
                "the entire submission will be returned.\n",
                "\n",
                "What to do next:\n",
                "{next_steps}\n",
                "\n",
                "Original mail reference: {mail_reference}",
            )
            .to_owned(),
            user_assets_rejected: concat!(
                "Your account request was not accepted.\n",
                "\n",
                "Account requests must not include equipment,\n",
                "Credits, other items or CoD.\n",
                "\n",
                "Please resend your request without these assets.",
            )
            .to_owned(),
            register_key: concat!(
                "Your HV-Auction login key is ready.\n",
                "\n",
                "Player: {username}\n",
                "\n",
                "Login key:\n",
                "{login_key}\n",
                "\n",
                "Use this key to sign in to HV-Auction.\n",
                "Keep it private.",
            )
            .to_owned(),
            register_result: concat!(
                "Your registration could not be completed.\n",
                "\n",
                "Player: {username}\n",
                "Result code: {result_code}\n",
                "\n",
                "Contact the auction operator if you need help\n",
                "resolving this issue.",
            )
            .to_owned(),
            rebind_result: concat!(
                "Your account rebind request has been processed.\n",
                "\n",
                "Player: {username}\n",
                "Result code: {result_code}\n",
                "\n",
                "Contact the auction operator if you need help\n",
                "understanding the result.",
            )
            .to_owned(),
            rekey_key: concat!(
                "Your HV-Auction login key has been rotated.\n",
                "\n",
                "Player: {username}\n",
                "\n",
                "New login key:\n",
                "{login_key}\n",
                "\n",
                "All previous sessions have been signed out.\n",
                "Keep this key private.",
            )
            .to_owned(),
            rekey_result: concat!(
                "Your login key rotation request could not be completed.\n",
                "\n",
                "Player: {username}\n",
                "Result code: {result_code}\n",
                "\n",
                "Send a message with the subject \"register\" first\n",
                "if you have not registered yet.\n",
                "Contact the auction operator if you need help\n",
                "understanding the result.",
            )
            .to_owned(),
            settlement_winner_equipment_cod: concat!(
                "Your auction item is ready to collect.\n",
                "\n",
                "Item: {item_name}\n",
                "\n",
                "{price_breakdown}Amount due (CoD): {amount} Credits\n",
                "Payment deadline: {payment_deadline}\n",
                "\n",
                "Collect the attached equipment to pay the CoD\n",
                "and receive your item.\n",
                "\n",
                "Reference: {reference}",
            )
            .to_owned(),
            settlement_payment_carrier: concat!(
                "This mail collects payment for your Isekai auction item.\n",
                "\n",
                "Item: {item_name}\n",
                "CoD: {amount} Credits\n",
                "Payment deadline: {payment_deadline}\n",
                "\n",
                "Collect the attached Credits to pay the CoD.\n",
                "Your equipment will be sent separately in Isekai\n",
                "after payment is confirmed.\n",
                "\n",
                "Reference: {reference}",
            )
            .to_owned(),
            settlement_payment_reminder: concat!(
                "Payment for this auction transaction is overdue.\n",
                "\n",
                "Please check your original auction payment mail.\n",
                "If you have already paid, contact the auction\n",
                "operator with the reference below.\n",
                "\n",
                "Reference: {reference}",
            )
            .to_owned(),
            settlement_equipment_handoff: concat!(
                "The following equipment is attached.\n",
                "\n",
                "Item: {item_name}\n",
                "\n",
                "Please collect the attachment.\n",
                "\n",
                "Reference: {reference}",
            )
            .to_owned(),
            settlement_seller_payout: concat!(
                "Your auction payout is attached.\n",
                "\n",
                "Item: {item_name}\n",
                "Amount: {amount} Credits\n",
                "\n",
                "Please collect the attached Credits.\n",
                "\n",
                "Reference: {reference}",
            )
            .to_owned(),
            settlement_buyer_refund: concat!(
                "Your auction refund is attached.\n",
                "\n",
                "Item: {item_name}\n",
                "Amount: {amount} Credits\n",
                "\n",
                "Please collect the attached Credits.\n",
                "\n",
                "Reference: {reference}",
            )
            .to_owned(),
            // 默认正文与 W14.2 冻结正文逐字一致：{deadline_line}、{lot_url_line} 缺失时展开为空行。
            lot_watch: concat!(
                "Your leading bid has been outbid.\n",
                "\n",
                "Item: {item_name}\n",
                "Lot: {lot_code}\n",
                "Accepted bid: {amount} Credits\n",
                "{deadline_line}{lot_url_line}",
                "\n",
                "Check the auction website for the latest price\n",
                "and place another bid if you wish to continue.",
            )
            .to_owned(),
            lot_watch_pm: concat!(
                "Your leading bid has been outbid.\n",
                "\n",
                "Item: {item_name}\n",
                "Lot: {lot_code}\n",
                "Accepted bid: {amount} Credits\n",
                "{deadline_line}{lot_url_line}",
                "\n",
                "Notification: {notification_id}.",
            )
            .to_owned(),
            operations_alert: concat!(
                "An auction service issue needs attention.\n",
                "\n",
                "{summary}\n",
                "\n",
                "Please review the Admin dashboard for details.",
            )
            .to_owned(),
        }
    }
}

/// 模板正文校验失败原因；消息不回显正文，供各配置入口映射为自身错误类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailTemplateError {
    InvalidBody,
    UnclosedPlaceholder,
    UnknownPlaceholder,
    MissingPlaceholder,
    MissingRejectionReason,
    MissingNotificationIdentity,
}

impl MailTemplateError {
    /// 稳定的中文错误消息。
    pub const fn message(self) -> &'static str {
        match self {
            Self::InvalidBody => "mail_templates 正文格式无效",
            Self::UnclosedPlaceholder => "mail_templates 占位符未闭合",
            Self::UnknownPlaceholder => "mail_templates 含有未知占位符",
            Self::MissingPlaceholder => "mail_templates 缺少必需占位符",
            Self::MissingRejectionReason => {
                "mail_templates.auction_equipment_rejected 必须包含 reason_code 或 reason_text"
            }
            Self::MissingNotificationIdentity => {
                "mail_templates.lot_watch_pm 必须包含 Notification: {notification_id}."
            }
        }
    }
}

impl AdminRuntimeMailTemplatesConfig {
    /// 按固定闭集顺序返回 `(模板键, 正文)`；键名即 Worker 渲染入口。
    pub fn entries(&self) -> [(&'static str, &str); 18] {
        [
            ("auction_assets_rejected", &self.auction_assets_rejected),
            ("auction_no_attachments", &self.auction_no_attachments),
            ("auction_equipment_rejected", &self.auction_equipment_rejected),
            ("user_assets_rejected", &self.user_assets_rejected),
            ("register_key", &self.register_key),
            ("register_result", &self.register_result),
            ("rebind_result", &self.rebind_result),
            ("rekey_key", &self.rekey_key),
            ("rekey_result", &self.rekey_result),
            ("settlement_winner_equipment_cod", &self.settlement_winner_equipment_cod),
            ("settlement_payment_carrier", &self.settlement_payment_carrier),
            ("settlement_payment_reminder", &self.settlement_payment_reminder),
            ("settlement_equipment_handoff", &self.settlement_equipment_handoff),
            ("settlement_seller_payout", &self.settlement_seller_payout),
            ("settlement_buyer_refund", &self.settlement_buyer_refund),
            ("lot_watch", &self.lot_watch),
            ("lot_watch_pm", &self.lot_watch_pm),
            ("operations_alert", &self.operations_alert),
        ]
    }

    /// 返回指定键的模板正文；未知键返回 `None`。
    pub fn body(&self, key: &str) -> Option<&str> {
        self.entries().into_iter().find_map(|(name, body)| (name == key).then_some(body))
    }

    /// 用给定变量展开模板；未登记占位符、缺少变量或展开后超出正文上限时返回 `None`。
    ///
    /// 出价被超提醒在**创建时**调用本方法，把结果冻结进投递载荷；发送阶段不再读取可变模板。
    /// 可选占位符由调用方传空串表示“该行不出现”，渲染本身不接受缺项。
    pub fn render(&self, key: &str, variables: &[(&str, &str)]) -> Option<String> {
        let template = self.body(key)?;
        let mut output = String::with_capacity(template.len());
        let mut cursor = 0;
        while let Some(relative_start) = template[cursor..].find('{') {
            let start = cursor + relative_start;
            let end = start + 1 + template[start + 1..].find('}')?;
            let placeholder = &template[start + 1..end];
            let value = variables.iter().find(|(name, _)| *name == placeholder)?.1;
            // 按展开后的字节数限流；重复占位符不能先膨胀成巨型正文再交给发送校验。
            if output.len().checked_add(start - cursor)?.checked_add(value.len())?
                > MAX_MAIL_TEMPLATE_BODY_BYTES
            {
                return None;
            }
            output.push_str(&template[cursor..start]);
            output.push_str(value);
            cursor = end + 1;
        }
        if output.len().checked_add(template.len() - cursor)? > MAX_MAIL_TEMPLATE_BODY_BYTES {
            return None;
        }
        output.push_str(&template[cursor..]);
        Some(output)
    }

    /// 校验全部正文格式与占位符，返回首个失败的模板键和原因。
    pub fn validate(&self) -> Result<(), (&'static str, MailTemplateError)> {
        for (key, body) in self.entries() {
            let (required, allowed) = placeholder_rule(key);
            validate_body(body, required, allowed).map_err(|error| (key, error))?;
        }
        // 保留旧自定义正文的原因码，也允许新正文用可读原因替代；至少展示其中一种原因。
        if !self.auction_equipment_rejected.contains("{reason_code}")
            && !self.auction_equipment_rejected.contains("{reason_text}")
        {
            return Err(("auction_equipment_rejected", MailTemplateError::MissingRejectionReason));
        }
        // 私信发送证明按 `Notification: <id>.` 绑定冻结正文；自定义模板必须保留这一句。
        if !self.lot_watch_pm.contains("Notification: {notification_id}.") {
            return Err(("lot_watch_pm", MailTemplateError::MissingNotificationIdentity));
        }
        // 被超提醒在创建时冻结正文，必须先满足投递正文上限，否则每封新通知都会回落默认正文。
        for key in ["lot_watch", "lot_watch_pm"] {
            if !self
                .body(key)
                .is_some_and(|body| crate::ports::mail::valid_notification_body(Some(body)))
            {
                return Err((key, MailTemplateError::InvalidBody));
            }
        }
        Ok(())
    }
}

/// 返回模板键的 `(必需占位符, 允许占位符)`；键集合由 [`AdminRuntimeMailTemplatesConfig::entries`] 封闭。
fn placeholder_rule(key: &str) -> (&'static [&'static str], &'static [&'static str]) {
    match key {
        "auction_equipment_rejected" => {
            (&[], &["reason_code", "reason_text", "next_steps", "mail_reference"])
        }
        "register_key" => (&["login_key", "username"], &["login_key", "username"]),
        "register_result" | "rebind_result" | "rekey_result" => {
            (&["result_code"], &["result_code", "username"])
        }
        "rekey_key" => (&["login_key", "username"], &["login_key", "username"]),
        "settlement_winner_equipment_cod" => (
            &["item_name", "amount", "reference"],
            &["item_name", "amount", "reference", "price_breakdown", "payment_deadline"],
        ),
        "settlement_payment_carrier" => {
            (&["amount", "reference"], &["amount", "reference", "item_name", "payment_deadline"])
        }
        "settlement_seller_payout" | "settlement_buyer_refund" => {
            (&["amount", "reference"], &["amount", "reference", "item_name"])
        }
        "settlement_payment_reminder" => (&["reference"], &["reference"]),
        "settlement_equipment_handoff" => {
            (&["item_name", "reference"], &["item_name", "reference"])
        }
        "lot_watch" => (
            &["item_name", "lot_code", "amount"],
            &["item_name", "lot_code", "amount", "deadline_line", "lot_url_line"],
        ),
        "lot_watch_pm" => (
            &["item_name", "lot_code", "amount"],
            &[
                "item_name",
                "lot_code",
                "amount",
                "deadline_line",
                "lot_url_line",
                "notification_id",
            ],
        ),
        "operations_alert" => (&["summary"], &["summary"]),
        // 拒收类正文不接受任何占位符。
        _ => (&[], &[]),
    }
}

fn validate_body(
    value: &str,
    required: &[&str],
    allowed: &[&str],
) -> Result<(), MailTemplateError> {
    if value.is_empty()
        || value != value.trim()
        || value.len() > MAX_MAIL_TEMPLATE_BODY_BYTES
        || value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(MailTemplateError::InvalidBody);
    }
    let mut cursor = 0;
    let mut seen = Vec::with_capacity(allowed.len());
    while let Some(relative_start) = value[cursor..].find('{') {
        let start = cursor + relative_start;
        if value[cursor..start].contains('}') {
            return Err(MailTemplateError::UnclosedPlaceholder);
        }
        let end = value[start + 1..]
            .find('}')
            .map(|relative_end| start + 1 + relative_end)
            .ok_or(MailTemplateError::UnclosedPlaceholder)?;
        let placeholder = &value[start + 1..end];
        if !allowed.contains(&placeholder) {
            return Err(MailTemplateError::UnknownPlaceholder);
        }
        if !seen.contains(&placeholder) {
            seen.push(placeholder);
        }
        cursor = end + 1;
    }
    if value[cursor..].contains('}') {
        return Err(MailTemplateError::UnclosedPlaceholder);
    }
    if required.iter().any(|placeholder| !seen.contains(placeholder)) {
        return Err(MailTemplateError::MissingPlaceholder);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
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
            settlement_buyer_refund: "Refund for {item_name}: {amount} Credits\n{reference}"
                .to_owned(),
            ..AdminRuntimeMailTemplatesConfig::default()
        };
        refund_allowed.validate().expect("item_name 是退款邮件的允许占位符");
        let refund_unknown = AdminRuntimeMailTemplatesConfig {
            settlement_buyer_refund:
                "Refund for {item_name}: {amount} Credits\n{reference}\n{payment_deadline}"
                    .to_owned(),
            ..AdminRuntimeMailTemplatesConfig::default()
        };
        assert_eq!(
            refund_unknown.validate(),
            Err(("settlement_buyer_refund", MailTemplateError::UnknownPlaceholder))
        );
        // 载体付款邮件允许冻结期限，同时仍要求金额与引用。
        let carrier_allowed = AdminRuntimeMailTemplatesConfig {
            settlement_payment_carrier:
                "Pay {amount} Credits for {item_name}\nby {payment_deadline}\n{reference}"
                    .to_owned(),
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
        assert_eq!(
            pm_unknown.validate(),
            Err(("lot_watch_pm", MailTemplateError::UnknownPlaceholder))
        );

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
}
