//! 普通 MoogleMail 投递载荷的统一判定：来源 × 操作组合、规范 UUID、附件装备 ID、
//! `external_mail_id` 与来源身份只在这里定义一次。存储入队校验、MoogleMail 发送前解码与内部
//! 履约的资产冲突排除都调用 [`MailDeliveryShape::check`]；任一处单独放宽或收紧，合法投递就会
//! 在入队处被拒，或入队后才在发送前判为无效载荷。越界的编码与身份一律失败关闭（不变量 6），
//! 不在任何一处静默修正。结算来源的载荷由结算 owner 校验，不经这里。

use std::collections::HashSet;

use thiserror::Error;
use uuid::Uuid;

use super::delivery::{
    MailDeliveryCommand, MailDeliveryOperation, MailDeliveryOrigin, MailNotificationFields,
    MailNotificationTemplate, valid_season_reminder_mail_id,
};

/// 一封投递最多携带的附件装备数。
const MAX_DELIVERY_ATTACHMENTS: usize = 1000;

/// 附件外部装备 ID 的位数上限。
const MAX_ATTACHMENT_ID_DIGITS: usize = 32;

/// `external_mail_id` 的字节上限。
const MAX_EXTERNAL_MAIL_ID_BYTES: usize = 256;

impl MailDeliveryOrigin {
    /// 来源 × 操作组合闭集：入站邮件可寄出、退回与领取；恢复动作只寄付款提醒与领取退件；
    /// 告警、出价被超提醒、卖方汇总与换季提醒只寄出。
    pub(crate) const fn admits_operation(self, operation: MailDeliveryOperation) -> bool {
        match self {
            Self::MailIntake => true,
            Self::SettlementRecoveryAction => {
                matches!(operation, MailDeliveryOperation::Send | MailDeliveryOperation::Take)
            }
            Self::OperationsAlert | Self::LotWatch | Self::SellerSummary | Self::SeasonReminder => {
                matches!(operation, MailDeliveryOperation::Send)
            }
        }
    }
}

/// 普通投递不满足统一判定的方面；只用于诊断，不进入持久化或线上协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum MailDeliveryShapeError {
    #[error("来源不允许该投递操作")]
    Operation,
    #[error("投递世界不在闭集内")]
    World,
    #[error("external_mail_id 不是规范单行文本")]
    ExternalMailId,
    #[error("投递金额或附件越界")]
    Assets,
    #[error("通知字段与模板约束不符")]
    Notification,
    #[error("来源标识与载荷不符")]
    Source,
}

/// 普通投递中由统一判定约束的字段；入队命令与已落库载荷都投影成这一视图。
#[derive(Debug, Clone, Copy)]
pub struct MailDeliveryShape<'a> {
    pub origin: MailDeliveryOrigin,
    pub operation: MailDeliveryOperation,
    /// `deliveries.command_id`；出价被超提醒与卖方汇总的邮件 ID 由它派生。
    pub command_id: Uuid,
    pub origin_id: &'a str,
    pub world: &'a str,
    pub external_mail_id: &'a str,
    pub template: Option<MailNotificationTemplate>,
    pub result_code: Option<&'a str>,
    pub user_key_user_id: Option<Uuid>,
    /// 冻结正文（命令的 `notification_body`、载荷的 `body`）。
    pub body: Option<&'a str>,
    pub credits: i64,
    pub cod: i64,
    pub attachment_ids: &'a [String],
}

