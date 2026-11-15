//! 入站处理产生的投递操作、通知模板与限额。

use auction_domain::CommandId;
use chrono::{DateTime, SecondsFormat, Utc};
use uuid::Uuid;

use super::filter::{
    EQUIPMENT_ATTRIBUTES_INVALID, EQUIPMENT_CATEGORY_NOT_ALLOWED, EQUIPMENT_LEVEL_NOT_ALLOWED,
    EQUIPMENT_PREFIX_NOT_ALLOWED, EQUIPMENT_QUALITY_NOT_ALLOWED, EQUIPMENT_SUFFIX_NOT_ALLOWED,
    EQUIPMENT_TRADEABLE_REQUIRED, EQUIPMENT_TYPE_NOT_ALLOWED,
};
use crate::ports::auction::ApplicationFuture;
use crate::ports::core::RequestHash;

/// Delivery 操作闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailDeliveryOperation {
    Send,
    Return,
    Take,
}

impl MailDeliveryOperation {
    /// 返回稳定的 outbox 操作名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Send => "send",
            Self::Return => "return",
            Self::Take => "take",
        }
    }
}

/// MoogleMail delivery 的业务 owner 闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailDeliveryOrigin {
    MailIntake,
    SettlementRecoveryAction,
    OperationsAlert,
    LotWatch,
    /// 卖方成交与流拍汇总；每场每卖方一封，正文在结算事实后冻结。
    SellerSummary,
    /// 换季前 isekai 领取与交付提醒；边界取自已冻结的赛季身份。
    SeasonReminder,
}

impl MailDeliveryOrigin {
    /// 返回 deliveries.origin_kind 使用的稳定名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MailIntake => "mail_intake",
            Self::SettlementRecoveryAction => "settlement_recovery_action",
            Self::OperationsAlert => "operations_alert",
            Self::LotWatch => "lot_watch",
            Self::SellerSummary => "seller_summary",
            Self::SeasonReminder => "season_reminder",
        }
    }
}

/// MoogleMail 通知模板闭集；正文不能由入站邮件任意注入。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailNotificationTemplate {
    AuctionAssetsRejected,
    AuctionNoAttachments,
    AuctionEquipmentRejected,
    /// 投稿接收回执：整封投稿按原邮件合并，正文在录入事务完成后冻结。
    AuctionSubmissionReceipt,
    /// 卖方成交与流拍汇总：正文在结算事实确定后冻结，每场每卖方一封。
    AuctionSellerSummary,
    /// 换季前 isekai 领取与交付提醒：正文绑定已冻结的赛季边界。
    IsekaiSeasonReminder,
    UserAssetsRejected,
    RegisterKey,
    RegisterResult,
    RebindResult,
    RekeyKey,
    RekeyResult,
    ForumAuctionNotice,
    RecoveryPaymentReminder,
    OperationsAlert,
    LotWatch,
}

impl MailNotificationTemplate {
    /// 全部模板的稳定顺序；通知目录测试据此核对文档与模板注册一致。
    pub const ALL: [Self; 16] = [
        Self::AuctionAssetsRejected,
        Self::AuctionNoAttachments,
        Self::AuctionEquipmentRejected,
        Self::AuctionSubmissionReceipt,
        Self::AuctionSellerSummary,
        Self::IsekaiSeasonReminder,
        Self::UserAssetsRejected,
        Self::RegisterKey,
        Self::RegisterResult,
        Self::RebindResult,
        Self::RekeyKey,
        Self::RekeyResult,
        Self::ForumAuctionNotice,
        Self::RecoveryPaymentReminder,
        Self::OperationsAlert,
        Self::LotWatch,
    ];

