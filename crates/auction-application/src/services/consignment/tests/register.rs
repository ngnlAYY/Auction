//! 登记入参、准入策略、凭证边界及原回执验证。

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use auction_domain::consignment::{FeeBps, ListingMode};
use auction_domain::{CommandId, EquipmentKey, UserId};
use uuid::Uuid;

use crate::ports::admin::EquipmentKeySealerPort;
use crate::ports::consignment::test_support::{FakeListingCommands, FakeListingEquipment};
use crate::ports::consignment::{
    AdminDelistListingCommand, ApplyListingLookupCommand, AutoCloseListingCommand,
    ConsignmentError, ConsignmentFuture, ListingAutoCloseEffect, ListingBusinessRejection,
    ListingClosed, ListingCommandOutcome, ListingCommandRejection, ListingCommandStore,
    ListingEquipmentReference, ListingLookupEffect, ListingPriceReceipt, ListingRegistered,
    ListingRegistrationPolicy, ListingWorlds, RegisterListingCommand, RegistrationReplayCrypto,
    RepriceListingCommand, StoredListingDecision, UnrecordedListingRejection,
    WithdrawListingCommand,
};
use crate::services::consignment::tests::{
    FAKE_KEY, FAKE_URL, LISTING_ID, SELLER_ID, command_id, identity, price, run, seller,
};
use crate::services::consignment::{RegisterListingInput, RegisterListingService};

struct RegistrationFixture {
    store: Arc<DebugRegistrationStore>,
    equipment: Arc<FakeListingEquipment>,
    uuid_calls: Arc<AtomicU32>,
}

impl RegistrationFixture {
    fn new() -> Self {
        Self {
            store: Arc::new(DebugRegistrationStore::default()),
            equipment: Arc::new(FakeListingEquipment::default()),
            uuid_calls: Arc::new(AtomicU32::new(0)),
        }
    }

    fn service(&self, enabled: bool, capable: bool) -> RegisterListingService {
        let uuid_calls = self.uuid_calls.clone();
        RegisterListingService::new(
            self.store.clone(),
            self.equipment.clone(),
            Some(self.equipment.clone()),
            Some(self.equipment.clone()),
            policy(enabled),
            capable,
            move || Uuid::from_u128(100 + u128::from(uuid_calls.fetch_add(1, Ordering::Relaxed))),
        )
    }

    fn url_ok(&self) {
        self.equipment.url.reply(Ok(ListingEquipmentReference {
            identity: identity(),
            key: EquipmentKey::new(FAKE_KEY).unwrap(),
        }));
    }
}

fn policy(enabled: bool) -> ListingRegistrationPolicy {
    ListingRegistrationPolicy {
        enabled,
        worlds: ListingWorlds { main: false, isekai: true },
        consigned_fee_bps: FeeBps::new(615).unwrap(),
        max_open_per_user: 24,
        registrations_per_hour: 9,
    }
}

fn input() -> RegisterListingInput<'static> {
    RegisterListingInput {
        command_id: command_id(),
        mode: ListingMode::Consigned,
        equipment_url: FAKE_URL,
        price: 101,
        expected_fee_bps: 615,
    }
}

// 2P 的调用记录会先移除 key；额外窥测真实命令的 Debug，但不保存或输出调试串。
#[derive(Default)]
struct DebugRegistrationStore(FakeListingCommands);

