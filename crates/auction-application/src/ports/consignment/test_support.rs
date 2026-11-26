//! 寄售用例测试的进程内脚本替身，记录端口调用并返回预置结果。

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard};

use auction_domain::consignment::{FeeBps, ListingEquipmentIdentity};
use auction_domain::{CommandId, EquipmentId, EquipmentKey, EquipmentKeyEnvelope, UserId};
use auction_time::Timestamp;
use uuid::Uuid;

use crate::ports::admin::{AdminIntakeError, EquipmentKeySealerPort};
use crate::ports::consignment::commands::{
    AdminDelistListingCommand, ApplyListingLookupCommand, AutoCloseListingCommand,
    ListingAutoCloseEffect, ListingClosed, ListingCommandOutcome, ListingCommandRejection,
    ListingCommandStore, ListingLookupEffect, ListingPriceReceipt, ListingRegistered,
    ListingRegistrationPolicy, OwnedListingCommandReceipt, RegisterListingCommand,
    RegistrationInput, RegistrationSeller, RepriceListingCommand, WithdrawListingCommand,
};
use crate::ports::consignment::equipment::{
    ListingCredentialError, ListingEquipmentKeyOpener, ListingEquipmentLookup,
    ListingEquipmentLookupError, ListingEquipmentReference, ListingEquipmentUrlValidator,
    ListingLookupSnapshot, ListingRegistrationSecret, RegistrationReplayCrypto,
    RegistrationRequestMac, WrappedRegistrationReplayKey,
};
use crate::ports::consignment::errors::{
    ConsignmentError, ConsignmentFuture, ListingBusinessRejection, UnrecordedListingRejection,
};
use crate::ports::consignment::maintenance::{
    ListingAdmission, ListingMaintenance, ListingMaintenancePage,
};
use crate::ports::consignment::reads::{
    AdminListingDetail, AdminListingQuery, AdminListingRead, AdminListingRecord,
    AnonymousRegistrationReceipt, ListingReadPage, ListingRegistrationAvailability,
    OwnedListingRead, OwnedListingRecord, PrivateListingQuery, PublicListingDetail,
    PublicListingQuery, PublicListingRead, PublicListingSummary,
};
use crate::ports::consignment::work::{
    ListingLookupClaim, ListingWorkerReadiness, LookupAbandonment, LookupClaimRequest,
    LookupDispatch, LookupFence, LookupLeaseUpdate, LookupPermitStatus, LookupWorkStore,
};
use crate::ports::core::TransactionFuture;

/// 每种调用有独立 FIFO。未配置的调用视为测试失败，不能假装端口返回空数据。
pub(crate) struct Script<I, T>(Mutex<ScriptState<I, T>>);

struct ScriptState<I, T> {
    calls: Vec<I>,
    replies: VecDeque<T>,
}

impl<I, T> Default for Script<I, T> {
    fn default() -> Self {
        Self(Mutex::new(ScriptState { calls: Vec::new(), replies: VecDeque::new() }))
    }
}

impl<I, T> Script<I, T> {
    /// 为后续的一次调用预设返回值。
    pub(crate) fn reply(&self, reply: T) {
        self.state().replies.push_back(reply);
    }

    /// 取出观测到的全部调用参数，保留各端口方法的调用顺序。
    pub(crate) fn take_calls(&self) -> Vec<I> {
        std::mem::take(&mut self.state().calls)
    }

    fn call(&self, input: I) -> T {
        let mut state = self.state();
        state.calls.push(input);
        let Some(reply) = state.replies.pop_front() else {
            panic!("寄售用例调用了未配置返回值的端口");
        };
        reply
    }

    fn state(&self) -> MutexGuard<'_, ScriptState<I, T>> {
        let Ok(state) = self.0.lock() else {
            panic!("寄售替身脚本锁已中毒");
        };
        state
    }

    fn run<E, O>(&self, input: I) -> TransactionFuture<'static, O, E>
    where
        T: Into<Result<O, E>>,
        O: Send + 'static,
        E: Send + 'static,
    {
        Box::pin(std::future::ready(self.call(input).into()))
    }
}

type Reply<I, T> = Script<I, Result<T, ConsignmentError>>;
type CommandReply<I, T> = Reply<I, ListingCommandOutcome<T>>;
type OwnedQueries = Reply<(UserId, PrivateListingQuery), ListingReadPage<OwnedListingRecord>>;

/// 四个 HTTP 命令用例共用的非法审计关联号样本。
pub(crate) fn invalid_listing_request_ids() -> impl Iterator<Item = String> {
    [
        "fake\u{2028}request",
        "fake\u{2029}request",
        "fake-é",
        "fake request",
        "fake\trequest",
        "fake\nrequest",
        "",
        " fake-request",
        "fake-request ",
        " fake-request ",
        "fake:request",
    ]
    .into_iter()
    .map(str::to_owned)
    .chain(["r".repeat(65), "r".repeat(128), "r".repeat(129)])
}

