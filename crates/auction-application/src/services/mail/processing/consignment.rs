//! 寄售来信分类：在既有拍卖路由之后、`process_unknown` 的资产分支之前运行。
//!
//! 本模块只接受已被拍卖路由判定为“旧 owner 无法归属”的入站信和 4D1 的绑定观察。它先检查
//! 可识别的旧拍卖直达（投稿续办、可识别退款、原 owner 证据），再调用领域整封决定；只有寄售
//! 自己的决定才经 4A 的 `DepositCommandService` 生成规范摘要与系统 UUIDv5 并写入终局。

use std::sync::Arc;

use auction_domain::UserId;
use auction_domain::consignment::deposit::{
    EnvelopeObservation, EnvelopeRoute, LockedEnvelopeFacts, PendingReason, SenderIdentity,
    TrustedConsignmentRts, decide_envelope_with_origin,
};
use auction_domain::consignment::{
    EnvelopeCapabilities, LegacyAuctionOwner, OriginalMailKey, ReservationBinding,
    is_consign_subject,
};
use auction_time::Timestamp;

use crate::ports::consignment::{ConsignmentError, DecideEnvelopeCommand, EnvelopeReceipt};
use crate::ports::mail::MailMessage;
use crate::ports::settlement::system_returned_settlement_obligation_id;
use crate::services::consignment::DepositCommandService;

/// 非寄售输入的安全分类；调用方必须继续原有路由。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConsignmentNotApplicable {
    /// 主题既不是 `consign`，也不是待判的系统 RTS。
    NotConsignmentSubject,
    /// 观察已带旧拍卖 owner；原 owner 必须继续处理。
    LegacyAuctionOwner(LegacyAuctionOwner),
    /// 系统发件人与规范主题已足以归入旧拍卖退款/卖款案件。
    IdentifiableLegacySettlementReturn,
}

/// 寄售分类结果；`Pending` 与 `NotApplicable` 均不写任何寄售终局。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsignmentMailOutcome {
    /// 与寄售无关，或已明确属于旧拍卖直达。
    NotApplicable(ConsignmentNotApplicable),
    /// 保留原定位稍后重读，不写终局。
    Pending(PendingReason),
    /// 已有最终决定，调用方恢复原动作。
    Recovered(EnvelopeReceipt),
    /// 本次首次决定或同命令重放。
    Decided(EnvelopeReceipt),
}

/// 纯分类计划：不打开事务、不写观察或决定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsignmentMailPlan {
    /// 与寄售无关，或已明确属于旧拍卖直达。
    NotApplicable(ConsignmentNotApplicable),
    /// 保留原定位稍后重读，不写终局。
    Pending(PendingReason),
    /// 已有最终决定，调用方只推进前缀并恢复原动作。
    Recovered(EnvelopeReceipt),
    /// 首次寄售决定；存储须在同一事务内写观察、决定与前缀。
    First(Box<DecideEnvelopeCommand>),
}

/// 分类入口的全部输入。
///
/// `observation` 是 4D1 绑定观察经应用层补齐卖家、可交易性、历史与受保护凭证引用后的领域
/// 观察；`reservations` 是调用方在锁内预读的预约集合。存储仍须在 `decide_envelope` 的
/// Serializable 事务内重新锁定并核对同一集合。
pub struct ConsignmentMailInput<'a> {
    /// 已通过既有 MoogleMail 边界校验的入站信；本模块只读取主题、发件人与世界。
    pub message: &'a MailMessage,
    /// 认证 Bot UID、世界与规范 MID；无法建立认证 UID 时为空。
    pub mail_key: Option<OriginalMailKey>,
    /// 4D1 绑定的原始观察，`observation_id` 必须是本次真实观察身份。
    pub observation: EnvelopeObservation,
    /// 锁内预约快照。
    pub reservations: Vec<ReservationBinding>,
    /// 发件人对应的唯一卖家。
    pub envelope_seller_user_id: Option<UserId>,
    /// 当前世界已签收的收件能力；第 4 期取得验证器未签收时为 false。
    pub capabilities: EnvelopeCapabilities,
    /// 临时限流、战斗或读不到详情。
    pub read_failure: bool,
    /// 已由 4C 读回的首次决定；存在时只恢复，不写第二份。
    pub existing_decision: Option<EnvelopeReceipt>,
    /// 锁外预读时钟；存储会在锁内重算。
    pub now: Timestamp,
}

/// 寄售来信分类用例。
pub struct ConsignmentMailService {
    deposits: Option<Arc<DepositCommandService>>,
}

impl ConsignmentMailService {
    /// 注入 4A 的寄售写用例；规范编码与系统 UUIDv5 只在该服务内产生。
    pub fn new(deposits: Arc<DepositCommandService>) -> Self {
        Self { deposits: Some(deposits) }
    }

