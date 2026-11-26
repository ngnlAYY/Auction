//! 收款与确领推进循环的装配适配（PIWQ、N1）：待办来源只委托存储，领取观察只经 MoogleMail 投递传输的只读入口，
//! 严格现金的现金观察只经内部证据入口。
//!
//! 不变量 4、6、7：观察只读，不取写锁、不 POST、不 Take；冻结 Send 身份的世界与轮的发信世界不一致时不读，按
//! “无法确定”等待。严格现金收款认证所需的新鲜 main 现金观察经 Worker 已装配的同一 MoogleMail gateway 的内部证据
//! 入口取得（用途 `ConsignmentCollection`，绑定已绑定 Sent 的那次起写尝试）：认证 UID 不等于收据 UID、用途不符、
//! 缺现金或快照已过期一律返回 `None` 等待。余额是否覆盖占用不在这里判断，由存储认证事务在 UID 账户锁内复验。
//! 记账模式不读余额、不受影响。收件人、主题、UID 与余额都不进日志。

use std::sync::Arc;
use std::time::{Duration, Instant};

use auction_application::consignment::{
    CollectionCursor, CollectionPage, ConsignmentFuture, PendingCollection,
};
use auction_application::core::Clock;
use auction_application::settlement::{
    FreshCashObservation, InternalEvidencePurpose, InternalEvidenceRequest,
    SettlementInternalEvidenceReader, SettlementInternalEvidenceSnapshot,
    internal_evidence_deadline,
};
use auction_domain::SettlementWorld;
use auction_domain::consignment::dispatch::{EvidenceCategory, collection_attempt_id};
use auction_integrations::mooglemail::{
    CONSIGNMENT_DISPATCH_COLLECTED_VALIDATOR_VERSION, ConsignmentCollection,
    ConsignmentCollectionTarget, MoogleMailDeliveryTransport, MoogleMailWorld,
};
use auction_jobs::consignment::{
    CollectionObservation, CollectionObserver, CollectionRead, CollectionSource,
};
use auction_storage::CollectedRoundRepository;
use auction_time::Timestamp;
use uuid::Uuid;

/// 存储待办来源。
pub struct StoreCollections {
    /// 收款与确领待办的存储读口。
    pub repository: CollectedRoundRepository,
}

impl CollectionSource for StoreCollections {
    fn pending_collections(
        &self,
        after: Option<CollectionCursor>,
        limit: u32,
    ) -> ConsignmentFuture<'_, CollectionPage> {
        Box::pin(self.repository.pending(after, limit))
    }

    fn validator_signed(&self, category: EvidenceCategory, version: &str) -> bool {
        self.repository.validator_signed(category, version)
    }
}

/// 经 Worker 已装配的投递传输只读观察领取状态。
pub struct MoogleMailCollectionObserver {
    /// 与派发同一个 transport 实例（只用其只读入口）。
    pub transport: Arc<MoogleMailDeliveryTransport>,
    /// 观察时刻来源。
    pub clock: Arc<dyn Clock>,
    /// 内部证据入口（与派发现金证据同一个 MoogleMailGateway），严格现金认证用它读 main 现金。
    pub evidence: Arc<dyn SettlementInternalEvidenceReader>,
}

/// 现金观察的证据预算：与内部证据通用预算一致，由 `internal_evidence_deadline` 收口。
const CASH_EVIDENCE_BUDGET: Duration = Duration::from_secs(60);

/// 一次收款认证现金证据请求的身份：操作 ID 与用途都由起写尝试确定，快照必须原样带回。
#[derive(Debug, Clone, PartialEq, Eq)]
struct CashRequestIdentity {
    operation_id: Uuid,
    purpose: InternalEvidencePurpose,
}

impl CashRequestIdentity {
    /// fence 超出 `i64` 时返回 `None`，调用方按无法确定等待。
    fn for_item(item: &PendingCollection) -> Option<Self> {
        Some(Self {
            operation_id: collection_attempt_id(&item.attempt),
            purpose: InternalEvidencePurpose::ConsignmentCollection {
                delivery_id: item.attempt.delivery_id,
                fence: i64::try_from(item.attempt.fence).ok()?,
            },
        })
    }
}