/// 仅记录登记的安全参数和能力装配情况。
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RegistrationCall {
    pub command_id: CommandId,
    pub seller_user_id: Option<UserId>,
    pub seller_game_name: Option<String>,
    pub request_id: Option<String>,
    pub identity: ListingEquipmentIdentity,
    pub price: auction_domain::consignment::ListingPrice,
    pub mode: auction_domain::consignment::ListingMode,
    pub expected_fee_bps: auction_domain::consignment::FeeBps,
    pub listing_id: Uuid,
    pub policy: ListingRegistrationPolicy,
    pub sealer: bool,
    pub replay_crypto: bool,
}

/// 六类命令，可独立注入提交、拒绝、重放和不确定的错误。登记仅在首次决定时核对准入。
#[derive(Default)]
pub(crate) struct FakeListingCommands {
    pub register: CommandReply<RegistrationCall, ListingRegistered>,
    pub reprice: CommandReply<RepriceListingCommand, ListingPriceReceipt>,
    pub withdraw: CommandReply<WithdrawListingCommand, ListingClosed>,
    pub apply_lookup: CommandReply<ApplyListingLookupCommand, ListingLookupEffect>,
    pub auto_close: CommandReply<AutoCloseListingCommand, ListingAutoCloseEffect>,
    pub admin_delist: CommandReply<AdminDelistListingCommand, ListingClosed>,
}

impl ListingCommandStore for FakeListingCommands {
    fn register<'a>(
        &'a self,
        command: RegisterListingCommand<'a>,
        sealer: Option<&'a dyn EquipmentKeySealerPort>,
        replay_crypto: Option<&'a dyn RegistrationReplayCrypto>,
    ) -> ConsignmentFuture<'a, ListingCommandOutcome<ListingRegistered>> {
        let outcome = self.register.call(RegistrationCall {
            command_id: command.command_id,
            seller_user_id: match command.input.seller {
                RegistrationSeller::Account(user_id) => Some(user_id),
                RegistrationSeller::GameName(_) => None,
            },
            seller_game_name: match command.input.seller {
                RegistrationSeller::Account(_) => None,
                RegistrationSeller::GameName(name) => Some(name.as_str().to_owned()),
            },
            request_id: command.request_id,
            identity: command.input.identity.clone(),
            price: command.input.price,
            mode: command.input.mode,
            expected_fee_bps: command.input.expected_fee_bps,
            listing_id: command.listing_id,
            policy: command.policy,
            sealer: sealer.is_some(),
            replay_crypto: replay_crypto.is_some(),
        });
        Box::pin(std::future::ready(outcome.and_then(|outcome| {
            // 脚本模拟账本仲裁；重放、冲突及账本错误先返回，再检查首次命令的策略。
            if matches!(
                outcome,
                ListingCommandOutcome::Accepted(_)
                    | ListingCommandOutcome::Rejected(ListingCommandRejection::Recorded(_))
            ) {
                let policy = command.policy;
                if !policy.enabled {
                    return Ok(ListingCommandOutcome::Rejected(
                        ListingCommandRejection::Unrecorded(UnrecordedListingRejection::Disabled),
                    ));
                }
                if command.input.expected_fee_bps
                    != FeeBps::for_mode(command.input.mode, policy.consigned_fee_bps)
                {
                    return Ok(ListingCommandOutcome::Rejected(ListingCommandRejection::Recorded(
                        ListingBusinessRejection::FeeChanged,
                    )));
                }
                if sealer.is_none() || replay_crypto.is_none() {
                    return Err(ConsignmentError::Unavailable);
                }
            }
            Ok(outcome)
        })))
    }

    fn reprice(
        &self,
        command: RepriceListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingPriceReceipt>> {
        self.reprice.run(command)
    }

    fn withdraw(
        &self,
        command: WithdrawListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingClosed>> {
        self.withdraw.run(command)
    }

    fn apply_lookup(
        &self,
        command: ApplyListingLookupCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingLookupEffect>> {
        self.apply_lookup.run(command)
    }

    fn auto_close(
        &self,
        command: AutoCloseListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingAutoCloseEffect>> {
        self.auto_close.run(command)
    }

    fn admin_delist(
        &self,
        command: AdminDelistListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingClosed>> {
        self.admin_delist.run(command)
    }
}

/// 三种读权限的替身分别预置结果，私有调用记录会话卖家。
#[derive(Default)]
pub(crate) struct FakeListingReads {
    pub list_public: Reply<PublicListingQuery, ListingReadPage<PublicListingSummary>>,
    pub public_detail: Reply<Uuid, Option<PublicListingDetail>>,
    pub registration_availability: Reply<(), ListingRegistrationAvailability>,
    pub anonymous_command: Reply<CommandId, Option<AnonymousRegistrationReceipt>>,
    pub list_owned: OwnedQueries,
    pub owned_detail: Reply<(UserId, Uuid), Option<OwnedListingRecord>>,
    pub owned_command: Reply<(UserId, CommandId), Option<OwnedListingCommandReceipt>>,
    pub list_admin: Reply<AdminListingQuery, ListingReadPage<AdminListingRecord>>,
    pub admin_detail: Reply<Uuid, Option<AdminListingDetail>>,
}

impl PublicListingRead for FakeListingReads {
    fn list_public(
        &self,
        query: PublicListingQuery,
    ) -> ConsignmentFuture<'_, ListingReadPage<PublicListingSummary>> {
        self.list_public.run(query)
    }

    fn public_detail(
        &self,
        listing_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<PublicListingDetail>> {
        self.public_detail.run(listing_id)
    }

    fn registration_availability<'a>(
        &'a self,
        _replay_crypto: &'a dyn RegistrationReplayCrypto,
    ) -> ConsignmentFuture<'a, ListingRegistrationAvailability> {
        self.registration_availability.run(())
    }

    fn anonymous_command(
        &self,
        command_id: CommandId,
    ) -> ConsignmentFuture<'_, Option<AnonymousRegistrationReceipt>> {
        self.anonymous_command.run(command_id)
    }
}

impl OwnedListingRead for FakeListingReads {
    fn list_owned(
        &self,
        seller_user_id: UserId,
        query: PrivateListingQuery,
    ) -> ConsignmentFuture<'_, ListingReadPage<OwnedListingRecord>> {
        self.list_owned.run((seller_user_id, query))
    }

    fn owned_detail(
        &self,
        seller_user_id: UserId,
        listing_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<OwnedListingRecord>> {
        self.owned_detail.run((seller_user_id, listing_id))
    }

    fn owned_command(
        &self,
        seller_user_id: UserId,
        command_id: CommandId,
    ) -> ConsignmentFuture<'_, Option<OwnedListingCommandReceipt>> {
        self.owned_command.run((seller_user_id, command_id))
    }
}

