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
            settlement_winner_equipment_cod: concat!(
                "Your auction item is ready to collect.\n",
                "\n",
                "Item: {item_name}\n",
                "\n",
                "{price_breakdown}Amount due (CoD): {amount} Credits\n",
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
                "CoD: {amount} Credits\n",
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
                "Amount: {amount} Credits\n",
                "\n",
                "Please collect the attached Credits.\n",
                "\n",
                "Reference: {reference}",
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
        }
    }
}

impl AdminRuntimeMailTemplatesConfig {
    /// 按固定闭集顺序返回 `(模板键, 正文)`；键名即 Worker 渲染入口。
    pub fn entries(&self) -> [(&'static str, &str); 14] {
        [
            ("auction_assets_rejected", &self.auction_assets_rejected),
            ("auction_no_attachments", &self.auction_no_attachments),
            ("auction_equipment_rejected", &self.auction_equipment_rejected),
            ("user_assets_rejected", &self.user_assets_rejected),
            ("register_key", &self.register_key),
            ("register_result", &self.register_result),
            ("rebind_result", &self.rebind_result),
            ("settlement_winner_equipment_cod", &self.settlement_winner_equipment_cod),
            ("settlement_payment_carrier", &self.settlement_payment_carrier),
            ("settlement_payment_reminder", &self.settlement_payment_reminder),
            ("settlement_equipment_handoff", &self.settlement_equipment_handoff),
            ("settlement_seller_payout", &self.settlement_seller_payout),
            ("settlement_buyer_refund", &self.settlement_buyer_refund),
            ("operations_alert", &self.operations_alert),
        ]
    }

    /// 返回指定键的模板正文；未知键返回 `None`。
    pub fn body(&self, key: &str) -> Option<&str> {
        self.entries().into_iter().find_map(|(name, body)| (name == key).then_some(body))
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
        "register_result" | "rebind_result" => (&["result_code"], &["result_code", "username"]),
        "settlement_winner_equipment_cod" => (
            &["item_name", "amount", "reference"],
            &["item_name", "amount", "reference", "price_breakdown"],
        ),
        "settlement_payment_carrier" | "settlement_seller_payout" | "settlement_buyer_refund" => {
            (&["amount", "reference"], &["amount", "reference"])
        }
        "settlement_payment_reminder" => (&["reference"], &["reference"]),
        "settlement_equipment_handoff" => {
            (&["item_name", "reference"], &["item_name", "reference"])
        }
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