impl MailDeliveryShape<'_> {
    /// 全部方面都在统一口径内。
    pub fn admits(&self) -> bool {
        self.check().is_ok()
    }

    /// 逐方面核对，返回第一处越界的方面。
    pub fn check(&self) -> Result<(), MailDeliveryShapeError> {
        if !self.origin.admits_operation(self.operation) {
            return Err(MailDeliveryShapeError::Operation);
        }
        if !matches!(self.world, "main" | "isekai") {
            return Err(MailDeliveryShapeError::World);
        }
        if !auction_domain::valid_text(self.external_mail_id, MAX_EXTERNAL_MAIL_ID_BYTES) {
            return Err(MailDeliveryShapeError::ExternalMailId);
        }
        if !self.assets_admitted() {
            return Err(MailDeliveryShapeError::Assets);
        }
        if !self.notification_admitted() {
            return Err(MailDeliveryShapeError::Notification);
        }
        if !self.source_admitted() {
            return Err(MailDeliveryShapeError::Source);
        }
        Ok(())
    }

    /// 金额非负，附件逐项合法且不重复。寄出不带资产；入站邮件的退回与领取不带金额；恢复领取的
    /// 附件严格升序，与冻结的预期动作逐项比对。
    fn assets_admitted(&self) -> bool {
        // 先判数量上限再建去重集合，越界的历史载荷不按其长度分配内存。
        if self.credits < 0 || self.cod < 0 || self.attachment_ids.len() > MAX_DELIVERY_ATTACHMENTS
        {
            return false;
        }
        let mut seen = HashSet::with_capacity(self.attachment_ids.len());
        if !self
            .attachment_ids
            .iter()
            .all(|value| valid_attachment_id(value) && seen.insert(value.as_str()))
        {
            return false;
        }
        match (self.origin, self.operation) {
            (_, MailDeliveryOperation::Send) => {
                self.credits == 0 && self.cod == 0 && self.attachment_ids.is_empty()
            }
            (MailDeliveryOrigin::MailIntake, _) => self.credits == 0 && self.cod == 0,
            (MailDeliveryOrigin::SettlementRecoveryAction, MailDeliveryOperation::Take) => {
                self.attachment_ids.windows(2).all(|pair| pair[0] < pair[1])
            }
            // 组合闭集之外的退回与领取已在 `check` 开头拒绝；这里同样失败关闭，不放行资产。
            _ => false,
        }
    }

    /// 寄出只按模板表核对；退回与领取不带模板、结果码、登录 key 引用或正文。
    fn notification_admitted(&self) -> bool {
        match self.operation {
            MailDeliveryOperation::Send => self.template.is_some_and(|template| {
                template.binding().admits(MailNotificationFields {
                    origin: self.origin.as_str(),
                    world: self.world,
                    result_code: self.result_code,
                    user_key_user_id: self.user_key_user_id,
                    body: self.body,
                })
            }),
            MailDeliveryOperation::Return | MailDeliveryOperation::Take => {
                self.template.is_none()
                    && self.result_code.is_none()
                    && self.user_key_user_id.is_none()
                    && self.body.is_none()
            }
        }
    }

    /// `origin_id` 与 `external_mail_id` 的来源身份；内嵌 UUID 一律是规范文本。
    fn source_admitted(&self) -> bool {
        match self.origin {
            MailDeliveryOrigin::MailIntake => {
                // 历史 producer 误把命令用途写进 origin_id；兼容仅限同一 world、邮件 ID 与操作
                // 的逐项别名，未知后缀失败关闭。
                let legacy = match self.operation {
                    MailDeliveryOperation::Send => "delivery-notice",
                    MailDeliveryOperation::Return => "delivery-return",
                    MailDeliveryOperation::Take => "delivery-take",
                };
                let prefix = format!("{}:{}:", self.world, self.external_mail_id);
                self.origin_id
                    .strip_prefix(prefix.as_str())
                    .is_some_and(|suffix| suffix == self.operation.as_str() || suffix == legacy)
            }
            MailDeliveryOrigin::SettlementRecoveryAction => {
                self.origin_id.strip_prefix("recovery:").is_some_and(canonical_uuid)
            }
            MailDeliveryOrigin::OperationsAlert => {
                self.external_mail_id.strip_prefix("operations-alert:").is_some_and(|batch_id| {
                    canonical_uuid(batch_id)
                        && self.origin_id.strip_prefix("operations_alert:") == Some(batch_id)
                })
            }
            MailDeliveryOrigin::LotWatch => {
                self.mail_id_derived_from_command()
                    && self.origin_id.strip_prefix("lot_watch:") == Some(self.external_mail_id)
            }
            MailDeliveryOrigin::SellerSummary => {
                self.mail_id_derived_from_command() && self.origin_id.starts_with("seller_summary:")
            }
            MailDeliveryOrigin::SeasonReminder => {
                self.origin_id.starts_with("season_reminder:")
                    && valid_season_reminder_mail_id(self.external_mail_id)
            }
        }
    }

    /// 邮件 ID 是非 nil 命令 ID 的规范文本。
    fn mail_id_derived_from_command(&self) -> bool {
        !self.command_id.is_nil() && self.external_mail_id == self.command_id.to_string()
    }
}

impl MailDeliveryCommand {
    /// 入队命令投影到统一判定；存储入队校验与各生产者的自检用同一视图。
    pub fn shape(&self) -> MailDeliveryShape<'_> {
        MailDeliveryShape {
            origin: self.origin,
            operation: self.operation,
            command_id: self.command_id.as_uuid(),
            origin_id: &self.origin_id,
            world: &self.world_id,
            external_mail_id: &self.external_mail_id,
            template: self.template,
            result_code: self.result_code.as_deref(),
            user_key_user_id: self.user_key_user_id,
            body: self.notification_body.as_deref(),
            credits: self.credits,
            cod: self.cod,
            attachment_ids: &self.attachment_ids,
        }
    }
}

/// 规范 UUID 文本：小写连字符形式且非 nil；大写、无连字符、花括号与 urn 写法一律拒绝。
fn canonical_uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|parsed| !parsed.is_nil() && parsed.to_string() == value)
}

