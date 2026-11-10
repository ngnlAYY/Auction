//! 出站 MoogleMail 的预算与分类开关读取端口。
//!
//! 预算与开关只决定通知是否入队，不参与金额、资格、幂等或审计；关闭某类通知不改变已提交的
//! 命令、履约义务或审计事实（不变量 3、6）。Worker 的完整热快照通过本端口暴露给各生成点，
//! 避免每个服务再持有一份可独立写入的状态。

use std::fmt::Debug;

use serde::{Deserialize, Serialize};

/// 出站 MoogleMail 的业务大类；runtime `operations.mail.send_classes` 按此闭集开关。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailSendClass {
    /// 账号回信：注册、换绑、登录 key 轮换与账号资产拒收说明。
    AccountReply,
    /// 投稿回信：投稿接收回执与拍卖资产/装备拒收说明。
    IntakeReply,
    /// 结算付款提醒；COD、交付、货款与退款等履约动作邮件不受开通关影响。
    SettlementNotice,
    /// 出价被超提醒（MoogleMail 渠道）。
    OutbidReminder,
    /// 卖方成交/流拍汇总与换季提醒。
    PlayerSummary,
    /// 运维告警的 MoogleMail 通知；告警台记录不受影响。
    OperationsAlert,
}

impl MailSendClass {
    /// 固定顺序与 TOML 字段一致。
    pub const ALL: [Self; 6] = [
        Self::AccountReply,
        Self::IntakeReply,
        Self::SettlementNotice,
        Self::OutbidReminder,
        Self::PlayerSummary,
        Self::OperationsAlert,
    ];

    /// 稳定标签；同时是 runtime 配置字段名。
    pub const fn label(self) -> &'static str {
        match self {
            Self::AccountReply => "account_reply",
            Self::IntakeReply => "intake_reply",
            Self::SettlementNotice => "settlement_notice",
            Self::OutbidReminder => "outbid_reminder",
            Self::PlayerSummary => "player_summary",
            Self::OperationsAlert => "operations_alert",
        }
    }

    /// 该业务大类的 `(origin_kind, template)` 模式；`*` 表示该来源的全部模板。
    pub const fn delivery_patterns(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::AccountReply => &[
                ("mail_intake", "register_key"),
                ("mail_intake", "register_result"),
                ("mail_intake", "rekey_key"),
                ("mail_intake", "rekey_result"),
                ("mail_intake", "rebind_result"),
                ("mail_intake", "user_assets_rejected"),
                ("mail_intake", "forum_auction_notice"),
            ],
            Self::IntakeReply => &[
                ("mail_intake", "auction_assets_rejected"),
                ("mail_intake", "auction_no_attachments"),
                ("mail_intake", "auction_equipment_rejected"),
                ("mail_intake", "auction_submission_receipt"),
            ],
            // 只匹配付款提醒这一条纯通知；COD、交付、货款与退款等结算履约动作不在开关范围内，
            // 关闭它们会让义务无法推进，违反“通知开关不改变履约”的边界。
            Self::SettlementNotice => {
                &[("settlement_recovery_action", "recovery_payment_reminder")]
            }
            Self::OutbidReminder => &[("lot_watch", "*")],
            Self::PlayerSummary => &[("seller_summary", "*"), ("season_reminder", "*")],
            Self::OperationsAlert => &[("operations_alert", "*")],
        }
    }

    /// 从持久投递的 `origin_kind` 与 payload 模板投影业务大类。
    ///
    /// 未登记的来源或模板返回 `None`：新增通知默认发送，不能因分类缺失被静默抑制。
    pub fn from_delivery(origin_kind: &str, template: Option<&str>) -> Option<Self> {
        match origin_kind {
            "mail_intake" => match template {
                Some(
                    "register_key"
                    | "register_result"
                    | "rekey_key"
                    | "rekey_result"
                    | "rebind_result"
                    | "user_assets_rejected"
                    | "forum_auction_notice",
                ) => Some(Self::AccountReply),
                Some(
                    "auction_assets_rejected"
                    | "auction_no_attachments"
                    | "auction_equipment_rejected"
                    | "auction_submission_receipt",
                ) => Some(Self::IntakeReply),
                _ => None,
            },
            "settlement_recovery_action" => matches!(template, Some("recovery_payment_reminder"))
                .then_some(Self::SettlementNotice),
            // 普通 settlement 投递承载 COD、交付、货款与退款等履约动作，不参与分类开关。
            "settlement" => None,
            "lot_watch" => Some(Self::OutbidReminder),
            "seller_summary" | "season_reminder" => Some(Self::PlayerSummary),
            "operations_alert" => Some(Self::OperationsAlert),
            _ => None,
        }
    }
}

