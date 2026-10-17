use serde::{Deserialize, Serialize};

/// 单个 MoogleMail world 的可编辑运行策略。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeWorldConfig {
    /// 是否由 Worker 处理该 world；旧版要求 `main`、`isekai` 至少启用一个。
    pub enabled: bool,
    /// 允许读取详情的发件人身份。
    pub sender_allowlist: Vec<String>,
    /// 明确关闭 allowlist 后才允许读取任意发件人的详情。
    pub sender_allowlist_disabled: bool,
}

impl Default for AdminRuntimeWorldConfig {
    fn default() -> Self {
        Self { enabled: true, sender_allowlist: Vec::new(), sender_allowlist_disabled: false }
    }
}

/// Admin 与 Worker 共享的 MoogleMail world 闭集。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeWorldsConfig {
    pub main: AdminRuntimeWorldConfig,
    pub isekai: AdminRuntimeWorldConfig,
}

/// MoogleMail 装备过滤的等级范围。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeEquipmentLevelConfig {
    pub min: i32,
    pub max: i32,
}

/// Admin 可编辑、Worker 启动时冻结的 MoogleMail 装备过滤策略。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeEquipmentFilterConfig {
    pub enabled: bool,
    pub qualities: Vec<String>,
    pub prefixes: Vec<String>,
    pub suffixes: Vec<String>,
    pub categories: Vec<String>,
    pub equipment_types: Vec<String>,
    pub level: AdminRuntimeEquipmentLevelConfig,
}

/// Admin 可编辑、Worker 启动时冻结的 MoogleMail 通知模板。
/// 字段闭集和默认正文逐项保持 Auction-BF runtime 文档契约；模板正文仍在应用边界严格
/// 校验占位符，不能通过自由键值表引入未审计的消费者。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeMailTemplatesConfig {
    pub auction_assets_rejected: String,
    pub auction_no_attachments: String,
    pub auction_equipment_rejected: String,
    pub user_assets_rejected: String,
    pub register_key: String,
    pub register_result: String,
    pub rebind_result: String,
    pub settlement_winner_equipment_cod: String,
    pub settlement_payment_carrier: String,
    pub settlement_payment_reminder: String,
    pub settlement_equipment_handoff: String,
    pub settlement_seller_payout: String,
    pub settlement_buyer_refund: String,
    pub operations_alert: String,
}

impl Default for AdminRuntimeMailTemplatesConfig {
    fn default() -> Self {
        Self {
            auction_assets_rejected: "Auction assets rejected".to_owned(),
            auction_no_attachments: "Auction has no attachments".to_owned(),
            auction_equipment_rejected: "Auction equipment rejected: {reason_code}".to_owned(),
            user_assets_rejected: "User assets rejected".to_owned(),
            register_key: "Registration key {login_key} for {username}".to_owned(),
            register_result: "Registration result {result_code} {username}".to_owned(),
            rebind_result: "Rebind result {result_code} {username}".to_owned(),
            settlement_winner_equipment_cod:
                "Auction item {item_name}; COD {amount}; reference {reference}".to_owned(),
            settlement_payment_carrier: "Auction payment request {amount}; reference {reference}"
                .to_owned(),
            settlement_payment_reminder: "Auction payment is overdue; reference {reference}"
                .to_owned(),
            settlement_equipment_handoff: "Auction item {item_name}; reference {reference}"
                .to_owned(),
            settlement_seller_payout: "Auction seller payout {amount}; reference {reference}"
                .to_owned(),
            settlement_buyer_refund: "Auction buyer refund {amount}; reference {reference}"
                .to_owned(),
            operations_alert: "Operations alert: {summary}".to_owned(),
        }
    }
}