/// 附件外部装备 ID：1–32 位十进制且不全为零。
fn valid_attachment_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ATTACHMENT_ID_DIGITS
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && value.bytes().any(|byte| byte != b'0')
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{MailDeliveryShape, MailDeliveryShapeError, canonical_uuid, valid_attachment_id};
    use crate::ports::mail::{MailDeliveryOperation, MailDeliveryOrigin, MailNotificationTemplate};

    /// 来源 × 操作组合闭集逐项列出；新增来源或操作必须在这里显式决定可否组合。
    #[test]
    fn origin_operation_closed_set_is_exact() {
        let operations = [
            MailDeliveryOperation::Send,
            MailDeliveryOperation::Return,
            MailDeliveryOperation::Take,
        ];
        let admitted = MailDeliveryOrigin::ALL
            .into_iter()
            .flat_map(|origin| operations.map(|operation| (origin, operation)))
            .filter(|(origin, operation)| origin.admits_operation(*operation))
            .collect::<Vec<_>>();
        assert_eq!(
            admitted,
            [
                (MailDeliveryOrigin::MailIntake, MailDeliveryOperation::Send),
                (MailDeliveryOrigin::MailIntake, MailDeliveryOperation::Return),
                (MailDeliveryOrigin::MailIntake, MailDeliveryOperation::Take),
                (MailDeliveryOrigin::SettlementRecoveryAction, MailDeliveryOperation::Send),
                (MailDeliveryOrigin::SettlementRecoveryAction, MailDeliveryOperation::Take),
                (MailDeliveryOrigin::OperationsAlert, MailDeliveryOperation::Send),
                (MailDeliveryOrigin::LotWatch, MailDeliveryOperation::Send),
                (MailDeliveryOrigin::SellerSummary, MailDeliveryOperation::Send),
                (MailDeliveryOrigin::SeasonReminder, MailDeliveryOperation::Send),
            ]
        );
        for origin in MailDeliveryOrigin::ALL {
            assert_eq!(MailDeliveryOrigin::from_name(origin.as_str()), Some(origin));
        }
        for operation in operations {
            assert_eq!(MailDeliveryOperation::from_name(operation.as_str()), Some(operation));
        }
        for unknown in ["", "settlement", "MAIL_INTAKE", "mail_intake "] {
            assert_eq!(MailDeliveryOrigin::from_name(unknown), None, "{unknown}");
        }
        for unknown in ["", "Send", "forward"] {
            assert_eq!(MailDeliveryOperation::from_name(unknown), None, "{unknown}");
        }
    }

    #[test]
    fn uuid_text_must_be_lowercase_hyphenated_and_non_nil() {
        let value = Uuid::new_v4();
        assert!(canonical_uuid(&value.to_string()));
        for invalid in [
            value.to_string().to_uppercase(),
            value.simple().to_string(),
            value.braced().to_string(),
            value.urn().to_string(),
            Uuid::nil().to_string(),
            String::new(),
        ] {
            assert!(!canonical_uuid(&invalid), "{invalid}");
        }
    }

    #[test]
    fn attachment_ids_are_one_to_32_digits_and_not_all_zero() {
        for valid in ["1", "007", "900000001", &"9".repeat(32)] {
            assert!(valid_attachment_id(valid), "{valid}");
        }
        for invalid in ["", "0", "000", &"1".repeat(33), "12a", " 12", "-1"] {
            assert!(!valid_attachment_id(invalid), "{invalid}");
        }
    }

    /// 合法的入站领取逐方面破坏，`check` 报告对应方面。
    #[test]
    fn check_reports_the_violated_aspect() {
        let attachments = vec!["900000001".to_owned()];
        let long_mail_id = "7".repeat(257);
        let take = MailDeliveryShape {
            origin: MailDeliveryOrigin::MailIntake,
            operation: MailDeliveryOperation::Take,
            command_id: Uuid::new_v4(),
            origin_id: "main:4242:take",
            world: "main",
            external_mail_id: "4242",
            template: None,
            result_code: None,
            user_key_user_id: None,
            body: None,
            credits: 0,
            cod: 0,
            attachment_ids: &attachments,
        };
        assert_eq!(take.check(), Ok(()));
        let legacy = MailDeliveryShape { origin_id: "main:4242:delivery-take", ..take };
        assert_eq!(legacy.check(), Ok(()), "历史 origin_id 别名仍可识别");
        for (shape, expected) in [
            (
                MailDeliveryShape { origin: MailDeliveryOrigin::LotWatch, ..take },
                MailDeliveryShapeError::Operation,
            ),
            (MailDeliveryShape { world: "Main", ..take }, MailDeliveryShapeError::World),
            (
                MailDeliveryShape { external_mail_id: &long_mail_id, ..take },
                MailDeliveryShapeError::ExternalMailId,
            ),
            (MailDeliveryShape { credits: 1, ..take }, MailDeliveryShapeError::Assets),
            (
                MailDeliveryShape {
                    template: Some(MailNotificationTemplate::AuctionNoAttachments),
                    ..take
                },
                MailDeliveryShapeError::Notification,
            ),
            (
                MailDeliveryShape { origin_id: "main:4242:send", ..take },
                MailDeliveryShapeError::Source,
            ),
        ] {
            assert_eq!(shape.check(), Err(expected), "{shape:?}");
        }
    }
}
