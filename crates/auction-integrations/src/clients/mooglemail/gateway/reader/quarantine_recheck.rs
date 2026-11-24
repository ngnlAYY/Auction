//! 隔离信的无 poll 租约重读与收件就绪会话读取。
//!
//! 重检只使用自己的持久 lease/fence 与定位，不写 `mail_poll_pending_reads`；列表缺失或
//! 详情不可读保留 pending，不伪造观察。

use std::time::Instant;

use auction_application::consignment::MailEvidenceSessionDraft;
use auction_application::mail::QuarantineMailLocator;
use auction_application::settlement::InternalEvidenceError;

use super::consignment::{
    ConsignmentReadOutcome, ConsignmentReadPending, pending_from_error, pending_reason,
};
use super::inbox::{ConsignmentReadClassification, classify_consignment_read_error};
use crate::clients::mooglemail::MoogleMailOperation;
use crate::clients::mooglemail::MoogleMailWorld;
use crate::clients::mooglemail::gateway::MoogleMailGateway;
use crate::clients::mooglemail::parser::ConsignmentListRow;
use crate::clients::mooglemail::parser::subject_is_list_projection;
use crate::clients::mooglemail::protocol::valid_opaque_id;

impl MoogleMailGateway {
    /// 按持久定位重读一封隔离信：先枚举 Inbox / Read Mail 的列表行，再 GET 详情。
    pub async fn reread_quarantine(
        &self,
        locator: &QuarantineMailLocator,
        deadline: Instant,
    ) -> ConsignmentReadOutcome {
        if deadline <= Instant::now()
            || !valid_opaque_id(&locator.external_mail_id)
            || locator.external_mail_id.is_empty()
        {
            return ConsignmentReadOutcome::Pending(
                ConsignmentReadPending::TransientReadUnavailable,
            );
        }
        let world = MoogleMailWorld::from(locator.world);
        let row = match self.find_quarantine_row(world, locator, deadline).await {
            Ok(row) => row,
            Err(reason) => return ConsignmentReadOutcome::Pending(reason),
        };
        let session =
            match self.credentials.begin_exclusive(world, MoogleMailOperation::Detail).await {
                Ok(session) => session,
                Err(error) => return pending_from_error(error),
            };
        let target = match self.client.detail(world, &row.external_mail_id) {
            Ok(target) => target,
            Err(error) => return pending_from_error(error),
        };
        let document = match self.client.get(&target, session.cookie()).await {
            Ok(document) => document,
            Err(error) => {
                return match classify_consignment_read_error(error) {
                    ConsignmentReadClassification::Pending(reason) => {
                        ConsignmentReadOutcome::Pending(reason)
                    }
                    ConsignmentReadClassification::ObservedUnclear => {
                        ConsignmentReadOutcome::Pending(
                            ConsignmentReadPending::TransientReadUnavailable,
                        )
                    }
                };
            }
        };
        match super::consignment_candidate::bind_candidate(
            self,
            &session,
            world,
            tokio::time::Instant::from_std(deadline),
            &row,
            &document,
        )
        .await
        {
            Ok(bound) => ConsignmentReadOutcome::Observed(Box::new(bound)),
            Err(reason) => ConsignmentReadOutcome::Pending(reason),
        }
    }

