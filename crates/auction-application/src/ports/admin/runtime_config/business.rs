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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeEquipmentFilterConfig {
    /// 是否启用前后缀、分类、类型和等级过滤；品质门槛始终启用。
    pub enabled: bool,
    /// 硬性允许品质；空数组回退默认 Legendary/Peerless。
    pub qualities: Vec<String>,
    pub prefixes: Vec<String>,
    pub suffixes: Vec<String>,
    pub categories: Vec<String>,
    pub equipment_types: Vec<String>,
    pub level: AdminRuntimeEquipmentLevelConfig,
}

impl Default for AdminRuntimeEquipmentFilterConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            qualities: crate::default_accepted_qualities(),
            prefixes: Vec::new(),
            suffixes: Vec::new(),
            categories: Vec::new(),
            equipment_types: Vec::new(),
            level: AdminRuntimeEquipmentLevelConfig::default(),
        }
    }
}

/// 单条论坛名称标色规则；类型、种类、品质各维度内多选为 OR，维度间为 AND。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeEquipmentColorRule {
    /// 兼容旧单值装备类型；不得与非空 `categories` 同时使用。
    pub category: String,
    /// 兼容旧单值装备种类；不得与非空 `equipment_types` 同时使用。
    pub equipment_type: String,
    /// 兼容旧单值品质；不得与非空 `qualities` 同时使用。
    pub quality: String,
    /// 装备类型多选；空数组沿用旧单值，二者均空表示所有类型。
    /// 空数组不编码，保持旧文档 fingerprint 与 durable 命令摘要。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
    /// 装备种类多选，支持十二种护甲材质族或精确完整种类；空数组沿用旧单值。
    /// 材质族须同时匹配实际装备大类和完整种类首词，二者均空表示所有种类。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub equipment_types: Vec<String>,
    /// 品质多选；空数组沿用旧单值，二者均空表示所有品质。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub qualities: Vec<String>,
    pub level: AdminRuntimeEquipmentLevelConfig,
    pub prefixes: Vec<String>,
    pub suffixes: Vec<String>,
    pub prefix_color: String,
    pub suffix_color: String,
    pub combined_color: String,
}

impl Default for AdminRuntimeEquipmentColorRule {
    fn default() -> Self {
        Self {
            category: String::new(),
            equipment_type: String::new(),
            quality: String::new(),
            categories: Vec::new(),
            equipment_types: Vec::new(),
            qualities: Vec::new(),
            level: AdminRuntimeEquipmentLevelConfig::default(),
            prefixes: Vec::new(),
            suffixes: Vec::new(),
            prefix_color: "#ff0000".to_owned(),
            suffix_color: "#00aa00".to_owned(),
            combined_color: "#aa00aa".to_owned(),
        }
    }
}

/// Admin 可编辑、Public 分组与 Worker 论坛渲染在启动时冻结的装备标色规则集合。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeEquipmentColorsConfig {
    pub rules: Vec<AdminRuntimeEquipmentColorRule>,
}

impl AdminRuntimeEquipmentColorsConfig {
    /// 转换为公开装备分组与论坛渲染共用的窄规则类型。
    pub fn to_rules(&self) -> Vec<crate::EquipmentColorRule> {
        self.rules
            .iter()
            .map(|rule| crate::EquipmentColorRule {
                category: rule.category.clone(),
                equipment_type: rule.equipment_type.clone(),
                quality: rule.quality.clone(),
                categories: rule.categories.clone(),
                equipment_types: rule.equipment_types.clone(),
                qualities: rule.qualities.clone(),
                level_min: rule.level.min,
                level_max: rule.level.max,
                prefixes: rule.prefixes.clone(),
                suffixes: rule.suffixes.clone(),
                prefix_color: rule.prefix_color.clone(),
                suffix_color: rule.suffix_color.clone(),
                combined_color: rule.combined_color.clone(),
            })
            .collect()
    }
}

/// Admin 可编辑、Worker 启动时冻结的 MoogleMail 通知模板。
/// 字段闭集和默认正文与 Worker 部署配置保持一致；模板正文在应用边界严格校验
/// 占位符，不能通过自由键值表引入未审计的消费者。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeMailTemplatesConfig {
    pub auction_assets_rejected: String,
    pub auction_no_attachments: String,
    /// 原因至少保留 `{reason_code}` 或 `{reason_text}`；另支持 `{next_steps}`、`{mail_reference}`。
    pub auction_equipment_rejected: String,
    pub user_assets_rejected: String,
    pub register_key: String,
    pub register_result: String,
    pub rebind_result: String,
    /// 支持可选 `{price_breakdown}`；旧载荷没有冻结结算明细时该块为空。
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
