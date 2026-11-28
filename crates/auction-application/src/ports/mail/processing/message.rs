//! MoogleMail 业务 envelope 与装备附件读取端口。

use std::{collections::HashSet, fmt};

use auction_domain::{EquipmentKey, valid_text};
use auction_time::Timestamp;

use super::delivery_shape::valid_attachment_id;
use super::errors::{MailProcessingError, clear_string};
use crate::ports::admin::AdminIntakeSourceSnapshot;
use crate::ports::core::ApplicationFuture;

/// MoogleMail 业务 envelope 中单件装备附件。
/// key 只用于读取可信装备页和生成密钥信封，禁止实现任何会输出 key 的格式化或序列化。
pub struct MailEquipmentAttachment {
    pub external_equipment_id: String,
    pub key: EquipmentKey,
}

impl fmt::Debug for MailEquipmentAttachment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MailEquipmentAttachment")
            .field("external_equipment_id", &self.external_equipment_id)
            .field("key", &"[REDACTED]")
            .finish()
    }
}

impl MailEquipmentAttachment {
    /// 清零短生命周期装备 key。
    pub fn clear_secret(&mut self) {
        self.key.clear();
    }

    /// 为同一封邮件的有界幂等重试建立短生命周期附件副本。
    fn duplicate_for_retry(&self) -> Self {
        Self {
            external_equipment_id: self.external_equipment_id.clone(),
            key: self.key.duplicate_for_retry(),
        }
    }
}

/// 已由外部协议适配器校验的 MoogleMail 业务 envelope。
/// 该类型不允许进入数据库或持久化命令。Drop 会尽力清除正文、发件人、标题和附件 key。
pub struct MailMessage {
    pub mailbox_key: String,
    pub external_mail_id: String,
    pub world_id: String,
    pub received_at: Timestamp,
    pub sender: String,
    pub subject: String,
    pub body: String,
    pub credits: i64,
    pub cod: i64,
    pub attachments: Vec<MailEquipmentAttachment>,
    pub returnable: bool,
    pub claimable: bool,
    pub content_digest: [u8; 32],
}

impl fmt::Debug for MailMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MailMessage")
            .field("mailbox_key", &self.mailbox_key)
            .field("external_mail_id", &self.external_mail_id)
            .field("world_id", &self.world_id)
            .field("received_at", &self.received_at)
            .field("sender", &"[REDACTED]")
            .field("subject", &"[REDACTED]")
            .field("body", &"[REDACTED]")
            .field("credits", &self.credits)
            .field("cod", &self.cod)
            .field("attachment_count", &self.attachments.len())
            .field("returnable", &self.returnable)
            .field("claimable", &self.claimable)
            .field("content_digest", &self.content_digest)
            .finish()
    }
}

impl Drop for MailMessage {
    fn drop(&mut self) {
        self.clear_secrets();
    }
}

impl MailMessage {
    /// 校验 Worker 到应用边界的 envelope，拒绝不完整或越界的业务输入。
    pub fn validate(&self) -> Result<(), MailProcessingError> {
        // stable_command_id 使用 `/` 分隔身份字段；输入边界拒绝分隔符以保持编码唯一。
        if !valid_text(&self.mailbox_key, 256)
            || self.mailbox_key.contains('/')
            || self.external_mail_id.contains('/')
            || !valid_text(&self.external_mail_id, 512)
            || !matches!(self.world_id.as_str(), "main" | "isekai")
            || self.received_at.timestamp_nanos_opt().is_none()
            || !valid_text(&self.sender, 256)
            || !valid_text(&self.subject, 512)
            || self.body.len() > 64 * 1024
            || self
                .body
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
            || self.credits < 0
            || self.cod < 0
            || self.content_digest == [0; 32]
            || self.attachments.is_empty() && self.cod > 0 && self.credits == 0
            || self.attachments.len() > 1000
        {
            return Err(MailProcessingError::InvalidMessage);
        }
        // 只索引借用的外部 ID，避免千件附件逐项扫描前缀；不复制装备 key 或改动附件顺序。
        // 附件 ID 与统一投递判定同一谓词：入队必然拒绝的附件在入站边界即失败关闭。
        let mut attachment_ids = HashSet::with_capacity(self.attachments.len());
        for attachment in &self.attachments {
            if !valid_attachment_id(&attachment.external_equipment_id)
                || !attachment_ids.insert(attachment.external_equipment_id.as_str())
            {
                return Err(MailProcessingError::InvalidMessage);
            }
        }
        Ok(())
    }

    /// 清除当前 envelope 持有的敏感短生命周期数据。
    pub fn clear_secrets(&mut self) {
        clear_string(&mut self.sender);
        clear_string(&mut self.subject);
        clear_string(&mut self.body);
        for attachment in &mut self.attachments {
            attachment.clear_secret();
        }
    }