/// 六类出站邮件的发送开关；全部关闭不影响履约动作与审计。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "snake_case")]
pub struct MailSendClasses {
    pub account_reply: bool,
    pub intake_reply: bool,
    pub settlement_notice: bool,
    pub outbid_reminder: bool,
    pub player_summary: bool,
    pub operations_alert: bool,
}

impl Default for MailSendClasses {
    fn default() -> Self {
        Self {
            account_reply: true,
            intake_reply: true,
            settlement_notice: true,
            outbid_reminder: true,
            player_summary: true,
            operations_alert: true,
        }
    }
}

impl MailSendClasses {
    /// 该类通知当前是否允许入队。
    pub const fn allows(self, class: MailSendClass) -> bool {
        match class {
            MailSendClass::AccountReply => self.account_reply,
            MailSendClass::IntakeReply => self.intake_reply,
            MailSendClass::SettlementNotice => self.settlement_notice,
            MailSendClass::OutbidReminder => self.outbid_reminder,
            MailSendClass::PlayerSummary => self.player_summary,
            MailSendClass::OperationsAlert => self.operations_alert,
        }
    }

    /// 按持久投递投影判断；未登记组合不抑制。
    pub fn allows_delivery(self, origin_kind: &str, template: Option<&str>) -> bool {
        MailSendClass::from_delivery(origin_kind, template).is_none_or(|class| self.allows(class))
    }

    /// 关闭类对应的 `(origin_kind, template)` 领取排除模式；`*` 匹配该来源的全部模板。
    /// 返回两个平行数组，SQL 与本函数共用同一份分类闭集。
    pub fn suppressed_delivery_patterns(self) -> (Vec<&'static str>, Vec<&'static str>) {
        let mut origins = Vec::new();
        let mut templates = Vec::new();
        for class in MailSendClass::ALL {
            if self.allows(class) {
                continue;
            }
            for &(origin, template) in class.delivery_patterns() {
                origins.push(origin);
                templates.push(template);
            }
        }
        (origins, templates)
    }
}

/// 同一账号注册/轮换回信、说明信与出站分类开关的当前快照。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailReplyPolicy {
    /// 同一账号注册/轮换回信的窗口秒数。
    pub account_reply_window_seconds: u64,
    /// 说明信窗口秒数。
    pub explanation_mail_window_seconds: u64,
    /// 说明信窗口内最大封数；0 表示不限。
    pub explanation_mail_limit: u32,
    /// 六类出站通知的发送开关。
    pub send_classes: MailSendClasses,
}

impl MailReplyPolicy {
    /// 与启动配置和 runtime 校验保持一致。
    pub fn is_valid(self) -> bool {
        (60..=604_800).contains(&self.account_reply_window_seconds)
            && (60..=604_800).contains(&self.explanation_mail_window_seconds)
            && self.explanation_mail_limit <= 100
    }

    /// 该类通知在当前快照下是否允许入队。
    pub const fn allows(self, class: MailSendClass) -> bool {
        self.send_classes.allows(class)
    }
}

impl Default for MailReplyPolicy {
    fn default() -> Self {
        Self {
            account_reply_window_seconds: 86_400,
            explanation_mail_window_seconds: 86_400,
            explanation_mail_limit: 10,
            send_classes: MailSendClasses::default(),
        }
    }
}

