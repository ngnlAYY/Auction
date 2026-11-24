//! v1 账本与读模型的兼容性：计数、详情和原命令重放都不得产生 v2 事实。

use auction_application::consignment::{
    ListingCommandOutcome, ListingCommandStore, ListingCredentialError, ListingOperation,
    ListingPageWindow, OwnedListingRead, PrivateListingQuery, RegisterListingCommand,
    RegistrationInput, RegistrationReplayCrypto, RegistrationRequestMac, StoredListingDecision,
    WrappedRegistrationReplayKey,
};
use auction_domain::consignment::{
    FeeBps, ListingEquipmentIdentity, ListingMode, ListingPrice, ListingState,
};
use auction_domain::{CommandId, EquipmentId, EquipmentKey, UserId};
use sqlx::PgPool;
use uuid::Uuid;

use super::support::{LISTING, REGISTER, REPLAY, START, USER_1, succeeds};
use crate::support::TestDatabase;

const V1_DIGEST: [u8; 32] = [0x15; 32];

#[derive(Default)]
struct FixedReplayCrypto;

impl RegistrationReplayCrypto for FixedReplayCrypto {
    fn generate_wrapped(
        &self,
        _key_id: Uuid,
    ) -> Result<WrappedRegistrationReplayKey, ListingCredentialError> {
        WrappedRegistrationReplayKey::new(vec![0x13; 48], [0x14; 12], "fake-data-key".to_owned())
    }

    fn check_wrapped(
        &self,
        _key_id: Uuid,
        _wrapped: &WrappedRegistrationReplayKey,
    ) -> Result<(), ListingCredentialError> {
        Ok(())
    }

    fn registration_mac(
        &self,
        _key_id: Uuid,
        _wrapped: &WrappedRegistrationReplayKey,
        _input: &RegistrationInput<'_>,
    ) -> Result<RegistrationRequestMac, ListingCredentialError> {
        Ok(RegistrationRequestMac::new(V1_DIGEST))
    }

    fn rewrap(
        &self,
        _key_id: Uuid,
        wrapped: &WrappedRegistrationReplayKey,
    ) -> Result<WrappedRegistrationReplayKey, ListingCredentialError> {
        Ok(wrapped.clone())
    }
}

/// 只写一条 v1 登记命令的挂单；列表与详情读取所需的快照和准入一并写入。
async fn seed_v1(pool: &PgPool) {
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.users (id, username, public_key_hash)
             VALUES ('{USER_1}', 'FakeSellerOne', decode(repeat('11', 32), 'hex'));
             INSERT INTO public.consignment_replay_keys
                (id, write_current, wrapped_key, nonce, encryption_key_id, created_at, updated_at)
             VALUES ('{REPLAY}', true, decode(repeat('13', 48), 'hex'),
                     decode(repeat('14', 12), 'hex'), 'fake-data-key', '{START}', '{START}');
             INSERT INTO public.consignment_listings
                (id, seller_user_id, world_id, external_equipment_id, mode, state, price,
                 pricing_version, fee_bps, state_version, service_epoch,
                 blacklist_count_at_register, blacklist_baseline_ids, created_at, updated_at,
                 expires_at)
             VALUES ('{LISTING}', '{USER_1}', 'main', '42', 'consigned', 'resolving', 101,
                     1, 500, 1, 1, 0, ARRAY[]::uuid[], '{START}', '{START}', '2099-01-01T00:00:00Z');
             INSERT INTO public.consignment_commands
                (command_id, actor_kind, actor_ref, operation, request_target, request_digest,
                 replay_key_id, listing_id, result_status, result_code, result_body, created_at)
             VALUES ('{REGISTER}', 'public_user', '{USER_1}', 'register', 'new',
                     decode(repeat('15', 32), 'hex'), '{REPLAY}', '{LISTING}', 202, 'accepted',
                     '{{\"command_id\":\"{REGISTER}\",\"operation\":\"register\",
                        \"listing_id\":\"{LISTING}\",\"listing_version\":1,
                        \"state\":\"resolving\",\"disposition\":\"registered\",\"price\":101}}',
                     '{START}');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', 1, '{REGISTER}', 'public_user', 'registered', NULL,
                     'resolving', NULL, 101, '{START}');
             INSERT INTO public.consignment_lookup_work
                (id, listing_id, generation, state, attempt_count, fence, next_attempt_at,
                 created_at, updated_at)
             VALUES ('86000000-0000-0000-0000-000000000001', '{LISTING}', 1, 'queued', 0, 0,
                     '{START}', '{START}', '{START}');
             UPDATE public.consignment_lookup_control
             SET admitting=true, enabled_epoch=1,
                 worker_ready_until=clock_timestamp() + interval '1 hour'
             WHERE id=1;"
        ),
    )
    .await;
}

