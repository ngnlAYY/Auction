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
///
/// 快照只保存经过闭集键名和占位符校验的正文；发送过程中不再读取可变配置，避免同一
/// 批次出现部分旧模板、部分新模板的结果。
#[derive(Debug, Clone)]
pub struct MoogleMailNotificationTemplates {
    values: BTreeMap<String, String>,
}

impl Default for MoogleMailNotificationTemplates {
    fn default() -> Self {
        Self::from_values(BTreeMap::from([
            ("auction_assets_rejected".to_owned(), "Auction assets rejected".to_owned()),
            ("auction_no_attachments".to_owned(), "Auction has no attachments".to_owned()),
            (
                "auction_equipment_rejected".to_owned(),
                "Auction equipment rejected: {reason_code}".to_owned(),
            ),
            ("user_assets_rejected".to_owned(), "User assets rejected".to_owned()),
            ("register_key".to_owned(), "Registration key {login_key} for {username}".to_owned()),
            (
                "register_result".to_owned(),
                "Registration result {result_code} {username}".to_owned(),
            ),
            ("rebind_result".to_owned(), "Rebind result {result_code} {username}".to_owned()),
            (
                "settlement_winner_equipment_cod".to_owned(),
                "Auction item {item_name}; COD {amount}; reference {reference}".to_owned(),
            ),
            (
                "settlement_payment_carrier".to_owned(),
                "Auction payment request {amount}; reference {reference}".to_owned(),
            ),
            (
                "settlement_payment_reminder".to_owned(),
                "Auction payment is overdue; reference {reference}".to_owned(),
            ),
            (
                "settlement_equipment_handoff".to_owned(),
                "Auction item {item_name}; reference {reference}".to_owned(),
            ),
            (
                "settlement_seller_payout".to_owned(),
                "Auction seller payout {amount}; reference {reference}".to_owned(),
            ),
            (
                "settlement_buyer_refund".to_owned(),
                "Auction buyer refund {amount}; reference {reference}".to_owned(),
            ),
            ("operations_alert".to_owned(), "Operations alert: {summary}".to_owned()),
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
        "auction_equipment_rejected" => (&["reason_code"], &["reason_code"]),
        "register_key" => (&["login_key", "username"], &["login_key", "username"]),
        "register_result" | "rebind_result" => {
            (&["result_code", "username"], &["result_code", "username"])
        }
        "settlement_winner_equipment_cod" => {
            (&["item_name", "amount", "reference"], &["item_name", "amount", "reference"])
        }
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
    if value[cursor..].contains('}')
        || required.iter().any(|placeholder| !seen.contains(placeholder))
    {
        return Err(format!("MoogleMail mail_templates.{key} 缺少必需占位符"));
    }
    Ok(())
}