impl AdminListingRead for FakeListingReads {
    fn list_admin(
        &self,
        query: AdminListingQuery,
    ) -> ConsignmentFuture<'_, ListingReadPage<AdminListingRecord>> {
        self.list_admin.run(query)
    }

    fn admin_detail(&self, listing_id: Uuid) -> ConsignmentFuture<'_, Option<AdminListingDetail>> {
        self.admin_detail.run(listing_id)
    }
}

/// 工作租约、许可及维护可以分别注入成功、拒绝和依赖故障。
#[derive(Default)]
pub(crate) struct FakeLookupWork {
    external_access_paused: Mutex<bool>,
    pub claim: Reply<LookupClaimRequest, Option<ListingLookupClaim>>,
    pub renew: Reply<LookupFence, LookupLeaseUpdate<Timestamp>>,
    pub confirm_dispatch: Reply<(LookupFence, bool), LookupDispatch>,
    pub abandon: Reply<(LookupFence, LookupAbandonment), LookupLeaseUpdate<()>>,
    pub reap_expired: Reply<(), u32>,
    pub permit_status: Reply<(), LookupPermitStatus>,
    pub record_worker_readiness: Reply<(u64, ListingWorkerReadiness), ()>,
    pub scan_candidates: Reply<(), ListingMaintenancePage>,
    pub sync_admission: Reply<bool, ListingAdmission>,
}

impl FakeLookupWork {
    /// 模拟由 Worker 预检之外的写入方更新持久暂停事实。
    pub(crate) fn set_external_access_paused(&self, paused: bool) {
        *self.pause_state() = paused;
    }

    fn pause_state(&self) -> MutexGuard<'_, bool> {
        let Ok(state) = self.external_access_paused.lock() else {
            panic!("寄售替身暂停锁已中毒");
        };
        state
    }
}