    /// 为同一稳定命令 ID 的有界进程内重试建立显式副本。
    /// 该类型刻意不实现 Clone；只有 Worker 的有限次数重试可以调用此入口，并且任意时刻最多
    /// 保留原 envelope 与一个执行副本。所有副本仍通过 Drop 清除正文、身份和装备 key。
    pub fn duplicate_for_retry(&self) -> Self {
        Self {
            mailbox_key: self.mailbox_key.clone(),
            external_mail_id: self.external_mail_id.clone(),
            world_id: self.world_id.clone(),
            received_at: self.received_at,
            sender: self.sender.clone(),
            subject: self.subject.clone(),
            body: self.body.clone(),
            credits: self.credits,
            cod: self.cod,
            attachments: self
                .attachments
                .iter()
                .map(MailEquipmentAttachment::duplicate_for_retry)
                .collect(),
            returnable: self.returnable,
            claimable: self.claimable,
            content_digest: self.content_digest,
        }
    }

    /// 返回排序后的外部装备 ID，用于稳定 delivery payload 和审计元数据。
    pub fn sorted_attachment_ids(&self) -> Vec<String> {
        let mut ids = self
            .attachments
            .iter()
            .map(|attachment| attachment.external_equipment_id.clone())
            .collect::<Vec<_>>();
        ids.sort_unstable();
        ids
    }
}

/// MoogleMail 附件解析得到的可信装备读取器。
pub trait MailEquipmentReader: Send + Sync {
    fn read(
        &self,
        world_id: String,
        attachment: MailEquipmentAttachment,
    ) -> ApplicationFuture<'_, AdminIntakeSourceSnapshot>;