impl ListingCommandStore for DebugRegistrationStore {
    fn register<'a>(
        &'a self,
        command: RegisterListingCommand<'a>,
        sealer: Option<&'a dyn EquipmentKeySealerPort>,
        replay_crypto: Option<&'a dyn RegistrationReplayCrypto>,
    ) -> ConsignmentFuture<'a, ListingCommandOutcome<ListingRegistered>> {
        let safe = format!("{command:?}");
        assert!(!safe.contains(FAKE_KEY), "登记命令 Debug 泄露测试凭证");
        assert!(!safe.contains(FAKE_URL), "登记命令 Debug 泄露测试 URL");
        assert_ne!(command.aad_equipment_id.as_uuid(), command.listing_id);
        assert_ne!(command.aad_equipment_id.as_uuid(), command.work_id);
        assert_ne!(command.aad_equipment_id.as_uuid(), command.candidate_replay_key_id);
        self.0.register(command, sealer, replay_crypto)
    }

    fn reprice(
        &self,
        command: RepriceListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingPriceReceipt>> {
        self.0.reprice(command)
    }

    fn withdraw(
        &self,
        command: WithdrawListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingClosed>> {
        self.0.withdraw(command)
    }

    fn admin_delist(
        &self,
        command: AdminDelistListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingClosed>> {
        self.0.admin_delist(command)
    }

    fn apply_lookup(
        &self,
        command: ApplyListingLookupCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingLookupEffect>> {
        self.0.apply_lookup(command)
    }

    fn auto_close(
        &self,
        command: AutoCloseListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingAutoCloseEffect>> {
        self.0.auto_close(command)
    }
}

#[test]
fn register_checks_policy_and_redacts_command() {
    let fixture = RegistrationFixture::new();
    let service = fixture.service(true, true);
    let receipt = ListingRegistered::new(LISTING_ID, price(101)).unwrap();
    for (mode, fee, outcome) in [
        (ListingMode::Consigned, 615, ListingCommandOutcome::Accepted(receipt)),
        (
            ListingMode::Consigned,
            615,
            ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(receipt)),
        ),
        (ListingMode::Direct, 0, ListingCommandOutcome::Accepted(receipt)),
    ] {
        let command_id = if mode == ListingMode::Direct {
            CommandId::from_uuid(Uuid::from_u128(90))
        } else {
            command_id()
        };
        fixture.url_ok();
        fixture.store.0.register.reply(Ok(outcome));
        assert_eq!(
            run(service.register(
                &seller(),
                RegisterListingInput { command_id, mode, expected_fee_bps: fee, ..input() }
            )),
            Ok(receipt)
        );
        let [call] = fixture.store.0.register.take_calls().try_into().unwrap();
        assert_eq!(call.command_id, command_id);
        assert_eq!(call.seller_user_id, UserId::from_uuid(SELLER_ID));
        assert_eq!(call.identity, identity());
        assert_eq!(call.mode, mode);
        assert_eq!(call.price, price(101));
        assert_eq!(call.expected_fee_bps.value(), fee as u16);
        assert_eq!(call.policy, policy(true));
        assert!(call.sealer && call.replay_crypto);
        assert_ne!(call.listing_id, receipt.listing_id());
    }
    assert_eq!(fixture.equipment.url.take_calls().len(), 3);
    assert_eq!(fixture.uuid_calls.load(Ordering::Relaxed), 12);
    assert!(fixture.equipment.seal.take_calls().is_empty());
    assert!(fixture.equipment.registration_mac.take_calls().is_empty());
}

#[test]
fn register_closed_or_unsupported_never_touches_dependencies() {
    for (enabled, capable) in [(false, true), (true, false), (false, false)] {
        let fixture = RegistrationFixture::new();
        let service = fixture.service(enabled, capable);
        assert_eq!(run(service.register(&seller(), input())), Err(ConsignmentError::Disabled));
        assert!(fixture.equipment.url.take_calls().is_empty());
        assert!(fixture.equipment.seal.take_calls().is_empty());
        assert!(fixture.equipment.registration_mac.take_calls().is_empty());
        assert!(fixture.store.0.register.take_calls().is_empty());
        assert_eq!(fixture.uuid_calls.load(Ordering::Relaxed), 0);
    }
}

#[test]
fn register_stops_on_bad_url_before_credential_work() {
    let fixture = RegistrationFixture::new();
    fixture.equipment.url.reply(Err(ConsignmentError::InvalidEquipmentUrl));
    let result = run(fixture
        .service(true, true)
        .register(&seller(), RegisterListingInput { price: 0, ..input() }));
    assert_eq!(result, Err(ConsignmentError::InvalidEquipmentUrl));
    assert_eq!(fixture.equipment.url.take_calls().len(), 1);
    assert!(fixture.equipment.seal.take_calls().is_empty());
    assert!(fixture.equipment.registration_mac.take_calls().is_empty());
    assert!(fixture.store.0.register.take_calls().is_empty());
    assert_eq!(fixture.uuid_calls.load(Ordering::Relaxed), 0);
}

#[test]
fn register_stops_on_invalid_price_fee_or_identity() {
    use ListingBusinessRejection::FeeChanged;

    let fixture = RegistrationFixture::new();
    let service = fixture.service(true, true);
    for (input, error) in [
        (
            RegisterListingInput { command_id: CommandId::from_uuid(Uuid::nil()), ..input() },
            ConsignmentError::InvalidCommandId,
        ),
        (RegisterListingInput { price: 0, ..input() }, ConsignmentError::InvalidPrice),
        (
            RegisterListingInput { expected_fee_bps: 10_000, ..input() },
            ConsignmentError::InvalidListingRequest,
        ),
        (RegisterListingInput { expected_fee_bps: 500, ..input() }, FeeChanged.into()),
        (RegisterListingInput { mode: ListingMode::Direct, ..input() }, FeeChanged.into()),
    ] {
        fixture.url_ok();
        assert_eq!(run(service.register(&seller(), input)), Err(error));
    }
    let mut invalid_seller = seller();
    invalid_seller.user_id = Uuid::nil();
    fixture.url_ok();
    assert_eq!(
        run(service.register(&invalid_seller, input())),
        Err(ConsignmentError::InvalidListingRequest)
    );
    assert_eq!(fixture.equipment.url.take_calls().len(), 6);
    assert!(fixture.equipment.seal.take_calls().is_empty());
    assert!(fixture.store.0.register.take_calls().is_empty());
    assert_eq!(fixture.uuid_calls.load(Ordering::Relaxed), 0);
}

#[test]
fn register_maps_first_and_replayed_rejections() {
    use ListingBusinessRejection::{
        AlreadyOpen, EquipmentBusy, FeeChanged, HistoryAliasConflict, HistoryOwnerMismatch,
        HistoryUnproven, ListingLimitReached, ParticipantIneligible, RegistrationRateLimited,
        WorldUnavailable,
    };

    let fixture = RegistrationFixture::new();
    let service = fixture.service(true, true);
    for (rejection, expected) in [
        (ParticipantIneligible, (403, "participant_ineligible")),
        (AlreadyOpen, (409, "already_open")),
        (EquipmentBusy, (409, "equipment_busy")),
        (HistoryUnproven, (409, "history_unproven")),
        (HistoryOwnerMismatch, (409, "history_owner_mismatch")),
        (HistoryAliasConflict, (409, "history_alias_conflict")),
        (FeeChanged, (409, "fee_changed")),
        (WorldUnavailable, (409, "world_unavailable")),
        (RegistrationRateLimited, (429, "registration_rate_limited")),
        (ListingLimitReached, (429, "listing_limit_reached")),
    ] {
        for outcome in [
            ListingCommandOutcome::Rejected(ListingCommandRejection::Recorded(rejection)),
            ListingCommandOutcome::Replayed(StoredListingDecision::Rejected(rejection)),
        ] {
            fixture.url_ok();
            fixture.store.0.register.reply(Ok(outcome));
            assert_eq!(
                run(service.register(&seller(), input()))
                    .err()
                    .and_then(ConsignmentError::api_error),
                Some(expected)
            );
        }
    }
    for (rejection, expected) in [
        (UnrecordedListingRejection::CommandIdConflict, (409, "command_id_conflict")),
        (UnrecordedListingRejection::Disabled, (404, "consignment_disabled")),
    ] {
        fixture.url_ok();
        fixture.store.0.register.reply(Ok(ListingCommandOutcome::Rejected(
            ListingCommandRejection::Unrecorded(rejection),
        )));
        assert_eq!(
            run(service.register(&seller(), input())).err().and_then(ConsignmentError::api_error),
            Some(expected)
        );
    }
    assert_eq!(fixture.store.0.register.take_calls().len(), 22);
}

#[test]
fn register_preserves_safe_port_failure_without_retry() {
    let fixture = RegistrationFixture::new();
    let service = fixture.service(true, true);
    for error in [ConsignmentError::Unavailable, ConsignmentError::OutcomeUnknown] {
        fixture.url_ok();
        fixture.store.0.register.reply(Err(error));
        assert_eq!(run(service.register(&seller(), input())), Err(error));
    }
    assert_eq!(fixture.store.0.register.take_calls().len(), 2);
}