    /// 返回稳定的模板名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AuctionAssetsRejected => "auction_assets_rejected",
            Self::AuctionNoAttachments => "auction_no_attachments",
            Self::AuctionEquipmentRejected => "auction_equipment_rejected",
            Self::AuctionSubmissionReceipt => "auction_submission_receipt",
            Self::AuctionSellerSummary => "auction_seller_summary",
            Self::IsekaiSeasonReminder => "isekai_season_reminder",
            Self::UserAssetsRejected => "user_assets_rejected",
            Self::RegisterKey => "register_key",
            Self::RegisterResult => "register_result",
            Self::RebindResult => "rebind_result",
            Self::RekeyKey => "rekey_key",
            Self::RekeyResult => "rekey_result",
            Self::ForumAuctionNotice => "forum_auction_notice",
            Self::RecoveryPaymentReminder => "recovery_payment_reminder",
            Self::OperationsAlert => "operations_alert",
            Self::LotWatch => "lot_watch",
        }
    }

    /// 解析 payload 中冻结的稳定模板名；未知值返回 `None`，由调用方失败关闭。
    pub fn from_name(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|template| template.as_str() == value)
    }

    /// 模板的投递约束。入队校验、资产冲突排除与发送前解码都只查这张表，新增或调整模板只改
    /// 这里；任一处单独放宽或收紧，合法通知就会在入队处被拒（卡住入站邮箱）或在发送前判为
    /// 无效载荷。结果码闭集以发送端能渲染的集合为准，生产者只能使用闭集内的值。
    pub const fn binding(self) -> MailTemplateBinding {
        match self {
            Self::AuctionAssetsRejected => MailTemplateBinding::plain(
                MailDeliveryOrigin::MailIntake,
                &["auction_assets_rejected"],
            ),
            Self::AuctionNoAttachments => MailTemplateBinding::plain(
                MailDeliveryOrigin::MailIntake,
                &["auction_no_attachments"],
            ),
            Self::AuctionEquipmentRejected => MailTemplateBinding::plain(
                MailDeliveryOrigin::MailIntake,
                EQUIPMENT_REJECTION_RESULT_CODES,
            ),
            Self::AuctionSubmissionReceipt => MailTemplateBinding {
                body: MailTemplateField::Required,
                ..MailTemplateBinding::plain(MailDeliveryOrigin::MailIntake, &["accepted"])
            },
            Self::AuctionSellerSummary => MailTemplateBinding {
                body: MailTemplateField::Required,
                world: Some("main"),
                ..MailTemplateBinding::plain(MailDeliveryOrigin::SellerSummary, &["summary"])
            },
            Self::IsekaiSeasonReminder => MailTemplateBinding {
                body: MailTemplateField::Required,
                world: Some("isekai"),
                ..MailTemplateBinding::plain(MailDeliveryOrigin::SeasonReminder, &["season_end"])
            },
            Self::UserAssetsRejected => MailTemplateBinding::plain(
                MailDeliveryOrigin::MailIntake,
                &["user_assets_rejected"],
            ),
            Self::RegisterKey => MailTemplateBinding {
                user_key: MailTemplateField::Required,
                ..MailTemplateBinding::plain(
                    MailDeliveryOrigin::MailIntake,
                    &["registered", "recovered"],
                )
            },
            Self::RegisterResult => MailTemplateBinding::plain(
                MailDeliveryOrigin::MailIntake,
                &["blacklisted", "identity_conflict"],
            ),
            Self::RebindResult => MailTemplateBinding::plain(
                MailDeliveryOrigin::MailIntake,
                &[
                    "rebound",
                    "unchanged",
                    "blacklisted",
                    "identity_conflict",
                    "invalid_key",
                    "missing_key",
                ],
            ),
            Self::RekeyKey => MailTemplateBinding {
                user_key: MailTemplateField::Required,
                ..MailTemplateBinding::plain(MailDeliveryOrigin::MailIntake, &["rekeyed"])
            },
            Self::RekeyResult => MailTemplateBinding::plain(
                MailDeliveryOrigin::MailIntake,
                &["blacklisted", "identity_conflict", "not_registered"],
            ),
            // 已无生产者，发送前解码同样拒绝；只为资产冲突排除识别历史行保留形状。
            Self::ForumAuctionNotice => MailTemplateBinding {
                retired: true,
                ..MailTemplateBinding::plain(
                    MailDeliveryOrigin::MailIntake,
                    &["forum_auction_notice"],
                )
            },
            Self::RecoveryPaymentReminder => MailTemplateBinding {
                world: Some("main"),
                ..MailTemplateBinding::plain(
                    MailDeliveryOrigin::SettlementRecoveryAction,
                    &["payment_overdue"],
                )
            },
            Self::OperationsAlert => MailTemplateBinding {
                body: MailTemplateField::Required,
                ..MailTemplateBinding::plain(
                    MailDeliveryOrigin::OperationsAlert,
                    &["operations_alert"],
                )
            },
            Self::LotWatch => MailTemplateBinding {
                body: MailTemplateField::Required,
                ..MailTemplateBinding::plain(MailDeliveryOrigin::LotWatch, &["outbid"])
            },
        }
    }
}