    /// 读取同一装备并携带非权威诊断；旧读取器默认保留原结果。
    fn read_diagnostic(
        &self,
        context: crate::ports::mail::MailEquipmentDiagnosticContext,
        attachment: MailEquipmentAttachment,
    ) -> crate::ports::mail::MailEquipmentReadFuture<'_> {
        Box::pin(async move {
            let result = self.read(context.world_id.clone(), attachment).await;
            let observation = crate::ports::mail::MailEquipmentObservation::unavailable(context);
            match result {
                Ok(snapshot) => {
                    crate::ports::mail::MailEquipmentReadOutcome::Loaded { snapshot, observation }
                }
                Err(error) => {
                    crate::ports::mail::MailEquipmentReadOutcome::Failed { error, observation }
                }
            }
        })
    }

    /// 有界留存失败证据并生成安全摘要；诊断失败不能成为业务错误。
    /// 默认实现没有记录器：已准备的证据只能如实报告留存不可用；准备阶段已经失败时必须保留
    /// 原失败状态和机器码，不能降级成 disabled，否则拒绝证据的失败原因被静默丢弃。
    fn retain_failure(
        &self,
        observation: crate::ports::mail::MailEquipmentObservation,
        stage: crate::ports::mail::MailEquipmentDiagnosticStage,
        category: crate::ports::mail::MailEquipmentDiagnosticCategory,
        detail_code: Option<String>,
    ) -> crate::ports::mail::MailEquipmentRetentionFuture<'static> {
        Box::pin(async move {
            let evidence_id = observation
                .prepared
                .as_ref()
                .map(crate::ports::mail::diagnostics::PreparedMailEvidence::evidence_id);
            let (evidence_status, retention_error) = if evidence_id.is_some() {
                (
                    crate::ports::mail::MailEquipmentEvidenceStatus::SaveFailed,
                    Some("recorder_unavailable".to_owned()),
                )
            } else if observation.preparation_status
                == crate::ports::mail::MailEquipmentEvidenceStatus::PrepareFailed
            {
                (
                    crate::ports::mail::MailEquipmentEvidenceStatus::PrepareFailed,
                    Some(
                        observation
                            .preparation_error
                            .unwrap_or_else(|| "invalid_context".to_owned()),
                    ),
                )
            } else {
                (crate::ports::mail::MailEquipmentEvidenceStatus::Disabled, None)
            };
            crate::ports::mail::MailEquipmentDiagnosticSnapshot {
                schema_version: 1,
                equipment_id: observation.context.external_equipment_id,
                attachment_position: observation.context.attachment_position,
                stage,
                category,
                detail_code,
                http_status: observation.http_status,
                elapsed_millis: observation.elapsed_millis,
                evidence_id,
                evidence_status,
                retention_error,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;

    use auction_domain::EquipmentKey;
    use auction_time::Timestamp;

    use super::{MailEquipmentAttachment, MailEquipmentReader, MailMessage};
    use crate::ports::admin::AdminIntakeSourceSnapshot;
    use crate::ports::core::ApplicationFuture;
    use crate::ports::mail::{
        MailEquipmentDiagnosticCategory, MailEquipmentDiagnosticContext,
        MailEquipmentDiagnosticStage, MailEquipmentEvidenceStatus, MailEquipmentObservation,
        MailProcessingError,
    };

    fn message(attachment_ids: &[&str]) -> MailMessage {
        MailMessage {
            mailbox_key: "mooglemail:main".to_owned(),
            external_mail_id: "3125357".to_owned(),
            world_id: "main".to_owned(),
            received_at: Timestamp::from_timestamp(1_790_000_000, 0).expect("固定时间"),
            sender: "FixtureSender".to_owned(),
            subject: "auction".to_owned(),
            body: String::new(),
            credits: 0,
            cod: 0,
            attachments: attachment_ids
                .iter()
                .map(|external_equipment_id| MailEquipmentAttachment {
                    external_equipment_id: (*external_equipment_id).to_owned(),
                    key: EquipmentKey::new("0123456789").expect("固定装备 key"),
                })
                .collect(),
            returnable: true,
            claimable: true,
            content_digest: [7; 32],
        }
    }

    /// 入站附件装备 ID 与统一投递判定同一口径：1–32 位十进制且不全为零；空串、全零、33 位与
    /// 非数字在入站边界即拒绝，不等到入队才失败；重复附件同样拒绝。
    #[test]
    fn attachment_ids_follow_the_delivery_shape_rule() {
        let longest = format!("1{}", "0".repeat(31));
        for admitted in ["300753957", "1", "0012", longest.as_str()] {
            assert_eq!(message(&[admitted]).validate(), Ok(()), "{admitted}");
        }
        let too_long = format!("1{}", "0".repeat(32));
        for rejected in ["", "0", "000", too_long.as_str(), "12a"] {
            assert_eq!(
                message(&[rejected]).validate(),
                Err(MailProcessingError::InvalidMessage),
                "{rejected}"
            );
        }
        assert_eq!(
            message(&["300753957", "300753957"]).validate(),
            Err(MailProcessingError::InvalidMessage)
        );
        assert_eq!(message(&[]).validate(), Ok(()), "没有附件的普通邮件不受影响");
    }

    /// 只轮询立即就绪的默认留存 future，不引入额外异步运行时依赖。
    fn block_on<F: Future>(future: F) -> F::Output {
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        let mut future = Box::pin(future);
        match future.as_mut().poll(&mut context) {
            std::task::Poll::Ready(value) => value,
            std::task::Poll::Pending => panic!("默认留存的 future 不应挂起"),
        }
    }

    struct DiagnosticOnlyReader;

    impl MailEquipmentReader for DiagnosticOnlyReader {
        fn read(
            &self,
            _world_id: String,
            _attachment: MailEquipmentAttachment,
        ) -> ApplicationFuture<'_, AdminIntakeSourceSnapshot> {
            Box::pin(async { unreachable!("默认留存不重新读取装备") })
        }
    }

    fn diagnostic_observation() -> MailEquipmentObservation {
        MailEquipmentObservation::unavailable(MailEquipmentDiagnosticContext {
            mailbox_key: "mooglemail:main".to_owned(),
            world_id: "main".to_owned(),
            external_mail_id: "3125357".to_owned(),
            external_equipment_id: "300753957".to_owned(),
            attachment_position: 1,
        })
    }

    /// 默认留存没有记录器：未准备证据时报告 disabled，准备已失败时保留原失败码，不得伪报成
    /// disabled；两种快照都必须通过安全投影校验才能进入入站事实。
    #[test]
    fn default_retention_reports_the_real_preparation_state() {
        let reader = DiagnosticOnlyReader;
        let disabled = block_on(reader.retain_failure(
            diagnostic_observation(),
            MailEquipmentDiagnosticStage::Response,
            MailEquipmentDiagnosticCategory::Timeout,
            None,
        ));
        assert!(disabled.validate());
        assert_eq!(disabled.evidence_status, MailEquipmentEvidenceStatus::Disabled);
        assert_eq!(disabled.evidence_id, None);
        assert_eq!(disabled.retention_error, None);

        let prepare_failed = block_on(reader.retain_failure(
            MailEquipmentObservation {
                preparation_status: MailEquipmentEvidenceStatus::PrepareFailed,
                preparation_error: Some("payload_too_large".to_owned()),
                ..diagnostic_observation()
            },
            MailEquipmentDiagnosticStage::Response,
            MailEquipmentDiagnosticCategory::ProtocolDrift,
            None,
        ));
        assert!(prepare_failed.validate());
        assert_eq!(prepare_failed.evidence_status, MailEquipmentEvidenceStatus::PrepareFailed);
        assert_eq!(prepare_failed.retention_error.as_deref(), Some("payload_too_large"));
    }
}
