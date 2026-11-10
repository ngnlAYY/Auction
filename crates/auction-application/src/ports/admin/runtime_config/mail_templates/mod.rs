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
    /// 出价被超的 MoogleMail 正文；要求 `{item_name}`、`{lot_code}`、`{amount}`，
    /// 可选 `{deadline_line}`、`{lot_url_line}`（展开为带标签的整行，缺失时为空）。
    pub lot_watch: String,
    /// 出价被超的论坛私信正文；占位符与 `lot_watch` 相同，并且必须保留
    /// `Notification: {notification_id}.` 这一句以绑定发送证明。
    pub lot_watch_pm: String,
    /// 运维告警正文；要求 `{summary}`。
    pub operations_alert: String,
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
mod defaults;
mod validation;

use validation::{placeholder_rule, validate_body};

#[cfg(test)]
mod tests;