impl LookupWorkStore for FakeLookupWork {
    fn claim(
        &self,
        request: LookupClaimRequest,
    ) -> ConsignmentFuture<'_, Option<ListingLookupClaim>> {
        Box::pin(async move {
            // 同一锁内复核暂停并消费预置领取结果，暂停只记录调用，保留下一次领取。
            let paused = self.pause_state();
            if *paused {
                self.claim.state().calls.push(request);
                return Ok(None);
            }
            self.claim.call(request)
        })
    }

    fn renew(&self, fence: LookupFence) -> ConsignmentFuture<'_, LookupLeaseUpdate<Timestamp>> {
        self.renew.run(fence)
    }

    fn confirm_dispatch<'a>(
        &'a self,
        claim: &'a ListingLookupClaim,
        paused: bool,
    ) -> ConsignmentFuture<'a, LookupDispatch> {
        self.confirm_dispatch.run((claim.fence(), paused))
    }

    fn abandon(
        &self,
        fence: LookupFence,
        status: LookupAbandonment,
    ) -> ConsignmentFuture<'_, LookupLeaseUpdate<()>> {
        self.abandon.run((fence, status))
    }

    fn reap_expired(&self) -> ConsignmentFuture<'_, u32> {
        self.reap_expired.run(())
    }

    fn permit_status(&self) -> ConsignmentFuture<'_, LookupPermitStatus> {
        self.permit_status.run(())
    }

    fn record_worker_readiness(
        &self,
        service_epoch: u64,
        readiness: ListingWorkerReadiness,
    ) -> ConsignmentFuture<'_, ()> {
        self.record_worker_readiness.run((service_epoch, readiness))
    }
}

impl ListingMaintenance for FakeLookupWork {
    fn scan_candidates(&self) -> ConsignmentFuture<'_, ListingMaintenancePage> {
        self.scan_candidates.run(())
    }

    fn sync_admission(&self, enabled: bool) -> ConsignmentFuture<'_, ListingAdmission> {
        self.sync_admission.run(enabled)
    }
}

type CredentialReply<I, T> = Script<I, Result<T, ListingCredentialError>>;
type SourceReply = Script<
    (ListingEquipmentIdentity, Timestamp),
    Result<ListingLookupSnapshot, ListingEquipmentLookupError>,
>;

/// URL、查装备及密码操作的替身。调用记录只保存安全身份和截止，不保存输入凭据。
#[derive(Default)]
pub(crate) struct FakeListingEquipment {
    pub url: Reply<(), ListingEquipmentReference>,
    pub lookup: SourceReply,
    pub seal: Script<EquipmentId, Result<EquipmentKeyEnvelope, AdminIntakeError>>,
    pub open: CredentialReply<EquipmentId, EquipmentKey>,
    pub generate_wrapped: CredentialReply<Uuid, WrappedRegistrationReplayKey>,
    pub check_wrapped: CredentialReply<Uuid, ()>,
    pub registration_mac: CredentialReply<Uuid, RegistrationRequestMac>,
    pub rewrap: CredentialReply<Uuid, WrappedRegistrationReplayKey>,
}

impl ListingEquipmentUrlValidator for FakeListingEquipment {
    fn validate(
        &self,
        _equipment_url: &str,
    ) -> Result<ListingEquipmentReference, ConsignmentError> {
        self.url.call(())
    }
}

impl ListingEquipmentLookup for FakeListingEquipment {
    fn lookup<'a>(
        &'a self,
        identity: &'a ListingEquipmentIdentity,
        _key: &'a EquipmentKey,
        deadline: Timestamp,
    ) -> TransactionFuture<'a, ListingLookupSnapshot, ListingEquipmentLookupError> {
        self.lookup.run((identity.clone(), deadline))
    }
}

impl EquipmentKeySealerPort for FakeListingEquipment {
    fn seal(
        &self,
        equipment_id: EquipmentId,
        _key: &EquipmentKey,
    ) -> Result<EquipmentKeyEnvelope, AdminIntakeError> {
        self.seal.call(equipment_id)
    }
}

impl ListingEquipmentKeyOpener for FakeListingEquipment {
    fn open(
        &self,
        secret: &ListingRegistrationSecret,
    ) -> Result<EquipmentKey, ListingCredentialError> {
        self.open.call(secret.aad_equipment_id())
    }
}

impl RegistrationReplayCrypto for FakeListingEquipment {
    fn generate_wrapped(
        &self,
        replay_key_id: Uuid,
    ) -> Result<WrappedRegistrationReplayKey, ListingCredentialError> {
        self.generate_wrapped.call(replay_key_id)
    }

    fn check_wrapped(
        &self,
        replay_key_id: Uuid,
        _wrapped: &WrappedRegistrationReplayKey,
    ) -> Result<(), ListingCredentialError> {
        self.check_wrapped.call(replay_key_id)
    }

    fn registration_mac(
        &self,
        replay_key_id: Uuid,
        _wrapped: &WrappedRegistrationReplayKey,
        _input: &RegistrationInput<'_>,
    ) -> Result<RegistrationRequestMac, ListingCredentialError> {
        self.registration_mac.call(replay_key_id)
    }

    fn rewrap(
        &self,
        replay_key_id: Uuid,
        _wrapped: &WrappedRegistrationReplayKey,
    ) -> Result<WrappedRegistrationReplayKey, ListingCredentialError> {
        self.rewrap.call(replay_key_id)
    }
}