    /// 只做纯分类的实例；poll 的首次决定由
    /// [`PollEnvelopeStore`](crate::ports::consignment::deposit::PollEnvelopeStore) 在同一事务内提交。
    pub fn planner() -> Self {
        Self { deposits: None }
    }

    /// 分类一封已由拍卖路由判为无法归属的来信。
    ///
    /// 不变量 1、3、4、6、8：旧拍卖直达零寄售写入；疑似系统 RTS 不走普通接受、退回或空信
    /// 归档；首次决定与隔离回执由存储命令账本幂等恢复，pending 不写终局。
    pub async fn classify(
        &self,
        input: ConsignmentMailInput<'_>,
    ) -> Result<ConsignmentMailOutcome, ConsignmentError> {
        match self.plan(input).await? {
            ConsignmentMailPlan::NotApplicable(reason) => {
                Ok(ConsignmentMailOutcome::NotApplicable(reason))
            }
            ConsignmentMailPlan::Pending(reason) => Ok(ConsignmentMailOutcome::Pending(reason)),
            ConsignmentMailPlan::Recovered(receipt) => {
                Ok(ConsignmentMailOutcome::Recovered(receipt))
            }
            ConsignmentMailPlan::First(command) => {
                let deposits = self.deposits.as_ref().ok_or(ConsignmentError::Unavailable)?;
                match deposits.commit_decide_envelope(*command).await {
                    Ok(receipt) => Ok(ConsignmentMailOutcome::Decided(receipt)),
                    // 同 K 的首次决定已被并发写入；本轮不写第二份终局，下一轮由已有决定恢复。
                    Err(ConsignmentError::CommandIdConflict) => {
                        Ok(ConsignmentMailOutcome::Pending(PendingReason::ExistingDecision))
                    }
                    Err(error) => Err(error),
                }
            }
        }
    }

    /// 纯分类：先恢复已有决定与旧拍卖直达，再按锁外事实得到首次寄售命令。
    pub async fn plan(
        &self,
        input: ConsignmentMailInput<'_>,
    ) -> Result<ConsignmentMailPlan, ConsignmentError> {
        self.plan_with_origin(input, None).await
    }

    /// 带寄售来源的纯分类：`origin` 是 poll 预读按 RTS 线索定位的原轮
    /// （[`EnvelopePreflight::consignment_rts`](crate::ports::consignment::EnvelopePreflight::consignment_rts)）。
    ///
    /// 来源只用于定位：只有领域判为“疑似系统 RTS”隔离时才改交退件接管，接管写端在锁内按持久派发事实
    /// 复判，复判不成立仍整封隔离（不变量 6）。没有来源时与 [`Self::plan`] 逐字一致。
    pub async fn plan_with_origin(
        &self,
        input: ConsignmentMailInput<'_>,
        origin: Option<TrustedConsignmentRts>,
    ) -> Result<ConsignmentMailPlan, ConsignmentError> {
        let ConsignmentMailInput {
            message,
            mail_key,
            mut observation,
            reservations,
            envelope_seller_user_id,
            capabilities,
            read_failure,
            existing_decision,
            now,
        } = input;
        if message.validate().is_err() {
            return Err(ConsignmentError::InvalidDepositRequest);
        }
        // D9 §2.2：先按 K 恢复已有决定；已有最终决定只恢复原动作，不因重读另定处置。
        if let Some(receipt) = existing_decision {
            if receipt.validate_shape().is_err() {
                return Err(ConsignmentError::Unavailable);
            }
            return Ok(ConsignmentMailPlan::Recovered(receipt));
        }

        // 勘误 E1：旧拍卖直达不以共享原信键为前提。可识别的原 owner 与规范结算退件主题
        // 必须先返回“不适用”，不能因为寄售入口而写任何寄售命令。
        if let Some(owner) = observation.legacy_auction_owner {
            return Ok(ConsignmentMailPlan::NotApplicable(
                ConsignmentNotApplicable::LegacyAuctionOwner(owner),
            ));
        }
        if system_returned_settlement_obligation_id(&message.sender, &message.subject).is_some() {
            return Ok(ConsignmentMailPlan::NotApplicable(
                ConsignmentNotApplicable::IdentifiableLegacySettlementReturn,
            ));
        }
        let suspected_rts = observation.system_rts || is_system_rts_message(message);
        if suspected_rts {
            // 不变量 6：列表主题与详情不一致时也只进共享隔离，不降级成普通寄件决定。
            observation.system_rts = true;
        }
        if !is_consign_subject(&message.subject)
            && !observation.subject_is_consign
            && !suspected_rts
        {
            return Ok(ConsignmentMailPlan::NotApplicable(
                ConsignmentNotApplicable::NotConsignmentSubject,
            ));
        }
        if let Some(key) = mail_key.as_ref()
            && key.world().as_str() != message.world_id
        {
            // 认证世界与入站邮箱世界不符时不能把“空信”或“接受”写进错误世界；
            // 保守地按身份未证进入隔离，仍由拍卖路由先行完成。
            observation.sender_identity = SenderIdentity::Unverified;
        }

        let facts = LockedEnvelopeFacts {
            mail_key,
            observation,
            reservations,
            envelope_seller_user_id,
            capabilities,
            read_failure,
            already_decided: false,
            now,
        };
        match decide_envelope_with_origin(&facts, origin) {
            EnvelopeRoute::LegacyAuction(owner) => Ok(ConsignmentMailPlan::NotApplicable(
                ConsignmentNotApplicable::LegacyAuctionOwner(owner),
            )),
            EnvelopeRoute::Pending(reason) => Ok(ConsignmentMailPlan::Pending(reason)),
            EnvelopeRoute::Consignment(_) => {
                let command = DepositCommandService::prepare_decide_envelope(facts, origin)?
                    .ok_or(ConsignmentError::Unavailable)?;
                Ok(ConsignmentMailPlan::First(Box::new(command)))
            }
        }
    }
}