/// 快照转现金观察：认证 UID 等于收据 UID、绑定本次请求、在 `now` 仍有效且带 main 现金才成立；任一不满足
/// 返回 `None`（不变量 6：失败关闭）。余额与占用的比较留给存储认证事务。
fn cash_observation(
    snapshot: &SettlementInternalEvidenceSnapshot,
    uid: &str,
    request: &CashRequestIdentity,
    now: Timestamp,
) -> Option<FreshCashObservation> {
    if snapshot.identity.authenticated_uid != uid
        || snapshot.operation_id != request.operation_id
        || snapshot.purpose != request.purpose
        || !snapshot.valid_at(now)
    {
        return None;
    }
    let cash = snapshot.main_cash.as_ref()?;
    Some(FreshCashObservation {
        uid: uid.to_owned(),
        credits: cash.credits,
        observed_at: cash.observed_at,
        expires_at: snapshot.expires_at,
    })
}

impl CollectionObserver for MoogleMailCollectionObserver {
    fn validator_version(&self) -> &str {
        CONSIGNMENT_DISPATCH_COLLECTED_VALIDATOR_VERSION
    }

    fn observe<'a>(
        &'a self,
        pending: &'a PendingCollection,
    ) -> ConsignmentFuture<'a, CollectionRead> {
        Box::pin(async move {
            let world = MoogleMailWorld::from(pending.send_world);
            let expectation = &pending.expectation;
            if expectation.world_id != world.as_str() {
                tracing::warn!(
                    code = "collection_identity_mismatch",
                    "收款确领的冻结 Send 身份与发信世界不符，不读取"
                );
                return Ok(CollectionRead::Unavailable);
            }
            let target = ConsignmentCollectionTarget {
                world,
                sent_mid: pending.sent_mid.clone(),
                sending_at: pending.sending_at,
                recipient: expectation.recipient.clone(),
                subject: expectation.subject.clone(),
                body_digest: expectation.body_digest,
                credits: expectation.credits,
                cod: expectation.cod,
                equipment_ids: expectation.equipment_ids.clone(),
            };
            Ok(match self.transport.observe_consignment_collection(&target).await {
                // 版本必须就是本观察端口声明的版本：循环据声明的版本判签收并记入证据。
                ConsignmentCollection::Collected { validator_version, evidence_digest }
                    if validator_version == self.validator_version() =>
                {
                    CollectionRead::Collected(CollectionObservation {
                        remote_observation_id: pending.sent_mid.clone(),
                        observed_at: self.clock.now(),
                        evidence_digest,
                    })
                }
                ConsignmentCollection::Pending => CollectionRead::Pending,
                ConsignmentCollection::Collected { .. } | ConsignmentCollection::Unavailable => {
                    CollectionRead::Unavailable
                }
            })
        })
    }

    fn fresh_main_cash<'a>(
        &'a self,
        item: &'a PendingCollection,
        uid: &'a str,
    ) -> ConsignmentFuture<'a, Option<FreshCashObservation>> {
        Box::pin(async move { Ok(read_fresh_main_cash(self.evidence.as_ref(), item, uid).await) })
    }
}