/// `auction_equipment_rejected` 的结果码闭集：入站处理器的全部拒收原因，发送端为每一项准备了
/// 固定说明文案。只追加不删除：已入队的历史结果码仍需被发送前解码与资产冲突排除识别。
pub const EQUIPMENT_REJECTION_RESULT_CODES: &[&str] = &[
    "auction_equipment_rejected",
    "start_price_invalid",
    // 已不再产生，保留给历史投递。
    "start_price_missing",
    "start_price_duplicate",
    "start_price_identity_mismatch",
    "start_price_attachment_not_found",
    EQUIPMENT_QUALITY_NOT_ALLOWED,
    EQUIPMENT_PREFIX_NOT_ALLOWED,
    EQUIPMENT_SUFFIX_NOT_ALLOWED,
    EQUIPMENT_CATEGORY_NOT_ALLOWED,
    EQUIPMENT_TYPE_NOT_ALLOWED,
    EQUIPMENT_LEVEL_NOT_ALLOWED,
    "auction_equipment_conflict",
    EQUIPMENT_ATTRIBUTES_INVALID,
    "season_intake_closed",
    EQUIPMENT_TRADEABLE_REQUIRED,
];

/// 通知模板对一个可选载荷字段的要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailTemplateField {
    /// 必须出现：登录 key 引用是非 nil 用户 ID；冻结正文满足 [`valid_notification_body`]。
    Required,
    /// 必须省略。
    Forbidden,
}

impl MailTemplateField {
    fn admits_user_key(self, user_key_user_id: Option<Uuid>) -> bool {
        match self {
            Self::Required => user_key_user_id.is_some_and(|user_id| !user_id.is_nil()),
            Self::Forbidden => user_key_user_id.is_none(),
        }
    }

    fn admits_body(self, body: Option<&str>) -> bool {
        match self {
            Self::Required => valid_notification_body(body),
            Self::Forbidden => body.is_none(),
        }
    }
}

/// 通知模板的投递约束，见 [`MailNotificationTemplate::binding`]。
///
/// 只约束与模板绑定的字段；`origin_id` 形状、`external_mail_id` 前缀、零资产等来源与操作规则
/// 仍由各调用方按来源核对。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailTemplateBinding {
    /// 唯一可以使用该模板的 delivery 来源。
    pub origin: MailDeliveryOrigin,
    /// `result_code` 闭集。
    pub result_codes: &'static [&'static str],
    /// 登录 key 引用 `user_key_user_id`；只有寄送登录 key 的模板需要。
    pub user_key: MailTemplateField,
    /// 冻结正文；只有正文在创建时冻结的模板携带。
    pub body: MailTemplateField,
    /// 固定投递世界；`None` 表示由来源决定 main 或 isekai。
    pub world: Option<&'static str>,
    /// 已无生产者、只为历史行保留形状的模板：入队与发送拒绝，资产冲突排除仍按原形状识别。
    pub retired: bool,
}

impl MailTemplateBinding {
    /// 在用、不带 key 与冻结正文、不限世界的模板约束。
    const fn plain(origin: MailDeliveryOrigin, result_codes: &'static [&'static str]) -> Self {
        Self {
            origin,
            result_codes,
            user_key: MailTemplateField::Forbidden,
            body: MailTemplateField::Forbidden,
            world: None,
            retired: false,
        }
    }

    /// 入队与发送前解码使用：已退役模板一律拒绝，其余字段按 [`Self::admits_history`] 核对。
    pub fn admits(&self, fields: MailNotificationFields<'_>) -> bool {
        !self.retired && self.admits_history(fields)
    }

    /// 资产冲突排除等只读历史识别使用：已退役模板的旧行仍按原形状识别为零资产通知。
    pub fn admits_history(&self, fields: MailNotificationFields<'_>) -> bool {
        self.origin.as_str() == fields.origin
            && self.world.is_none_or(|world| world == fields.world)
            && fields.result_code.is_some_and(|code| self.result_codes.contains(&code))
            && self.user_key.admits_user_key(fields.user_key_user_id)
            && self.body.admits_body(fields.body)
    }
}

/// 一封通知中由模板约束的字段；调用方从命令或已解码载荷投影后交给 [`MailTemplateBinding`]。
#[derive(Debug, Clone, Copy)]
pub struct MailNotificationFields<'a> {
    /// `deliveries.origin_kind`。
    pub origin: &'a str,
    pub world: &'a str,
    pub result_code: Option<&'a str>,
    pub user_key_user_id: Option<Uuid>,
    /// 冻结正文（命令的 `notification_body`、载荷的 `body`）。
    pub body: Option<&'a str>,
}