fn replay_command() -> RegisterListingCommand<'static> {
    static GEAR: std::sync::OnceLock<ListingEquipmentIdentity> = std::sync::OnceLock::new();
    static KEY: std::sync::OnceLock<EquipmentKey> = std::sync::OnceLock::new();
    let gear = GEAR.get_or_init(|| ListingEquipmentIdentity::new("main", "42").expect("装备"));
    let key = KEY.get_or_init(|| EquipmentKey::new("01234abcde").expect("假 key"));
    RegisterListingCommand {
        request_id: None,
        command_id: CommandId::from_uuid(Uuid::parse_str(REGISTER).expect("命令")),
        seller_user_id: UserId::from_uuid(Uuid::parse_str(USER_1).expect("卖家")),
        input: RegistrationInput {
            identity: gear,
            key,
            mode: ListingMode::Consigned,
            price: ListingPrice::new(101).expect("价格"),
            expected_fee_bps: FeeBps::new(500).expect("费率"),
        },
        listing_id: Uuid::parse_str(LISTING).expect("挂单"),
        work_id: Uuid::new_v4(),
        aad_equipment_id: EquipmentId::from_uuid(Uuid::new_v4()),
        candidate_replay_key_id: Uuid::new_v4(),
        policy: auction_application::consignment::ListingRegistrationPolicy {
            enabled: false,
            worlds: auction_application::consignment::ListingWorlds { main: true, isekai: true },
            consigned_fee_bps: FeeBps::new(500).expect("费率"),
            max_open_per_user: 20,
            registrations_per_hour: 10,
        },
    }
}

#[tokio::test]
async fn postgres_deposit_v1_count_detail_and_replay_are_compatible() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let owner = database.maintenance_store().await.expect("维护账号");
    seed_v1(owner.postgres_pool()).await;
    let before_body: serde_json::Value = sqlx::query_scalar(
        "SELECT result_body FROM public.consignment_commands WHERE command_id=$1",
    )
    .bind(Uuid::parse_str(REGISTER).expect("命令"))
    .fetch_one(store.postgres_pool())
    .await
    .expect("读取原回执");

    let seller = UserId::from_uuid(Uuid::parse_str(USER_1).expect("卖家"));
    let query = PrivateListingQuery {
        window: ListingPageWindow::new(Some(1), Some(20)).expect("分页"),
        world: None,
        mode: None,
        state: None,
    };
    let page = store.list_owned(seller, query).await.expect("读取 v1 列表");
    assert_eq!(page.total, 1);
    assert_eq!(page.items.len(), 1);
    let detail = store
        .owned_detail(seller, Uuid::parse_str(LISTING).expect("挂单"))
        .await
        .expect("读取 v1 详情")
        .expect("v1 挂单存在");
    assert_eq!(detail.listing().listing_id, Uuid::parse_str(LISTING).expect("挂单"));
    assert_eq!(detail.listing().price, ListingPrice::new(101).expect("价格"));
    assert_eq!(detail.listing().view.mode(), ListingMode::Consigned);
    assert_eq!(detail.listing().view.stored_state(), ListingState::Resolving);
    assert_eq!(detail.listing().view.effective_state(), (ListingState::Resolving, None));

    let crypto = FixedReplayCrypto;
    let replay =
        store.register(replay_command(), None, Some(&crypto)).await.expect("原样重放 v1 命令");
    let ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(registered)) = replay
    else {
        panic!("v1 命令必须返回原登记回执：{replay:?}");
    };
    assert_eq!(registered.listing_id(), Uuid::parse_str(LISTING).expect("挂单"));
    assert_eq!(registered.price, ListingPrice::new(101).expect("价格"));

    let receipt = store
        .owned_command(seller, CommandId::from_uuid(Uuid::parse_str(REGISTER).expect("命令")))
        .await
        .expect("读取 v1 账本")
        .expect("v1 命令存在");
    assert_eq!(receipt.decision.operation(), ListingOperation::Register);
    assert_eq!(receipt.decision.status_and_code(), (202, "accepted"));

    let after_body: serde_json::Value = sqlx::query_scalar(
        "SELECT result_body FROM public.consignment_commands WHERE command_id=$1",
    )
    .bind(Uuid::parse_str(REGISTER).expect("命令"))
    .fetch_one(store.postgres_pool())
    .await
    .expect("读取重放后的回执");
    assert_eq!(before_body, after_body, "v1 回执字节不得被重放改写");
    assert!(after_body.get("schema_version").is_none(), "v1 回执不得出现 v2 形状");

    let facts: (i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
            (SELECT count(*) FROM public.consignment_commands),
            (SELECT count(*) FROM public.consignment_commands WHERE schema_version <> 1),
            (SELECT count(*) FROM public.consignment_deposit_reservations),
            (SELECT count(*) FROM public.mail_envelope_observations),
            (SELECT count(*) FROM public.mail_disposition_work),
            (SELECT count(*) FROM public.mail_initial_decisions),
            (SELECT count(*) FROM public.mail_final_dispositions)",
    )
    .fetch_one(store.postgres_pool())
    .await
    .expect("读取事实数");
    assert_eq!(facts, (1, 0, 0, 0, 0, 0, 0), "重放不得写出任何 v2 事实");
    let v2_deliveries: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.deliveries WHERE origin_kind='consignment_disposition'",
    )
    .fetch_one(store.postgres_pool())
    .await
    .expect("读取寄售投递");
    assert_eq!(v2_deliveries, 0);
    let lookup_work: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.consignment_lookup_work")
            .fetch_one(store.postgres_pool())
            .await
            .expect("读取解析工作");
    assert_eq!(lookup_work, 1, "重放不得重新排队 v1 解析工作");
}