fn is_system_rts_message(message: &MailMessage) -> bool {
    message.sender == "MoogleMail"
        && message.subject.get(..6).is_some_and(|prefix| prefix.eq_ignore_ascii_case("[RTS] "))
}

/// 集成入口的候选判定：普通 `consign` 或疑似系统 RTS。
///
/// 可识别的旧退款仍由 `process_message` 的系统退件分支先行处理；本函数只用于决定是否
/// 调用寄售分类器，不替代分类器内部的 E1 与旧 owner 检查。
pub fn is_consignment_candidate(message: &MailMessage) -> bool {
    is_consign_subject(&message.subject) || is_system_rts_message(message)
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::Arc;
    use std::task::{Context, Poll, Waker};

    use auction_domain::SettlementWorld;
    use auction_domain::UserId;
    use auction_domain::consignment::deposit::{
        EnvelopeCapabilities, EnvelopeItemKind, EnvelopeItemObservation, EnvelopeObservation,
        PendingReason, QuarantineReason, ReservationBinding, ReservationState, SenderIdentity,
    };
    use auction_domain::consignment::{
        LegacyAuctionOwner, ListingEquipmentIdentity, ListingState, ListingVersion, OriginalMailKey,
    };
    use auction_time::Timestamp;
    use uuid::Uuid;

    use super::{
        ConsignmentMailInput, ConsignmentMailOutcome, ConsignmentMailService,
        ConsignmentNotApplicable, is_consignment_candidate,
    };
    use crate::ports::consignment::deposit::test_support::FakeDepositStore;
    use crate::ports::consignment::{EnvelopeReceipt, ReceiptDisposition};
    use crate::ports::mail::MailMessage;
    use crate::services::consignment::DepositCommandService;

    const SELLER_UUID: u128 = 7;

    fn run<T>(future: impl Future<Output = T>) -> T {
        let mut future = std::pin::pin!(future);
        let mut context = Context::from_waker(Waker::noop());
        for _ in 0..64 {
            if let Poll::Ready(result) = future.as_mut().poll(&mut context) {
                return result;
            }
        }
        panic!("内存端口未在预期轮数内完成");
    }

    fn now() -> Timestamp {
        Timestamp::UNIX_EPOCH
    }

    fn seller() -> UserId {
        UserId::from_uuid(Uuid::from_u128(SELLER_UUID))
    }

    fn version(value: u64) -> ListingVersion {
        ListingVersion::new(value).expect("测试版本")
    }

    fn setup() -> (Arc<FakeDepositStore>, ConsignmentMailService) {
        let fake = Arc::new(FakeDepositStore::new());
        let commands = Arc::new(DepositCommandService::new(
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake.clone(),
        ));
        (fake, ConsignmentMailService::new(commands))
    }

    fn message(world: SettlementWorld, subject: &str, sender: &str) -> MailMessage {
        MailMessage {
            mailbox_key: format!("{}-mailbox", world.as_str()),
            external_mail_id: "100".to_owned(),
            world_id: world.as_str().to_owned(),
            received_at: now(),
            sender: sender.to_owned(),
            subject: subject.to_owned(),
            body: "test body".to_owned(),
            credits: 0,
            cod: 0,
            attachments: Vec::new(),
            returnable: true,
            claimable: true,
            content_digest: [7; 32],
        }
    }

    fn key(world: SettlementWorld, mid: &str) -> OriginalMailKey {
        OriginalMailKey::new("42", world, mid).expect("测试原信键")
    }

    fn observation(_world: SettlementWorld) -> EnvelopeObservation {
        EnvelopeObservation {
            observation_id: Uuid::from_u128(90),
            session_binding_digest: [1; 32],
            manifest_digest: [2; 32],
            parser_version: "consignment-test-parser".to_owned(),
            subject_is_consign: true,
            sender_identity: SenderIdentity::Verified,
            list_detail_consistent: true,
            asset_count: Some(0),
            duplicate_raw_eid: false,
            canonical_alias_ambiguous: false,
            unknown_attachment: false,
            credits: Some(0),
            cod: Some(0),
            returnable: true,
            takeable: Some(true),
            system_rts: false,
            legacy_auction_owner: None,
            items: Vec::new(),
        }
    }

    fn equipment(world: SettlementWorld, eid: &str, ordinal: u32) -> EnvelopeItemObservation {
        EnvelopeItemObservation {
            ordinal,
            kind: EnvelopeItemKind::Equipment,
            display_name: Some(format!("Fake Gear {eid}")),
            raw_eid: Some(eid.to_owned()),
            canonical_eid: Some(eid.to_owned()),
            amount: None,
            world: Some(world),
            seller_user_id: Some(seller()),
            tradeable: true,
            history_ok: true,
            protected_ref: Some(Uuid::from_u128(u128::from(ordinal) + 1_000)),
        }
    }

    fn binding(
        world: SettlementWorld,
        listing_id: Uuid,
        eid: &str,
        expires_at: Timestamp,
        reservation_id: Uuid,
    ) -> ReservationBinding {
        ReservationBinding {
            reservation_id,
            generation: 7,
            listing_id,
            listing_version: version(1),
            seller_user_id: seller(),
            equipment: ListingEquipmentIdentity::new(world.as_str(), eid).expect("测试装备身份"),
            expires_at,
            state: ReservationState::Active,
        }
    }

    fn expires_at() -> Timestamp {
        now() + auction_time::Duration::hours(1)
    }

    fn seed_accept(
        fake: &FakeDepositStore,
        world: SettlementWorld,
        listing_id: Uuid,
        eid: &str,
    ) -> ReservationBinding {
        fake.seed_listing(listing_id, seller(), eid, ListingState::AwaitingDeposit, 1, 100);
        let (reservation_id, _) = fake.seed_reservation(listing_id, seller(), eid, expires_at());
        binding(world, listing_id, eid, expires_at(), reservation_id)
    }

    fn assert_disposition(receipt: &EnvelopeReceipt, disposition: ReceiptDisposition) {
        assert_eq!(receipt.disposition, disposition);
        receipt.validate_shape().expect("回执形状");
    }

    #[test]
    fn two_world_observations_route_accept_return_archive_quarantine_and_pending() {
        for (index, world) in
            [SettlementWorld::Main, SettlementWorld::Isekai].into_iter().enumerate()
        {
            let (fake, service) = setup();
            let base = u128::try_from(index).expect("测试索引") * 100;
            let listing = Uuid::from_u128(base + 1);
            let eid = (base + 42).to_string();
            let reservation = seed_accept(&fake, world, listing, &eid);

            let mut accept_observation = observation(world);
            accept_observation.asset_count = Some(1);
            accept_observation.items = vec![equipment(world, &eid, 1)];
            let accepted = run(service.classify(ConsignmentMailInput {
                message: &message(world, "consign", "Seller"),
                mail_key: Some(key(world, &(base + 100).to_string())),
                observation: accept_observation,
                reservations: vec![reservation.clone()],
                envelope_seller_user_id: Some(seller()),
                capabilities: EnvelopeCapabilities { safe_original_return: true, take_ready: true },
                read_failure: false,
                existing_decision: None,
                now: now(),
            }))
            .expect("分类");
            let ConsignmentMailOutcome::Decided(receipt) = accepted else {
                panic!("应首次接受");
            };
            assert_disposition(&receipt, ReceiptDisposition::Accepted);
            assert_eq!(receipt.listing_changes.len(), 1);
            assert_eq!(
                fake.snapshot().listings.get(&listing).expect("挂单").state,
                ListingState::DepositAccepted
            );
            assert_eq!(
                fake.snapshot().reservations.get(&reservation.reservation_id).expect("预约").state,
                ReservationState::Consumed
            );

            let return_listing = Uuid::from_u128(base + 2);
            let unrelated_eid = (base + 43).to_string();
            fake.seed_listing(
                return_listing,
                seller(),
                &unrelated_eid,
                ListingState::AwaitingDeposit,
                1,
                100,
            );
            let (return_reservation, _) =
                fake.seed_reservation(return_listing, seller(), &unrelated_eid, expires_at());
            let mut return_observation = observation(world);
            return_observation.asset_count = Some(1);
            return_observation.items = vec![equipment(world, &(base + 99).to_string(), 1)];
            let returned = run(service.classify(ConsignmentMailInput {
                message: &message(world, "consign", "Seller"),
                mail_key: Some(key(world, &(base + 101).to_string())),
                observation: return_observation,
                reservations: Vec::new(),
                envelope_seller_user_id: Some(seller()),
                capabilities: EnvelopeCapabilities { safe_original_return: true, take_ready: true },
                read_failure: false,
                existing_decision: None,
                now: now(),
            }))
            .expect("分类");
            let ConsignmentMailOutcome::Decided(receipt) = returned else {
                panic!("应整封退回");
            };
            assert_disposition(&receipt, ReceiptDisposition::ReturnDecided);
            assert!(receipt.listing_changes.is_empty());
            assert_eq!(
                fake.snapshot().reservations.get(&return_reservation).expect("无关预约").state,
                ReservationState::Active
            );
            assert!(fake.snapshot().writes.is_empty());
            assert_eq!(fake.counters().permits_issued, 0);
            assert_eq!(fake.counters().posts_recorded, 0);

            let empty = run(service.classify(ConsignmentMailInput {
                message: &message(world, "consign", "Seller"),
                mail_key: Some(key(world, &(base + 102).to_string())),
                observation: observation(world),
                reservations: Vec::new(),
                envelope_seller_user_id: Some(seller()),
                capabilities: EnvelopeCapabilities { safe_original_return: true, take_ready: true },
                read_failure: false,
                existing_decision: None,
                now: now(),
            }))
            .expect("分类");
            let ConsignmentMailOutcome::Decided(receipt) = empty else {
                panic!("应空信归档");
            };
            assert_disposition(&receipt, ReceiptDisposition::ArchivedEmpty);
            assert!(receipt.listing_changes.is_empty());
            assert!(fake.snapshot().writes.is_empty());
            assert_eq!(fake.counters().permits_issued, 0);
            assert_eq!(fake.counters().posts_recorded, 0);

            let mut quarantined_observation = observation(world);
            quarantined_observation.asset_count = Some(1);
            quarantined_observation.duplicate_raw_eid = true;
            quarantined_observation.items = vec![equipment(world, &(base + 44).to_string(), 1)];
            let quarantined = run(service.classify(ConsignmentMailInput {
                message: &message(world, "consign", "Seller"),
                mail_key: Some(key(world, &(base + 103).to_string())),
                observation: quarantined_observation,
                reservations: Vec::new(),
                envelope_seller_user_id: Some(seller()),
                capabilities: EnvelopeCapabilities { safe_original_return: true, take_ready: true },
                read_failure: false,
                existing_decision: None,
                now: now(),
            }))
            .expect("分类");
            let ConsignmentMailOutcome::Decided(receipt) = quarantined else {
                panic!("应隔离");
            };
            assert_disposition(&receipt, ReceiptDisposition::Quarantined);
            let quarantine_id = receipt.quarantine.expect("隔离引用").quarantine_id;
            assert_eq!(
                fake.snapshot().quarantines.get(&quarantine_id).expect("隔离").reason,
                QuarantineReason::DuplicateRawEid
            );

            let ledger_before = fake.snapshot().ledger.len();
            let pending = run(service.classify(ConsignmentMailInput {
                message: &message(world, "consign", "Seller"),
                mail_key: Some(key(world, &(base + 104).to_string())),
                observation: observation(world),
                reservations: Vec::new(),
                envelope_seller_user_id: Some(seller()),
                capabilities: EnvelopeCapabilities { safe_original_return: true, take_ready: true },
                read_failure: true,
                existing_decision: None,
                now: now(),
            }))
            .expect("分类");
            assert_eq!(
                pending,
                ConsignmentMailOutcome::Pending(PendingReason::TransientReadFailure)
            );
            assert_eq!(fake.snapshot().ledger.len(), ledger_before);

            let missing = run(service.classify(ConsignmentMailInput {
                message: &message(world, "consign", "Seller"),
                mail_key: None,
                observation: observation(world),
                reservations: Vec::new(),
                envelope_seller_user_id: Some(seller()),
                capabilities: EnvelopeCapabilities { safe_original_return: true, take_ready: true },
                read_failure: false,
                existing_decision: None,
                now: now(),
            }))
            .expect("分类");
            assert_eq!(
                missing,
                ConsignmentMailOutcome::Pending(PendingReason::MissingAuthenticatedUid)
            );
            assert_eq!(fake.snapshot().ledger.len(), ledger_before);
            assert_eq!(fake.counters().permits_issued, 0);
            assert_eq!(fake.counters().posts_recorded, 0);
        }
    }

    #[test]
    fn e1_legacy_direct_continuation_and_identified_refund_never_write_consignment() {
        let (fake, service) = setup();
        let world = SettlementWorld::Main;
        let continuation = run(service.classify(ConsignmentMailInput {
            message: &message(world, "auction", "Seller"),
            mail_key: None,
            observation: EnvelopeObservation { subject_is_consign: false, ..observation(world) },
            reservations: Vec::new(),
            envelope_seller_user_id: Some(seller()),
            capabilities: EnvelopeCapabilities { safe_original_return: false, take_ready: false },
            read_failure: false,
            existing_decision: None,
            now: now(),
        }))
        .expect("分类");
        assert_eq!(
            continuation,
            ConsignmentMailOutcome::NotApplicable(ConsignmentNotApplicable::NotConsignmentSubject)
        );

        let refund = format!("[RTS] Auction buyer refund [{}]", Uuid::from_u128(77));
        let identified = run(service.classify(ConsignmentMailInput {
            message: &message(world, &refund, "MoogleMail"),
            // 勘误 E1：旧退款即使无法唯一映射 Bot UID，也不得进入寄售入口。
            mail_key: None,
            observation: EnvelopeObservation {
                subject_is_consign: false,
                system_rts: true,
                ..observation(world)
            },
            reservations: Vec::new(),
            envelope_seller_user_id: Some(seller()),
            capabilities: EnvelopeCapabilities { safe_original_return: false, take_ready: false },
            read_failure: false,
            existing_decision: None,
            now: now(),
        }))
        .expect("分类");
        assert_eq!(
            identified,
            ConsignmentMailOutcome::NotApplicable(
                ConsignmentNotApplicable::IdentifiableLegacySettlementReturn
            )
        );

        let legacy_owner = run(service.classify(ConsignmentMailInput {
            message: &message(world, "consign", "Seller"),
            mail_key: Some(key(world, "106")),
            observation: EnvelopeObservation {
                legacy_auction_owner: Some(LegacyAuctionOwner::BuyerRefund),
                ..observation(world)
            },
            reservations: Vec::new(),
            envelope_seller_user_id: Some(seller()),
            capabilities: EnvelopeCapabilities { safe_original_return: true, take_ready: true },
            read_failure: false,
            existing_decision: None,
            now: now(),
        }))
        .expect("分类");
        assert_eq!(
            legacy_owner,
            ConsignmentMailOutcome::NotApplicable(ConsignmentNotApplicable::LegacyAuctionOwner(
                LegacyAuctionOwner::BuyerRefund
            ))
        );
        assert!(fake.snapshot().ledger.is_empty());
        assert!(fake.snapshot().terminals.is_empty());
        assert!(fake.snapshot().quarantines.is_empty());
        assert_eq!(fake.counters().permits_issued, 0);
        assert_eq!(fake.counters().posts_recorded, 0);
    }

    #[test]
    fn application_legacy_owner_precedes_identified_refund_and_writes_nothing() {
        let (fake, service) = setup();
        let world = SettlementWorld::Main;
        let refund = format!("[RTS] Auction buyer refund [{}]", Uuid::from_u128(78));
        let outcome = run(service.classify(ConsignmentMailInput {
            message: &message(world, &refund, "MoogleMail"),
            mail_key: Some(key(world, "110")),
            observation: EnvelopeObservation {
                legacy_auction_owner: Some(LegacyAuctionOwner::BuyerRefund),
                ..observation(world)
            },
            reservations: Vec::new(),
            envelope_seller_user_id: Some(seller()),
            capabilities: EnvelopeCapabilities { safe_original_return: false, take_ready: false },
            read_failure: false,
            existing_decision: None,
            now: now(),
        }))
        .expect("分类");
        // 应用层必须先返回观察上的旧 owner，而不是把同一封信归成“可识别旧退款”。
        assert_eq!(
            outcome,
            ConsignmentMailOutcome::NotApplicable(ConsignmentNotApplicable::LegacyAuctionOwner(
                LegacyAuctionOwner::BuyerRefund
            ))
        );
        assert!(fake.snapshot().ledger.is_empty());
        assert!(fake.snapshot().terminals.is_empty());
        assert!(fake.snapshot().quarantines.is_empty());
        assert_eq!(fake.counters().permits_issued, 0);
        assert_eq!(fake.counters().posts_recorded, 0);
    }

    #[test]
    fn suspected_unowned_rts_quarantines_without_asset_work() {
        for (index, (subject, observation_system_rts)) in [
            ("[RTS] unknown system subject", false),
            ("[rts] unknown system subject", false),
            ("[RTS] unknown system subject", true),
        ]
        .into_iter()
        .enumerate()
        {
            let (fake, service) = setup();
            let world = SettlementWorld::Main;
            let mid = (107_u128 + u128::try_from(index).expect("测试索引")).to_string();
            let outcome = run(service.classify(ConsignmentMailInput {
                message: &message(world, subject, "MoogleMail"),
                mail_key: Some(key(world, &mid)),
                observation: EnvelopeObservation {
                    subject_is_consign: false,
                    system_rts: observation_system_rts,
                    ..observation(world)
                },
                reservations: Vec::new(),
                envelope_seller_user_id: Some(seller()),
                capabilities: EnvelopeCapabilities { safe_original_return: true, take_ready: true },
                read_failure: false,
                existing_decision: None,
                now: now(),
            }))
            .expect("分类");
            let ConsignmentMailOutcome::Decided(receipt) = outcome else {
                panic!("疑似 RTS 应隔离：{subject}");
            };
            assert_disposition(&receipt, ReceiptDisposition::Quarantined);
            let quarantine_id = receipt.quarantine.expect("隔离引用").quarantine_id;
            assert_eq!(
                fake.snapshot().quarantines.get(&quarantine_id).expect("隔离").reason,
                QuarantineReason::SuspectedSystemRts
            );
            assert_eq!(fake.snapshot().ledger.len(), 1);
            assert_eq!(fake.snapshot().terminals.len(), 1);
            assert_eq!(fake.counters().permits_issued, 0);
            assert_eq!(fake.counters().posts_recorded, 0);
        }
    }

    #[test]
    fn candidate_filter_accepts_consign_and_mooglemail_rts_only() {
        let world = SettlementWorld::Main;
        for subject in ["consign", "Consign", "CONSIGN", "cOnSiGn"] {
            assert!(
                is_consignment_candidate(&message(world, subject, "Seller")),
                "寄售主题应成为候选：{subject}"
            );
        }
        for subject in ["[RTS] Auction buyer refund [x]", "[rts] auction buyer refund [x]"] {
            assert!(
                is_consignment_candidate(&message(world, subject, "MoogleMail")),
                "系统 RTS 应成为候选：{subject}"
            );
        }
        assert!(
            !is_consignment_candidate(&message(world, "[RTS] Auction buyer refund [x]", "Seller")),
            "非系统发件人的 RTS 前缀不是寄售候选"
        );
        assert!(
            !is_consignment_candidate(&message(world, "auction", "MoogleMail")),
            "普通旧拍卖邮件不是寄售候选"
        );
        assert!(
            !is_consignment_candidate(&message(world, "hello", "Seller")),
            "普通邮件不是寄售候选"
        );
    }

    #[test]
    fn accept_requires_signed_take_readiness_and_leaves_reservation_active() {
        for (takeable, take_ready) in [(Some(true), false), (Some(false), true), (None, true)] {
            let (fake, service) = setup();
            let world = SettlementWorld::Main;
            let listing = Uuid::from_u128(300);
            let reservation = seed_accept(&fake, world, listing, "42");
            let mut observed = observation(world);
            observed.asset_count = Some(1);
            observed.takeable = takeable;
            observed.items = vec![equipment(world, "42", 1)];
            let outcome = run(service.classify(ConsignmentMailInput {
                message: &message(world, "consign", "Seller"),
                mail_key: Some(key(world, "108")),
                observation: observed,
                reservations: vec![reservation.clone()],
                envelope_seller_user_id: Some(seller()),
                capabilities: EnvelopeCapabilities { safe_original_return: true, take_ready },
                read_failure: false,
                existing_decision: None,
                now: now(),
            }))
            .expect("分类");
            let ConsignmentMailOutcome::Decided(receipt) = outcome else {
                panic!("未签收取得能力时应隔离");
            };
            assert_disposition(&receipt, ReceiptDisposition::Quarantined);
            let quarantine_id = receipt.quarantine.expect("隔离引用").quarantine_id;
            assert_eq!(
                fake.snapshot().quarantines.get(&quarantine_id).expect("隔离").reason,
                QuarantineReason::TakeUnproven
            );
            assert_eq!(
                fake.snapshot().reservations.get(&reservation.reservation_id).expect("预约").state,
                ReservationState::Active
            );
            assert_eq!(
                fake.snapshot().listings.get(&listing).expect("挂单").state,
                ListingState::AwaitingDeposit
            );
            assert_eq!(fake.counters().permits_issued, 0);
            assert_eq!(fake.counters().posts_recorded, 0);
        }
    }

    #[test]
    fn active_quarantine_same_key_replays_first_decision_and_never_creates_second() {
        let (fake, service) = setup();
        let world = SettlementWorld::Main;
        let mut observed = observation(world);
        observed.asset_count = Some(1);
        observed.unknown_attachment = true;
        observed.items = vec![equipment(world, "42", 1)];
        let mail = message(world, "consign", "Seller");
        let make_input = || ConsignmentMailInput {
            message: &mail,
            mail_key: Some(key(world, "109")),
            observation: observed.clone(),
            reservations: Vec::new(),
            envelope_seller_user_id: Some(seller()),
            capabilities: EnvelopeCapabilities { safe_original_return: true, take_ready: true },
            read_failure: false,
            existing_decision: None,
            now: now(),
        };
        let first = run(service.classify(make_input())).expect("首次隔离");
        let ConsignmentMailOutcome::Decided(first_receipt) = first else {
            panic!("应隔离");
        };
        let replay = run(service.classify(make_input())).expect("首回执重放");
        assert_eq!(replay, ConsignmentMailOutcome::Decided(first_receipt.clone()));
        assert_eq!(fake.snapshot().ledger.len(), 1);
        assert_eq!(fake.snapshot().terminals.len(), 1);
        assert_eq!(fake.snapshot().quarantines.len(), 1);

        let changed = EnvelopeObservation {
            unknown_attachment: false,
            items: vec![equipment(world, "42", 1)],
            ..observed.clone()
        };
        let conflicting =
            run(service.classify(ConsignmentMailInput { observation: changed, ..make_input() }))
                .expect("同 K 新证据");
        assert_eq!(conflicting, ConsignmentMailOutcome::Pending(PendingReason::ExistingDecision));
        assert_eq!(fake.snapshot().ledger.len(), 1);
        assert_eq!(fake.snapshot().terminals.len(), 1);
        assert_eq!(fake.snapshot().quarantines.len(), 1);

        let recovered = run(service.classify(ConsignmentMailInput {
            existing_decision: Some(first_receipt.clone()),
            ..make_input()
        }))
        .expect("恢复已有决定");
        assert_eq!(recovered, ConsignmentMailOutcome::Recovered(first_receipt));
        assert_eq!(fake.snapshot().ledger.len(), 1);
        assert_eq!(fake.snapshot().terminals.len(), 1);
        assert_eq!(fake.snapshot().quarantines.len(), 1);
    }

    #[test]
    fn command_id_conflict_is_pending_only_until_the_existing_decision_is_supplied() {
        let (fake, service) = setup();
        let world = SettlementWorld::Main;
        let mut observed = observation(world);
        observed.asset_count = Some(1);
        observed.unknown_attachment = true;
        observed.items = vec![equipment(world, "42", 1)];
        let mail = message(world, "consign", "Seller");
        let first = run(service.classify(ConsignmentMailInput {
            message: &mail,
            mail_key: Some(key(world, "111")),
            observation: observed.clone(),
            reservations: Vec::new(),
            envelope_seller_user_id: Some(seller()),
            capabilities: EnvelopeCapabilities { safe_original_return: true, take_ready: true },
            read_failure: false,
            existing_decision: None,
            now: now(),
        }))
        .expect("首次隔离");
        let ConsignmentMailOutcome::Decided(receipt) = first else {
            panic!("应首次隔离");
        };

        let changed = EnvelopeObservation { unknown_attachment: false, ..observed };
        let pending = run(service.classify(ConsignmentMailInput {
            message: &mail,
            mail_key: Some(key(world, "111")),
            observation: changed.clone(),
            reservations: Vec::new(),
            envelope_seller_user_id: Some(seller()),
            capabilities: EnvelopeCapabilities { safe_original_return: true, take_ready: true },
            read_failure: false,
            existing_decision: None,
            now: now(),
        }))
        .expect("同 K 新证据");
        assert_eq!(pending, ConsignmentMailOutcome::Pending(PendingReason::ExistingDecision));

        let recovered = run(service.classify(ConsignmentMailInput {
            message: &mail,
            mail_key: Some(key(world, "111")),
            observation: changed,
            reservations: Vec::new(),
            envelope_seller_user_id: Some(seller()),
            capabilities: EnvelopeCapabilities { safe_original_return: true, take_ready: true },
            read_failure: false,
            existing_decision: Some(receipt.clone()),
            now: now(),
        }))
        .expect("下一轮恢复");
        assert_eq!(recovered, ConsignmentMailOutcome::Recovered(receipt));
        assert_eq!(fake.snapshot().ledger.len(), 1);
        assert_eq!(fake.snapshot().terminals.len(), 1);
        assert_eq!(fake.snapshot().quarantines.len(), 1);
    }
}