/// 由业务 owner 生成的无密文 MoogleMail delivery 命令。
#[derive(Debug, Clone)]
pub struct MailDeliveryCommand {
    pub command_id: CommandId,
    pub request_hash: RequestHash,
    pub origin: MailDeliveryOrigin,
    pub origin_id: String,
    pub operation: MailDeliveryOperation,
    pub world_id: String,
    pub external_mail_id: String,
    pub recipient: String,
    pub template: Option<MailNotificationTemplate>,
    pub result_code: Option<String>,
    /// 发送登录 key 的通知使用的非 nil 用户 ID 引用；哪些模板必需由
    /// [`MailNotificationTemplate::binding`] 决定，其他模板必须省略。
    pub user_key_user_id: Option<Uuid>,
    pub credits: i64,
    pub cod: i64,
    pub attachment_ids: Vec<String>,
    /// 创建时冻结的通知正文；哪些模板必需由 [`MailNotificationTemplate::binding`] 决定，
    /// 其他 delivery 必须省略。
    pub notification_body: Option<String>,
}

/// 冻结通知正文的格式：非空、无首尾空白、不超过 16 KiB，控制字符只允许换行与制表；
/// 入队、存储重放核对与发送前校验共用同一规则。
pub fn valid_notification_body(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        !value.is_empty()
            && value.len() <= (16 << 10)
            && value == value.trim()
            && !value
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    })
}

/// 换季提醒 `external_mail_id` 的固定前缀；其后是冻结的赛季边界。
const SEASON_REMINDER_MAIL_ID_PREFIX: &str = "season-end:";

/// 换季提醒的 `external_mail_id`：固定前缀加秒精度 UTC RFC 3339 的冻结赛季边界。
/// 发送围栏与预检退役的 SQL 按这一写法解析边界。
pub fn season_reminder_mail_id(boundary: DateTime<Utc>) -> String {
    format!(
        "{SEASON_REMINDER_MAIL_ID_PREFIX}{}",
        boundary.to_rfc3339_opts(SecondsFormat::Secs, true)
    )
}

/// [`season_reminder_mail_id`] 的唯一规范校验；入队、存储核对、资产冲突排除与发送前解码共用。
/// 其他时区、小数秒或小写分隔符等写法与 SQL 解析口径不一致，一律失败关闭。
pub fn valid_season_reminder_mail_id(value: &str) -> bool {
    value.strip_prefix(SEASON_REMINDER_MAIL_ID_PREFIX).is_some_and(|boundary| {
        DateTime::parse_from_rfc3339(boundary)
            .is_ok_and(|parsed| season_reminder_mail_id(parsed.with_timezone(&Utc)) == value)
    })
}

/// MoogleMail delivery 入队后的安全结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailDeliveryResult {
    pub delivery_id: Uuid,
}

/// 说明信模板闭集：拒收说明、换绑结果与论坛回执共用同一发件人预算。
pub const EXPLANATION_NOTICE_TEMPLATES: &[&str] = &[
    "auction_assets_rejected",
    "auction_no_attachments",
    "auction_equipment_rejected",
    "user_assets_rejected",
    "rebind_result",
    "forum_auction_notice",
];

/// 按发件人窗口建立说明信预算；`window_seconds` 由调用方按配置校验后传入。
pub fn explanation_notice_limit(
    window_seconds: u64,
    maximum: u32,
    now: DateTime<Utc>,
) -> MailNoticeLimit {
    let seconds = i64::try_from(window_seconds).unwrap_or(604_800).clamp(60, 604_800);
    MailNoticeLimit {
        templates: EXPLANATION_NOTICE_TEMPLATES,
        since: now - chrono::Duration::seconds(seconds),
        maximum,
    }
}

/// 回信预算：在 `since` 之后该收件人已入队的 `templates` 数量达到 `maximum` 时不再入队。
/// 模板集合由应用层决定；storage 只按 payload 中冻结的模板名计数，因此不复制业务策略。
#[derive(Debug, Clone, Copy)]
pub struct MailNoticeLimit {
    pub templates: &'static [&'static str],
    pub since: DateTime<Utc>,
    /// 0 表示不限制。
    pub maximum: u32,
}

impl MailNoticeLimit {
    pub fn counts(self, template: MailNotificationTemplate) -> bool {
        self.templates.contains(&template.as_str())
    }
}

/// 受预算约束的入队结果；被抑制时不产生 delivery 行，由调用方在命令与审计中记录原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailNoticeEnqueue {
    Enqueued(MailDeliveryResult),
    Suppressed,
}