/// 只读策略来源；实现方必须返回一份完整、已校验的快照。
pub trait MailReplyPolicySource: Debug + Send + Sync {
    fn current(&self) -> MailReplyPolicy;
}

#[cfg(test)]
mod tests {
    use super::{MailSendClass, MailSendClasses};

    #[test]
    fn delivery_projection_covers_every_registered_template() {
        let cases: [(&str, Option<&str>, Option<MailSendClass>); 13] = [
            ("mail_intake", Some("register_key"), Some(MailSendClass::AccountReply)),
            ("mail_intake", Some("rebind_result"), Some(MailSendClass::AccountReply)),
            ("mail_intake", Some("user_assets_rejected"), Some(MailSendClass::AccountReply)),
            ("mail_intake", Some("forum_auction_notice"), Some(MailSendClass::AccountReply)),
            ("mail_intake", Some("auction_equipment_rejected"), Some(MailSendClass::IntakeReply)),
            ("mail_intake", Some("auction_submission_receipt"), Some(MailSendClass::IntakeReply)),
            (
                "settlement_recovery_action",
                Some("recovery_payment_reminder"),
                Some(MailSendClass::SettlementNotice),
            ),
            // 结算履约动作邮件不参与分类，任何开关都不能暂停它们。
            ("settlement", Some("settlement_seller_payout"), None),
            ("settlement", Some("settlement_buyer_refund"), None),
            ("lot_watch", Some("lot_watch"), Some(MailSendClass::OutbidReminder)),
            ("seller_summary", Some("auction_seller_summary"), Some(MailSendClass::PlayerSummary)),
            ("season_reminder", Some("isekai_season_reminder"), Some(MailSendClass::PlayerSummary)),
            ("operations_alert", Some("operations_alert"), Some(MailSendClass::OperationsAlert)),
        ];
        for (origin, template, expected) in cases {
            assert_eq!(MailSendClass::from_delivery(origin, template), expected);
        }
        // 未登记来源不分类，也不被任何开关抑制。
        assert_eq!(MailSendClass::from_delivery("unknown", Some("custom")), None);
        assert!(MailSendClasses::default().allows_delivery("unknown", None));
    }

    #[test]
    fn suppressed_patterns_cover_the_closed_class_map() {
        // 关闭账号回信只排除 mail_intake 的账号模板，投稿回执与结算通知不受影响。
        let classes = MailSendClasses { account_reply: false, ..MailSendClasses::default() };
        let (origins, templates) = classes.suppressed_delivery_patterns();
        assert_eq!(origins.len(), 7);
        assert!(origins.iter().all(|origin| *origin == "mail_intake"));
        assert!(templates.contains(&"rekey_key"));
        assert!(!templates.contains(&"auction_submission_receipt"));
        // 结算通知只排除恢复付款提醒；COD 与退款等履约动作邮件不被排除。
        let classes = MailSendClasses { settlement_notice: false, ..MailSendClasses::default() };
        let (origins, templates) = classes.suppressed_delivery_patterns();
        assert_eq!(origins, vec!["settlement_recovery_action"]);
        assert_eq!(templates, vec!["recovery_payment_reminder"]);
        assert!(
            !classes
                .allows_delivery("settlement_recovery_action", Some("recovery_payment_reminder"))
        );
        assert!(classes.allows_delivery("settlement", Some("settlement_buyer_refund")));
        // 全部开启时不产生任何排除。
        let (origins, templates) = MailSendClasses::default().suppressed_delivery_patterns();
        assert!(origins.is_empty() && templates.is_empty());
    }

    #[test]
    fn a_disabled_class_suppresses_only_itself() {
        let classes = MailSendClasses { account_reply: false, ..MailSendClasses::default() };
        assert!(!classes.allows(MailSendClass::AccountReply));
        assert!(classes.allows(MailSendClass::IntakeReply));
        assert!(!classes.allows_delivery("mail_intake", Some("rekey_key")));
        assert!(classes.allows_delivery("mail_intake", Some("auction_no_attachments")));
    }
}
