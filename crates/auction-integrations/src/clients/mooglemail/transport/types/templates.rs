//! MoogleMail 通知模板快照及占位符校验。

use super::super::*;

const MOOGLEMAIL_TEMPLATE_KEYS: &[&str] = &[
    "auction_assets_rejected",
    "auction_no_attachments",
    "auction_equipment_rejected",
    "user_assets_rejected",
    "register_key",
    "register_result",
    "rebind_result",
    "settlement_winner_equipment_cod",
    "settlement_payment_carrier",
    "settlement_payment_reminder",
    "settlement_equipment_handoff",
    "settlement_seller_payout",
    "settlement_buyer_refund",
    "operations_alert",
];

/// Worker 启动时冻结的 MoogleMail 通知模板快照。
/// 快照只保存经过闭集键名和占位符校验的正文；发送过程中不再读取可变配置，避免同一
/// 批次出现部分旧模板、部分新模板的结果。
#[derive(Debug, Clone)]
pub struct MoogleMailNotificationTemplates {
    values: BTreeMap<String, String>,
}

impl Default for MoogleMailNotificationTemplates {
    fn default() -> Self {
        Self::from_values(BTreeMap::from([
            (
                "auction_assets_rejected".to_owned(),
                concat!(
                    "Your auction submission was not accepted.\n",
                    "\n",
                    "Auction submissions must contain equipment only.\n",
                    "Please remove Credits, CoD and other item types\n",
                    "before submitting again.",
                )
                .to_owned(),
            ),
            (
                "auction_no_attachments".to_owned(),
                concat!(
                    "No equipment attachments were found in your message.\n",
                    "\n",
                    "Attach the equipment you want to submit,\n",
                    "then send a new message with the subject \"auction\".",
                )
                .to_owned(),
            ),
            (
                "auction_equipment_rejected".to_owned(),
                concat!(
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
            ),
            (
                "user_assets_rejected".to_owned(),
                concat!(
                    "Your account request was not accepted.\n",
                    "\n",
                    "Account requests must not include equipment,\n",
                    "Credits, other items or CoD.\n",
                    "\n",
                    "Please resend your request without these assets.",
                )
                .to_owned(),
            ),
            (
                "register_key".to_owned(),
                concat!(
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
            ),
            (
                "register_result".to_owned(),
                concat!(
                    "Your registration could not be completed.\n",
                    "\n",
                    "Player: {username}\n",
                    "Result code: {result_code}\n",
                    "\n",
                    "Contact the auction operator if you need help\n",
                    "resolving this issue.",
                )
                .to_owned(),
            ),
            (
                "rebind_result".to_owned(),
                concat!(
                    "Your account rebind request has been processed.\n",
                    "\n",
                    "Player: {username}\n",
                    "Result code: {result_code}\n",
                    "\n",
                    "Contact the auction operator if you need help\n",
                    "understanding the result.",
                )
                .to_owned(),
            ),
            (
                "settlement_winner_equipment_cod".to_owned(),
                concat!(
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
            ),
            (
                "settlement_payment_carrier".to_owned(),
                concat!(
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
            ),
            (
                "settlement_payment_reminder".to_owned(),
                concat!(
                    "Payment for this auction transaction is overdue.\n",
                    "\n",
                    "Please check your original auction payment mail.\n",
                    "If you have already paid, contact the auction\n",
                    "operator with the reference below.\n",
                    "\n",
                    "Reference: {reference}",
                )
                .to_owned(),
            ),
            (
                "settlement_equipment_handoff".to_owned(),
                concat!(
                    "The following equipment is attached.\n",
                    "\n",
                    "Item: {item_name}\n",
                    "\n",
                    "Please collect the attachment.\n",
                    "\n",
                    "Reference: {reference}",
                )
                .to_owned(),
            ),
            (
                "settlement_seller_payout".to_owned(),
                concat!(
                    "Your auction payout is attached.\n",
                    "\n",
                    "Amount: {amount} Credits\n",
                    "\n",
                    "Please collect the attached Credits.\n",
                    "\n",
                    "Reference: {reference}",
                )
                .to_owned(),
            ),
            (
                "settlement_buyer_refund".to_owned(),
                concat!(
                    "Your auction refund is attached.\n",
                    "\n",
                    "Amount: {amount} Credits\n",
                    "\n",
                    "Please collect the attached Credits.\n",
                    "\n",
                    "Reference: {reference}",
                )
                .to_owned(),
            ),
            (
                "operations_alert".to_owned(),
                concat!(
                    "An auction service issue needs attention.\n",
                    "\n",
                    "{summary}\n",
                    "\n",
                    "Please review the Admin dashboard for details.",
                )
                .to_owned(),
            ),
        ]))
        .unwrap_or_else(|error| panic!("内置 MoogleMail 模板必须通过启动校验: {error}"))
    }
}

impl MoogleMailNotificationTemplates {
    /// 从配置快照构造模板，并在 Worker 启动阶段拒绝未知键、非法正文和未知占位符。
    pub fn from_values(values: BTreeMap<String, String>) -> Result<Self, String> {
        if values.len() != MOOGLEMAIL_TEMPLATE_KEYS.len()
            || MOOGLEMAIL_TEMPLATE_KEYS.iter().any(|key| !values.contains_key(*key))
        {
            return Err("MoogleMail mail_templates 必须包含且仅包含旧版闭集模板键".to_owned());
        }
        for key in MOOGLEMAIL_TEMPLATE_KEYS {
            let Some(value) = values.get(*key) else {
                return Err("MoogleMail mail_templates 必须包含且仅包含旧版闭集模板键".to_owned());
            };
            validate_template_body(key, value)?;
        }
        Ok(Self { values })
    }

    pub(crate) fn render(&self, key: &str, variables: &[(&str, &str)]) -> Option<String> {
        let template = self.values.get(key)?;
        let mut output = String::with_capacity(template.len());
        let mut cursor = 0;
        while let Some(relative_start) = template[cursor..].find('{') {
            let start = cursor + relative_start;
            let end = start + 1 + template[start + 1..].find('}')?;
            let placeholder = &template[start + 1..end];
            let value = variables.iter().find(|(name, _)| *name == placeholder)?.1;
            output.push_str(&template[cursor..start]);
            output.push_str(value);
            cursor = end + 1;
        }
        output.push_str(&template[cursor..]);
        Some(output)
    }
}

fn validate_template_body(key: &str, value: &str) -> Result<(), String> {
    if value.is_empty()
        || value != value.trim()
        || value.len() > 64 * 1024
        || value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(format!("MoogleMail mail_templates.{key} 正文格式无效"));
    }
    let (required, allowed): (&[&str], &[&str]) = match key {
        "auction_assets_rejected" | "auction_no_attachments" | "user_assets_rejected" => (&[], &[]),
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
        _ => return Err("MoogleMail mail_templates 含有未知键".to_owned()),
    };
    let mut cursor = 0;
    let mut seen = Vec::new();
    while let Some(relative_start) = value[cursor..].find('{') {
        let start = cursor + relative_start;
        if value[cursor..start].contains('}') {
            return Err(format!("MoogleMail mail_templates.{key} 占位符未闭合"));
        }
        let relative_end = value[start + 1..]
            .find('}')
            .ok_or_else(|| format!("MoogleMail mail_templates.{key} 占位符未闭合"))?;
        let end = start + 1 + relative_end;
        let placeholder = &value[start + 1..end];
        if !allowed.contains(&placeholder) {
            return Err(format!("MoogleMail mail_templates.{key} 含有未知占位符"));
        }
        if !seen.contains(&placeholder) {
            seen.push(placeholder);
        }
        cursor = end + 1;
    }
    // 拒收原因兼容旧原因码与新可读正文，和配置入口保持相同的至少一项约束。
    if value[cursor..].contains('}')
        || required.iter().any(|placeholder| !seen.contains(placeholder))
        || key == "auction_equipment_rejected"
            && !seen.contains(&"reason_code")
            && !seen.contains(&"reason_text")
    {
        return Err(format!("MoogleMail mail_templates.{key} 缺少必需占位符"));
    }
    Ok(())
}