/// recovery delivery 激活时使用的稳定身份。
/// prepare 属于 delivery 行本身的幂等身份；activate 属于 recovery action 的第二个本地事务
/// 边界。两者分开保存，才能在 delivery 已经进入 pending 而 action 尚未写回时安全重放。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailDeliveryActivation {
    pub delivery_id: Uuid,
    pub prepare_command_id: CommandId,
    pub prepare_request_hash: RequestHash,
    pub activate_command_id: CommandId,
    pub activate_request_hash: RequestHash,
    pub activated_at: DateTime<Utc>,
}

/// MoogleMail delivery outbox 入队端口。
pub trait MailDeliveryEnqueuer: Send + Sync {
    fn enqueue(&self, command: MailDeliveryCommand) -> ApplicationFuture<'_, MailDeliveryResult>;

    /// 在同一 Serializable 事务内先计数再入队；达到预算时返回 [`MailNoticeEnqueue::Suppressed`]。
    /// 计数按收件人不区分 ASCII 大小写与 payload 中冻结的模板名，和入队必须同事务完成，
    /// 避免并发入站邮件在同一窗口内一起越过上限。
    fn enqueue_within_limit(
        &self,
        command: MailDeliveryCommand,
        limit: MailNoticeLimit,
    ) -> ApplicationFuture<'_, MailNoticeEnqueue>;