    /// 为收件就绪心跳读取一次认证会话绑定；只在装配根确认读取器已就绪时调用。
    ///
    /// 返回的会话 UUID 是本次进程的运行身份；数据库仍会复核凭据版本、owner 与世界。
    pub async fn readiness_session(
        &self,
        world: MoogleMailWorld,
        deadline: Instant,
    ) -> Result<MailEvidenceSessionDraft, crate::clients::mooglemail::MoogleMailError> {
        let pending = |reason| super::consignment_candidate::pending_error(world, reason);
        let session =
            self.credentials.begin_exclusive(world, MoogleMailOperation::Detail).await.map_err(
                |error| {
                    super::consignment_candidate::pending_error(
                        world,
                        match classify_consignment_read_error(error) {
                            ConsignmentReadClassification::Pending(reason) => reason,
                            ConsignmentReadClassification::ObservedUnclear => {
                                ConsignmentReadPending::TransientReadUnavailable
                            }
                        },
                    )
                },
            )?;
        let binding =
            self.read_identity_in_session(&session, world, deadline).await.map_err(|error| {
                match error {
                    InternalEvidenceError::Unsupported
                    | InternalEvidenceError::Unproven
                    | InternalEvidenceError::InvalidRequest => {
                        pending(ConsignmentReadPending::AuthenticatedUidUnavailable)
                    }
                    _ => pending(ConsignmentReadPending::TransientReadUnavailable),
                }
            })?;
        let (credential_id, credential_version, runtime, _) = session
            .internal_credential_binding(world)
            .map_err(|error| pending(evidence_pending(error)))?;
        let credential_version = u64::try_from(credential_version).map_err(|error| {
            tracing::warn!(error = %error, "MoogleMail 凭据版本超出可签收范围");
            pending(ConsignmentReadPending::AuthenticatedUidUnavailable)
        })?;
        let runtime_generation = u64::try_from(runtime.generation).map_err(|error| {
            tracing::warn!(error = %error, "MoogleMail 运行代次超出可签收范围");
            pending(ConsignmentReadPending::AuthenticatedUidUnavailable)
        })?;
        session
            .check_internal_current(deadline)
            .await
            .map_err(|error| pending(evidence_pending(error)))?;
        Ok(MailEvidenceSessionDraft {
            session_id: uuid::Uuid::new_v4(),
            world: binding.world,
            bot_uid: binding.authenticated_uid,
            player_name: binding.player_name,
            credential_id,
            credential_version,
            runtime_generation,
            protocol_version: binding.protocol_version.to_owned(),
            binding_digest: binding.digest,
        })
    }

    async fn find_quarantine_row(
        &self,
        world: MoogleMailWorld,
        locator: &QuarantineMailLocator,
        deadline: Instant,
    ) -> Result<ConsignmentListRow, ConsignmentReadPending> {
        if deadline <= Instant::now() {
            return Err(ConsignmentReadPending::TransientReadUnavailable);
        }
        // 详情 GET 已把原信标为已读；先读 Read Mail，再读 Inbox 覆盖尚未移出收件箱的旧状态。
        for read_mail in [true, false] {
            let operation =
                if read_mail { MoogleMailOperation::ReadMail } else { MoogleMailOperation::Inbox };
            let session =
                self.credentials.begin_read(world, operation).await.map_err(pending_reason)?;
            let round =
                self.client.read_round(session.cookie(), world, false).map_err(pending_reason)?;
            let list = self
                .client
                .observe_incoming_list(
                    session.cookie(),
                    &round,
                    self.config.maximum_pages,
                    read_mail,
                )
                .await
                .map_err(pending_reason)?;
            let row = list
                .summaries
                .iter()
                .find(|summary| locator_matches(locator, summary))
                .map(super::consignment_candidate::row_from_summary);
            self.client.check_read_round(session.cookie(), &round).await.map_err(pending_reason)?;
            session.check_current().await.map_err(pending_reason)?;
            if let Some(row) = row {
                return Ok(row);
            }
        }
        Err(ConsignmentReadPending::TransientReadUnavailable)
    }
}

fn evidence_pending(error: InternalEvidenceError) -> ConsignmentReadPending {
    match error {
        InternalEvidenceError::Unsupported
        | InternalEvidenceError::Unproven
        | InternalEvidenceError::InvalidRequest => {
            ConsignmentReadPending::AuthenticatedUidUnavailable
        }
        InternalEvidenceError::Unavailable
        | InternalEvidenceError::Expired
        | InternalEvidenceError::Revoked => ConsignmentReadPending::TransientReadUnavailable,
    }
}

fn locator_matches(
    locator: &QuarantineMailLocator,
    summary: &crate::clients::mooglemail::parser::MoogleMailSummary,
) -> bool {
    summary.external_mail_id == locator.external_mail_id
        && summary.world == MoogleMailWorld::from(locator.world)
        && (locator.sender.is_empty() || summary.sender == locator.sender)
        && subject_matches(&locator.subject, &summary.subject)
        && (locator.received_minute_raw.is_empty()
            || summary.received_minute_raw == locator.received_minute_raw)
}

/// 列表标题按 35 字节截断；定位保存的是规范系统主题，必须按可证明前缀匹配。
fn subject_matches(locator_subject: &str, list_subject: &str) -> bool {
    if locator_subject.is_empty() {
        return true;
    }
    if list_subject.eq_ignore_ascii_case(locator_subject) {
        return true;
    }
    subject_is_list_projection(
        &list_subject.to_ascii_lowercase(),
        &locator_subject.to_ascii_lowercase(),
    )
}
