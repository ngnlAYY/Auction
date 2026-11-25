//! 寄售现金轮（载体 1、卖款 S、退款 P）的同会话资金证据：在派发准备取得的同一独占会话里读取认证身份
//! 与（严格现金时）main 现金余额，核对认证 UID 等于成交冻结的资金 UID。
//!
//! 不变量 4、6、7（维护者 2026-11-25 决定）：取不到证据、证据不新鲜、认证 UID 不符或观察形状与记账方式不符，
//! 一律在写前失败关闭，零 POST。余额是否足够不在这里判断：证据随准备交给起写事务，由资金 owner 的通用现金门
//! （与拍卖现金发送同一实现）在账户锁下与该 UID 的全部未支出占用比较，本模块不复制第二份规则。证据绑定本次
//! 尝试、投递租约与冻结身份摘要，起写事务据此拒绝挪用。日志只记世界、投递与固定分类码。

use std::time::Instant;

use auction_application::core::RequestHash;
use auction_application::core::diagnostics::DiagnosticContext;
use auction_application::settlement::{
    InternalEvidencePurpose, InternalEvidenceRequest, SettlementInternalEvidenceSnapshot,
    internal_evidence_deadline,
};
use auction_domain::consignment::dispatch::{AttemptKey, dispatch_attempt_id};
use auction_time::Timestamp;
use uuid::Uuid;

use super::{ConsignmentDispatchMail, PREPARE_BUDGET};
use crate::clients::mooglemail::MoogleMailWorld;
use crate::clients::mooglemail::gateway::MoogleMailCredentialSession;
use crate::clients::mooglemail::transport::types::MoogleMailDeliveryTransport;
use crate::safe_diagnostics;

/// 一封现金轮的资金绑定：证据请求的身份与核对口径。
pub(super) struct CashBinding {
    world: MoogleMailWorld,
    delivery_id: Uuid,
    operation_id: Uuid,
    purpose: InternalEvidencePurpose,
    funding_uid: String,
    observe_main_cash: bool,
}

impl CashBinding {
    /// 冻结 Credits 大于零才是现金轮；资金 UID 缺失、租约 token 为空或 fence 越界时返回 `None`，
    /// 调用方按内容无效不发。
    pub(super) fn from_mail(mail: &ConsignmentDispatchMail) -> Option<Self> {
        if mail.credits <= 0 || mail.claim_token.is_nil() {
            return None;
        }
        let funding_uid = mail.funding_uid.clone()?;
        let attempt = AttemptKey { delivery_id: mail.delivery_id, fence: mail.fence };
        Some(Self {
            world: mail.world,
            delivery_id: mail.delivery_id,
            operation_id: dispatch_attempt_id(&attempt),
            purpose: InternalEvidencePurpose::DeliveryClaim {
                delivery_id: mail.delivery_id,
                claim_token: mail.claim_token,
                claim_fencing: i64::try_from(mail.fence).ok()?,
                request_hash: RequestHash::new(mail.identity_digest),
            },
            funding_uid,
            observe_main_cash: mail.observe_main_cash,
        })
    }

    /// 证据可交起写：认证 UID 是冻结资金 UID、在 `now` 仍有效、绑定本次请求，且严格现金带 main 现金观察、
    /// 记账模式不带（通用现金门对两种模式的证据形状要求）。
    pub(super) fn accepts(
        &self,
        snapshot: &SettlementInternalEvidenceSnapshot,
        now: Timestamp,
    ) -> bool {
        snapshot.identity.authenticated_uid == self.funding_uid
            && snapshot.valid_at(now)
            && snapshot.operation_id == self.operation_id
            && snapshot.purpose == self.purpose
            && snapshot.main_cash.is_some() == self.observe_main_cash
    }
}