    /// 只读恢复旧版已准备但尚未关联 action 的同邮件投递；完整负载不符必须失败关闭。
    /// 返回 None 只表示该旧命令尚不存在，或已绑定另一次退件，调用者才能准备新轮次。
    /// 未装配查询能力的实现明确拒绝，不能把未知当作原投递不存在。
    fn find_recovery_preparation(
        &self,
        _command: MailDeliveryCommand,
    ) -> ApplicationFuture<'_, Option<MailDeliveryResult>> {
        Box::pin(async {
            Err(crate::errors::ApplicationError::CapabilityUnavailable(
                "recovery preparation lookup",
            ))
        })
    }

    /// 以 prepared 状态写入 recovery delivery；普通通知仍直接进入 pending。
    fn prepare_recovery(
        &self,
        command: MailDeliveryCommand,
    ) -> ApplicationFuture<'_, MailDeliveryResult>;

    /// 将已校验的 recovery delivery 从 prepared 激活为可领取的 pending。
    fn activate_recovery(
        &self,
        activation: MailDeliveryActivation,
    ) -> ApplicationFuture<'_, MailDeliveryResult>;
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use chrono::{TimeZone, Utc};
    use uuid::Uuid;

    use super::{
        EQUIPMENT_REJECTION_RESULT_CODES, MailDeliveryOrigin, MailNotificationFields,
        MailNotificationTemplate, MailTemplateField, season_reminder_mail_id,
        valid_season_reminder_mail_id,
    };

    const ORIGINS: [MailDeliveryOrigin; 6] = [
        MailDeliveryOrigin::MailIntake,
        MailDeliveryOrigin::SettlementRecoveryAction,
        MailDeliveryOrigin::OperationsAlert,
        MailDeliveryOrigin::LotWatch,
        MailDeliveryOrigin::SellerSummary,
        MailDeliveryOrigin::SeasonReminder,
    ];

    /// 按模板表生成的合法字段；`code` 取自该模板的结果码闭集。
    fn canonical(template: MailNotificationTemplate, code: &str) -> MailNotificationFields<'_> {
        let binding = template.binding();
        MailNotificationFields {
            origin: binding.origin.as_str(),
            world: binding.world.unwrap_or("main"),
            result_code: Some(code),
            user_key_user_id: (binding.user_key == MailTemplateField::Required)
                .then(|| Uuid::from_u128(7)),
            body: (binding.body == MailTemplateField::Required).then_some("Frozen notice body"),
        }
    }

    #[test]
    fn template_names_round_trip_through_from_name() {
        let names = MailNotificationTemplate::ALL
            .into_iter()
            .map(MailNotificationTemplate::as_str)
            .collect::<HashSet<_>>();
        assert_eq!(names.len(), MailNotificationTemplate::ALL.len(), "模板名不得重复");
        for template in MailNotificationTemplate::ALL {
            assert_eq!(MailNotificationTemplate::from_name(template.as_str()), Some(template));
        }
        for unknown in ["", "LOT_WATCH", "lot_watch ", "settlement_payment_reminder"] {
            assert_eq!(MailNotificationTemplate::from_name(unknown), None, "{unknown}");
        }
    }

    /// 每个模板只接受表中的来源、结果码、key 引用、冻结正文与固定世界；任一字段越界都拒绝。
    #[test]
    fn binding_admits_exactly_the_bound_shape() {
        for template in MailNotificationTemplate::ALL {
            let binding = template.binding();
            let codes = binding.result_codes.iter().copied().collect::<HashSet<_>>();
            assert!(!codes.is_empty(), "{template:?} 必须有结果码闭集");
            assert_eq!(codes.len(), binding.result_codes.len(), "{template:?} 结果码重复");
            for code in binding.result_codes {
                let fields = canonical(template, code);
                assert!(binding.admits_history(fields), "{template:?}/{code}");
                assert_eq!(binding.admits(fields), !binding.retired, "{template:?}/{code}");
            }
            let valid = canonical(template, binding.result_codes[0]);
            let mut invalid = vec![
                MailNotificationFields { result_code: None, ..valid },
                MailNotificationFields { result_code: Some("unknown_result_code"), ..valid },
            ];
            invalid.extend(
                ORIGINS
                    .into_iter()
                    .filter(|origin| *origin != binding.origin)
                    .map(|origin| MailNotificationFields { origin: origin.as_str(), ..valid }),
            );
            match binding.user_key {
                MailTemplateField::Required => invalid.extend([
                    MailNotificationFields { user_key_user_id: None, ..valid },
                    MailNotificationFields { user_key_user_id: Some(Uuid::nil()), ..valid },
                ]),
                MailTemplateField::Forbidden => invalid.push(MailNotificationFields {
                    user_key_user_id: Some(Uuid::from_u128(7)),
                    ..valid
                }),
            }
            match binding.body {
                MailTemplateField::Required => invalid.extend([
                    MailNotificationFields { body: None, ..valid },
                    MailNotificationFields { body: Some(" padded body "), ..valid },
                ]),
                MailTemplateField::Forbidden => {
                    invalid.push(MailNotificationFields { body: Some("Injected body"), ..valid });
                }
            }
            if let Some(world) = binding.world {
                let other = if world == "main" { "isekai" } else { "main" };
                invalid.push(MailNotificationFields { world: other, ..valid });
            }
            for fields in invalid {
                assert!(!binding.admits_history(fields), "{template:?} 必须拒绝 {fields:?}");
                assert!(!binding.admits(fields), "{template:?} 必须拒绝 {fields:?}");
            }
        }
    }

    /// 只有已无生产者的论坛回执是退役模板；其余模板都必须能入队与发送。
    #[test]
    fn only_the_forum_notice_is_retired() {
        let retired = MailNotificationTemplate::ALL
            .into_iter()
            .filter(|template| template.binding().retired)
            .collect::<Vec<_>>();
        assert_eq!(retired, [MailNotificationTemplate::ForumAuctionNotice]);
    }

    #[test]
    fn equipment_rejection_codes_include_the_generic_reason() {
        let codes = EQUIPMENT_REJECTION_RESULT_CODES.iter().copied().collect::<HashSet<_>>();
        assert_eq!(codes.len(), EQUIPMENT_REJECTION_RESULT_CODES.len(), "拒收原因不得重复");
        assert!(codes.contains(MailNotificationTemplate::AuctionEquipmentRejected.as_str()));
        assert!(codes.iter().all(|code| auction_domain::valid_text(code, 256)));
    }

    #[test]
    fn season_reminder_mail_id_accepts_only_its_canonical_form() {
        let boundary = Utc.with_ymd_and_hms(2026, 11, 1, 9, 0, 0).single().expect("合法时刻");
        let canonical = season_reminder_mail_id(boundary);
        assert_eq!(canonical, "season-end:2026-11-01T09:00:00Z");
        assert!(valid_season_reminder_mail_id(&canonical));
        for value in [
            "season-end:fixture",
            "season-end:",
            "season-end:2026-13-45T25:61:61Z",
            "season-end:2026-02-30T09:00:00Z",
            "season-end:2026-11-01T09:00:00+00:00",
            "season-end:2026-11-01T18:00:00+09:00",
            "season-end:2026-11-01T09:00:00.000Z",
            "season-end:2026-11-01 09:00:00Z",
            "season-end:2026-11-01t09:00:00z",
            "2026-11-01T09:00:00Z",
            "Season-End:2026-11-01T09:00:00Z",
        ] {
            assert!(!valid_season_reminder_mail_id(value), "{value}");
        }
    }
}