/// 严格现金认证的现金观察读取：只读，任一环节失败都返回 `None` 等待（不变量 6）。
async fn read_fresh_main_cash(
    evidence: &dyn SettlementInternalEvidenceReader,
    item: &PendingCollection,
    uid: &str,
) -> Option<FreshCashObservation> {
    let Some(identity) = CashRequestIdentity::for_item(item) else {
        tracing::warn!(code = "collection_cash_fence_invalid", "收款认证的尝试围栏越界，等待");
        return None;
    };
    let request = InternalEvidenceRequest {
        operation_id: identity.operation_id,
        purpose: identity.purpose.clone(),
        world: SettlementWorld::Main,
        equipment: None,
        observe_main_cash: true,
        deadline: internal_evidence_deadline(Instant::now() + CASH_EVIDENCE_BUDGET),
    };
    // 会话随 `prepared` 在本作用域结束时释放；只读，不取提交守卫。
    let prepared = match evidence.prepare_internal_evidence(request).await {
        Ok(prepared) => prepared,
        Err(error) => {
            tracing::warn!(code = error.safe_kind(), "收款认证读取现金证据失败，等待");
            return None;
        }
    };
    if let Err(error) = prepared.check_current().await {
        tracing::warn!(code = error.safe_kind(), "收款认证的现金证据已不当前，等待");
        return None;
    }
    let observation = cash_observation(prepared.snapshot(), uid, &identity, Timestamp::now());
    if observation.is_none() {
        tracing::warn!(code = "collection_cash_rejected", "收款认证的现金证据不符，等待");
    }
    observation
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::Instant;

    use auction_application::consignment::{CollectionLeg, PendingCollection};
    use auction_application::delivery::DeliverySendExpectation;
    use auction_application::settlement::{
        INTERNAL_EVIDENCE_PROTOCOL, InternalEvidenceError, InternalEvidenceFuture,
        InternalEvidenceLease, InternalEvidencePurpose, InternalEvidenceRequest,
        MailAccountEvidenceBinding, MailRuntimeBinding, PreparedSettlementInternalEvidence,
        SettlementInternalEvidenceReader, SettlementInternalEvidenceSnapshot,
        VerifiedMainCashObservation,
    };
    use auction_domain::SettlementWorld;
    use auction_domain::consignment::dispatch::{
        AttemptKey, DispatchRoundKey, collection_attempt_id,
    };
    use auction_time::{Duration, Timestamp};
    use uuid::Uuid;

    use super::{CashRequestIdentity, cash_observation, read_fresh_main_cash};

    const RECEIPT_UID: &str = "7001";

    fn request() -> CashRequestIdentity {
        CashRequestIdentity {
            operation_id: Uuid::from_u128(0x0b),
            purpose: InternalEvidencePurpose::ConsignmentCollection {
                delivery_id: Uuid::from_u128(0xd1),
                fence: 3,
            },
        }
    }

    /// 绑定 `request` 的严格现金快照；`uid` 是会话认证 UID，`with_cash` 控制是否带现金观察。
    fn snapshot(
        request: &CashRequestIdentity,
        uid: &str,
        with_cash: bool,
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
        let main_cash = with_cash.then(|| {
            let mut cash = VerifiedMainCashObservation {
                binding: identity.clone(),
                credits: 1_000,
                observed_at: observed,
                digest: [0; 32],
            };
            cash.digest = cash.compute_digest();
            cash
        });
        let mut snapshot = SettlementInternalEvidenceSnapshot {
            operation_id: request.operation_id,
            purpose: request.purpose.clone(),
            identity,
            equipment: None,
            main_cash,
            sent_collection: None,
            observed_from: observed,
            expires_at: observed + Duration::seconds(30),
            digest: [0; 32],
        };
        snapshot.digest = snapshot.compute_digest();
        snapshot
    }

    /// 不变量 6：UID 相符、绑定本次尝试、带现金且在时窗内才得到观察；其余一律失败关闭。
    #[test]
    fn snapshot_becomes_observation_only_when_every_binding_holds() {
        let request = request();
        let now = Timestamp::now();
        let observed = now - Duration::seconds(1);

        let good = snapshot(&request, RECEIPT_UID, true, observed);
        let observation = cash_observation(&good, RECEIPT_UID, &request, now).expect("同 UID 观察");
        assert_eq!(observation.uid, RECEIPT_UID);
        assert_eq!(observation.credits, 1_000);
        assert_eq!(observation.observed_at, observed);
        assert_eq!(observation.expires_at, good.expires_at);

        let other_purpose = CashRequestIdentity {
            purpose: InternalEvidencePurpose::ConsignmentCollection {
                delivery_id: Uuid::from_u128(0xd1),
                fence: 4,
            },
            ..request
        };
        let other_operation =
            CashRequestIdentity { operation_id: Uuid::from_u128(0x0c), ..request.clone() };
        let rejected = [
            ("UID 不符", snapshot(&request, "7002", true, observed), &request),
            ("缺现金", snapshot(&request, RECEIPT_UID, false, observed), &request),
            ("用途不符", good_for(&other_purpose, observed), &request),
            ("操作 ID 不符", good_for(&other_operation, observed), &request),
            (
                "已过期",
                snapshot(&request, RECEIPT_UID, true, now - Duration::seconds(120)),
                &request,
            ),
        ];
        for (label, snapshot, request) in rejected {
            assert_eq!(cash_observation(&snapshot, RECEIPT_UID, request, now), None, "{label}");
        }
    }

    fn good_for(
        request: &CashRequestIdentity,
        observed: Timestamp,
    ) -> SettlementInternalEvidenceSnapshot {
        snapshot(request, RECEIPT_UID, true, observed)
    }

    /// 假内部证据入口的行为。
    #[derive(Clone, Copy)]
    enum Mode {
        Good,
        PrepareFails,
        CheckFails,
        NoCash,
    }

    /// 记录收到的请求；按 `mode` 返回绑定该请求的快照。
    struct FakeEvidence {
        mode: Mode,
        requests: Mutex<Vec<InternalEvidenceRequest>>,
    }

    struct FakeLease {
        check_ok: bool,
    }

    impl InternalEvidenceLease for FakeLease {
        fn is_current(&self) -> bool {
            true
        }

        fn check_current(&self, _deadline: Instant) -> InternalEvidenceFuture<'_, ()> {
            let result = if self.check_ok { Ok(()) } else { Err(InternalEvidenceError::Revoked) };
            Box::pin(async move { result })
        }

        fn revoke(&self) {}
    }

    impl SettlementInternalEvidenceReader for FakeEvidence {
        fn prepare_internal_evidence(
            &self,
            request: InternalEvidenceRequest,
        ) -> InternalEvidenceFuture<'_, PreparedSettlementInternalEvidence> {
            let bound = CashRequestIdentity {
                operation_id: request.operation_id,
                purpose: request.purpose.clone(),
            };
            let deadline = request.deadline;
            self.requests.lock().expect("记录请求").push(request);
            let result = match self.mode {
                Mode::PrepareFails => Err(InternalEvidenceError::Unavailable),
                mode => {
                    let observed = Timestamp::now() - Duration::seconds(1);
                    let with_cash = !matches!(mode, Mode::NoCash);
                    let lease = FakeLease { check_ok: !matches!(mode, Mode::CheckFails) };
                    PreparedSettlementInternalEvidence::new(
                        snapshot(&bound, RECEIPT_UID, with_cash, observed),
                        Box::new(lease),
                        deadline,
                    )
                }
            };
            Box::pin(async move { result })
        }
    }

    fn pending() -> PendingCollection {
        PendingCollection {
            key: DispatchRoundKey {
                contract_id: Uuid::from_u128(1),
                obligation_id: Uuid::from_u128(2),
                round: 1,
            },
            attempt: AttemptKey { delivery_id: Uuid::from_u128(0xd1), fence: 3 },
            leg: CollectionLeg::MainOriginalCod,
            send_world: SettlementWorld::Main,
            sent_mid: "9001".to_owned(),
            sending_at: Timestamp::now(),
            expectation: DeliverySendExpectation {
                world_id: "main".to_owned(),
                recipient: "FakeBuyer".to_owned(),
                subject: "Fake subject".to_owned(),
                body_digest: [1; 32],
                credits: 0,
                cod: 100,
                equipment_ids: vec!["42".to_owned()],
                credential: None,
            },
            evidence: None,
            certify: None,
        }
    }

    /// 不变量 4、6：请求只读 main 现金且绑定本次尝试，得到同 UID 观察。
    #[tokio::test]
    async fn fresh_cash_request_is_bound_to_the_attempt() {
        let evidence = FakeEvidence { mode: Mode::Good, requests: Mutex::new(Vec::new()) };
        let item = pending();
        let observation = read_fresh_main_cash(&evidence, &item, RECEIPT_UID).await;
        let observation = observation.expect("同 UID 现金观察");
        assert_eq!(observation.uid, RECEIPT_UID);
        assert_eq!(observation.credits, 1_000);

        let requests = evidence.requests.lock().expect("读取请求");
        let [request] = requests.as_slice() else { panic!("应恰好发出一次请求") };
        assert_eq!(request.world, SettlementWorld::Main);
        assert!(request.observe_main_cash);
        assert!(request.equipment.is_none());
        assert_eq!(request.operation_id, collection_attempt_id(&item.attempt));
        assert_eq!(
            request.purpose,
            InternalEvidencePurpose::ConsignmentCollection {
                delivery_id: Uuid::from_u128(0xd1),
                fence: 3,
            }
        );
    }

    /// 不变量 6：准备失败、会话已不当前、快照缺现金都按无法确定等待。
    #[tokio::test]
    async fn fresh_cash_failures_wait() {
        for mode in [Mode::PrepareFails, Mode::CheckFails, Mode::NoCash] {
            let evidence = FakeEvidence { mode, requests: Mutex::new(Vec::new()) };
            assert_eq!(read_fresh_main_cash(&evidence, &pending(), RECEIPT_UID).await, None);
        }
    }
}