impl MoogleMailDeliveryTransport {
    /// 在派发准备的同一独占会话里读取资金证据并核对（见模块说明）；不满足返回 `None`，不写远端。
    pub(super) async fn consignment_cash_evidence(
        &self,
        session: &MoogleMailCredentialSession,
        cash: &CashBinding,
    ) -> Option<SettlementInternalEvidenceSnapshot> {
        let context = DiagnosticContext {
            world: Some(cash.world.into()),
            delivery_id: Some(cash.delivery_id),
            ..Default::default()
        };
        // 与拍卖现金发送同一入口：只有注入了共享凭据集合的 gateway 才能在本会话里取证据。
        let Some(gateway) = &self.cash_evidence_gateway else {
            tracing::warn!(
                world = cash.world.as_str(),
                code = "cash_evidence_unavailable",
                "寄售现金轮没有同会话资金证据入口，不发送"
            );
            return None;
        };
        let request = InternalEvidenceRequest {
            operation_id: cash.operation_id,
            purpose: cash.purpose.clone(),
            world: cash.world.into(),
            equipment: None,
            observe_main_cash: cash.observe_main_cash,
            deadline: internal_evidence_deadline(Instant::now() + PREPARE_BUDGET),
        };
        let snapshot = match gateway.read_internal_evidence_in_session(session, &request).await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                safe_diagnostics::warn(
                    "mooglemail_send",
                    "read_consignment_cash_evidence",
                    safe_diagnostics::project(&error),
                    context,
                );
                return None;
            }
        };
        if !cash.accepts(&snapshot, Timestamp::now()) {
            tracing::warn!(
                world = cash.world.as_str(),
                code = "cash_evidence_rejected",
                "寄售现金轮的同会话资金证据不符（UID、时效或绑定），不发送"
            );
            return None;
        }
        Some(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use auction_application::core::RequestHash;
    use auction_application::settlement::{
        INTERNAL_EVIDENCE_PROTOCOL, InternalEvidencePurpose, MailAccountEvidenceBinding,
        MailRuntimeBinding, SettlementInternalEvidenceSnapshot, VerifiedMainCashObservation,
    };
    use auction_domain::SettlementWorld;
    use auction_time::{Duration, Timestamp};
    use uuid::Uuid;

    use super::CashBinding;
    use crate::clients::mooglemail::MoogleMailWorld;
    use crate::clients::mooglemail::transport::delivery::ConsignmentDispatchMail;

    const FUNDING_UID: &str = "7001";

    fn mail() -> ConsignmentDispatchMail {
        ConsignmentDispatchMail {
            world: MoogleMailWorld::Main,
            delivery_id: Uuid::from_u128(0xd1),
            fence: 1,
            recipient: "FakeSeller".to_owned(),
            subject: "Fake subject".to_owned(),
            body: "Fake body".to_owned(),
            credits: 95,
            cod: 0,
            equipment_ids: Vec::new(),
            claim_token: Uuid::from_u128(0xa1),
            identity_digest: [0x1d; 32],
            funding_uid: Some(FUNDING_UID.to_owned()),
            observe_main_cash: true,
        }
    }

    /// 由同一绑定取得的严格现金证据；`uid` 是会话认证 UID。
    fn snapshot(
        binding: &CashBinding,
        uid: &str,
        observed: Timestamp,
    ) -> SettlementInternalEvidenceSnapshot {
        let mut identity = MailAccountEvidenceBinding {
            credential_id: Uuid::from_u128(0xc1),
            credential_version: 1,
            runtime: MailRuntimeBinding { generation: 1 },
            authenticated_uid: uid.to_owned(),
            player_name: "Fake Bot".to_owned(),
            world: SettlementWorld::Main,
            observed_at: observed,
            protocol_version: INTERNAL_EVIDENCE_PROTOCOL,
            digest: [0; 32],
        };
        identity.digest = identity.compute_digest();
        let mut cash = VerifiedMainCashObservation {
            binding: identity.clone(),
            credits: 1_000,
            observed_at: observed,
            digest: [0; 32],
        };
        cash.digest = cash.compute_digest();
        let mut evidence = SettlementInternalEvidenceSnapshot {
            operation_id: binding.operation_id,
            purpose: binding.purpose.clone(),
            identity,
            equipment: None,
            main_cash: Some(cash),
            sent_collection: None,
            observed_from: observed,
            expires_at: observed + Duration::seconds(30),
            digest: [0; 32],
        };
        evidence.digest = evidence.compute_digest();
        evidence
    }

    /// 同会话证据的认证 UID 是冻结资金 UID 且仍新鲜才可交起写；UID 不符、观察过期都失败关闭。
    #[test]
    fn only_fresh_evidence_of_the_frozen_funding_uid_is_accepted() {
        let binding = CashBinding::from_mail(&mail()).expect("现金轮绑定");
        let now = Timestamp::now();
        let observed = now - Duration::seconds(1);
        assert!(binding.accepts(&snapshot(&binding, FUNDING_UID, observed), now));

        assert!(!binding.accepts(&snapshot(&binding, "7002", observed), now), "UID 不符");
        let stale = now - Duration::seconds(120);
        assert!(!binding.accepts(&snapshot(&binding, FUNDING_UID, stale), now), "观察不新鲜");
    }

    /// 证据绑定本次尝试与投递租约：为另一租约取得的证据不能挪用。
    #[test]
    fn evidence_for_another_lease_is_rejected() {
        let binding = CashBinding::from_mail(&mail()).expect("现金轮绑定");
        let now = Timestamp::now();
        let mut evidence = snapshot(&binding, FUNDING_UID, now - Duration::seconds(1));
        evidence.purpose = InternalEvidencePurpose::DeliveryClaim {
            delivery_id: Uuid::from_u128(0xd1),
            claim_token: Uuid::from_u128(0xa2),
            claim_fencing: 1,
            request_hash: RequestHash::new([0x1d; 32]),
        };
        evidence.digest = evidence.compute_digest();
        assert!(!binding.accepts(&evidence, now));
    }
}
