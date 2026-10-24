//! 在已有身份/现金 session 中复用既有 Sent 解析，不重取 guard、不推算原 COD 金额。

use std::time::{Duration, Instant};

use auction_application::{
    InternalEvidenceError as Error, InternalEvidenceRequest, InternalSentCollectionTarget,
    MailAccountEvidenceBinding, PreparedSettlementInternalEvidence,
    VerifiedMailSentCollectionObservation,
};
use chrono::Utc;

use super::{
    internal::{mail_world, map_read_error},
    *,
};
use crate::clients::mooglemail::parser::subject_is_list_projection;

impl MoogleMailGateway {
    pub(super) async fn prepare_with_sent(
        &self,
        mut request: InternalEvidenceRequest,
        target: InternalSentCollectionTarget,
    ) -> Result<PreparedSettlementInternalEvidence, Error> {
        if !request.validate() || !target.validate() {
            return Err(Error::InvalidRequest);
        }
        request.deadline = auction_application::internal_evidence_deadline(request.deadline)
            .min(Instant::now() + self.config.fetch_budget.min(Duration::from_secs(30)));
        let deadline = request.deadline;
        tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), async {
            let session = self
                .credentials
                .begin_exclusive(mail_world(request.world), MoogleMailOperation::Identity)
                .await
                .map_err(map_read_error)?;
            let mut snapshot = self.read_internal_evidence_in_session(&session, &request).await?;
            snapshot.sent_collection = Some(
                self.read_sent_collection_in_session(
                    &session,
                    &snapshot.identity,
                    &target,
                    deadline,
                )
                .await?,
            );
            snapshot.digest = snapshot.compute_digest();
            // Sent 读取已在 read_deadline 内完成 owner 复核；不占用预留给提交的一秒再探测。
            PreparedSettlementInternalEvidence::new(snapshot, Box::new(session), deadline)
        })
        .await
        .map_err(|_| Error::Expired)?
    }

    /// 同 session 窄读取入口；调用方保留原 guard/绝对预算，禁止嵌套 begin_exclusive。
    /// target 只来自可信成功 delivery；本方法不凭零剩余资产推算原金额或资金化资格。
    pub(crate) async fn read_sent_collection_in_session(
        &self,
        session: &MoogleMailCredentialSession,
        identity: &MailAccountEvidenceBinding,
        target: &InternalSentCollectionTarget,
        deadline: Instant,
    ) -> Result<VerifiedMailSentCollectionObservation, Error> {
        if !target.validate() || !identity.validate() || !self.credentials.owns_session(session) {
            return Err(Error::InvalidRequest);
        }
        let now = Utc::now();
        if identity.observed_at > now || now - identity.observed_at > chrono::Duration::seconds(30)
        {
            return Err(Error::Expired);
        }
        let read_deadline = deadline.checked_sub(Duration::from_secs(1)).ok_or(Error::Expired)?;
        if read_deadline <= Instant::now() {
            return Err(Error::Expired);
        }
        tokio::time::timeout_at(tokio::time::Instant::from_std(read_deadline), async {
            let world = mail_world(target.world);
            let (credential_id, version, runtime, _) =
                session.internal_credential_binding(world)?;
            if identity.credential_id != credential_id
                || identity.credential_version != version
                || identity.runtime != runtime
            {
                return Err(Error::Revoked);
            }
            let binding = if identity.world == target.world {
                identity.clone()
            } else {
                self.read_identity_in_session(session, world, read_deadline).await?
            };
            if !identity.same_account(&binding) {
                return Err(Error::Unproven);
            }
            self.require_clean_compose(session, world).await?;
            // 原路径已经严格核对列表与详情的 MID/world/recipient/subject/sent_at 和资产矩阵。
            let observed = match self
                .read_sent_with_cookie(
                    world,
                    &target.external_mail_id,
                    Utc::now(),
                    session.cookie(),
                    true,
                )
                .await
            {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    if error.is_session_expired() {
                        let _ = session.invalidate_internal_current().await;
                    }
                    return Err(map_read_error(error));
                }
            };
            if !observed.validate()
                || observed.external_mail_id != target.external_mail_id
                || observed.world != world
                || observed.recipient != target.recipient
                || !subject_is_list_projection(&target.subject, &observed.subject)
                || target.expected_sent_at.is_some_and(|expected| expected != observed.sent_at)
                || !observed.collected
            {
                return Err(Error::Unproven);
            }
            session.check_internal_current(read_deadline).await?;
            let observed_at = Utc::now();
            if identity.observed_at > observed_at
                || observed_at - identity.observed_at > chrono::Duration::seconds(30)
            {
                return Err(Error::Expired);
            }
            let mut evidence = VerifiedMailSentCollectionObservation {
                binding,
                external_mail_id: observed.external_mail_id,
                recipient: observed.recipient,
                subject: observed.subject,
                sent_at: observed.sent_at,
                observed_at,
                remaining_credits: observed.remaining_credits,
                cod_pending: observed.cod_pending,
                remaining_equipment_ids: observed.remaining_equipment_ids,
                collected: observed.collected,
                digest: [0; 32],
            };
            evidence.digest = evidence.compute_digest();
            Ok(evidence)
        })
        .await
        .map_err(|_| Error::Expired)?
    }
}
