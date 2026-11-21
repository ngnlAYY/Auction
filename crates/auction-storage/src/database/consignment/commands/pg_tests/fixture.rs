//! 寄售事务测试使用的最小临时数据库夹具。

use auction_application::consignment::{
    ListingBusinessRejection, ListingOperation, RegistrationRequestMac,
};
use auction_application::core::RequestHash;
use auction_domain::consignment::{
    FeeBps, ListingEquipmentIdentity, ListingMode, ListingPrice, ListingSnapshot, ListingState,
    ListingVersion,
};
use auction_time::Timestamp;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::facts::{ChangedOperation, Decision};
use crate::database::consignment::commands::ledger::{
    Actor, Arbitration, RegistrationProof, RequestProof, StoredReceipt, Target,
};
use crate::database::consignment::commands::session::{LockKeys, with_command_transaction};
use crate::database::consignment::listings::projection::{ListingLockIdentity, NewListing};

pub(super) const SELLER: Uuid = Uuid::from_u128(101);
pub(super) const OTHER_SELLER: Uuid = Uuid::from_u128(102);
pub(super) const KEY: Uuid = Uuid::from_u128(201);
pub(super) const COMMAND: Uuid = Uuid::from_u128(301);
pub(super) const LISTING: Uuid = Uuid::from_u128(401);

pub(super) fn equipment(eid: &str) -> ListingEquipmentIdentity {
    ListingEquipmentIdentity::new("main", eid).unwrap_or_else(|error| panic!("夹具身份：{error}"))
}

pub(super) fn time(value: &str) -> Timestamp {
    value.parse().unwrap_or_else(|error| panic!("夹具时刻：{error}"))
}

pub(super) fn start() -> Timestamp {
    time("2026-01-01T00:00:00Z")
}

pub(super) fn later() -> Timestamp {
    time("2026-01-01T01:00:00Z")
}

pub(super) fn price(value: i64) -> ListingPrice {
    ListingPrice::new(value).unwrap_or_else(|error| panic!("夹具标价：{error}"))
}

pub(super) fn registration_proof(replay_key_id: Uuid, marker: u8) -> RegistrationProof {
    RegistrationProof { replay_key_id, mac: RegistrationRequestMac::new([marker; 32]) }
}

pub(super) async fn insert_key(
    connection: &mut PgConnection,
    replay_key_id: Uuid,
    current: bool,
) -> Result<(), WriteError> {
    sqlx::query(
        "INSERT INTO public.consignment_replay_keys
          (id, write_current, wrapped_key, nonce, encryption_key_id, created_at, updated_at)
         VALUES ($1, $2, decode(repeat('11',48),'hex'), $4,
                 'fake-data-key', $3, $3)",
    )
    .bind(replay_key_id)
    .bind(current)
    .bind(start())
    .bind(if replay_key_id == KEY { [0x22_u8; 12] } else { [0x33; 12] }.as_slice())
    .execute(connection)
    .await?;
    Ok(())
}

pub(super) async fn seed(maintenance: &DatabaseStore) -> Result<(), WriteError> {
    let pool = maintenance.postgres_pool();
    for (seller, username, marker) in
        [(SELLER, "FakeSellerOne", '1'), (OTHER_SELLER, "FakeSellerTwo", '2')]
    {
        sqlx::query(
            "INSERT INTO public.users (id, username, public_key_hash)
             VALUES ($1, $2, decode(repeat($3,64),'hex'))",
        )
        .bind(seller)
        .bind(username)
        .bind(marker.to_string())
        .execute(pool)
        .await?;
    }
    insert_key(&mut *pool.acquire().await?, KEY, true).await
}

pub(super) async fn register(
    store: &DatabaseStore,
    command_id: Uuid,
    listing_id: Uuid,
    seller: Uuid,
    eid: &str,
) -> Result<StoredReceipt, WriteError> {
    let identity = ListingLockIdentity { seller_user_id: seller, equipment: equipment(eid) };
    with_command_transaction(
        store,
        LockKeys {
            command_id,
            sellers: &[seller],
            equipment: std::slice::from_ref(&identity.equipment),
        },
        &identity,
        move |operation_identity, tx| {
            Box::pin(async move {
                let compute_mac =
                    |_, _: &auction_application::consignment::WrappedRegistrationReplayKey| {
                        Ok(registration_proof(KEY, 1).mac)
                    };
                match tx
                    .arbitrate(
                        Actor::PublicUser(seller),
                        ListingOperation::Register,
                        Target::New,
                        RequestProof::Registration(&compute_mac),
                    )
                    .await?
                {
                    Arbitration::Replay(receipt) => Ok(receipt),
                    Arbitration::First(mut writer) => {
                        writer
                            .stage_registration(
                                NewListing {
                                    listing_id,
                                    identity: operation_identity,
                                    fee_bps: FeeBps::DIRECT,
                                    service_epoch: 1,
                                },
                                ListingSnapshot {
                                    mode: ListingMode::Direct,
                                    state: ListingState::Resolving,
                                    price: price(101),
                                    state_version: ListingVersion::FIRST,
                                    expires_at: time("2026-01-02T00:00:00Z"),
                                    reason: None,
                                },
                                start(),
                            )
                            .await?;
                        writer
                            .finish_registration(
                                registration_proof(KEY, 1),
                                Decision::Changed {
                                    listing_id,
                                    operation: ChangedOperation::Register,
                                },
                                start(),
                            )
                            .await
                    }
                }
            })
        },
    )
    .await
}

pub(super) async fn reject(
    tx: &mut super::super::session::CommandTransaction<'_>,
    actor: Uuid,
    listing_id: Uuid,
    proof: RequestHash,
) -> Result<(bool, StoredReceipt), WriteError> {
    match tx
        .arbitrate(
            Actor::PublicUser(actor),
            ListingOperation::Reprice,
            Target::Listing(listing_id),
            RequestProof::Digest(proof),
        )
        .await?
    {
        Arbitration::Replay(receipt) => Ok((true, receipt)),
        Arbitration::First(writer) => Ok((
            false,
            writer
                .finish(
                    Decision::Rejected {
                        listing_id: None,
                        reason: ListingBusinessRejection::ListingNotFound,
                    },
                    start(),
                )
                .await?,
        )),
    }
}

pub(super) async fn counts(pool: &PgPool) -> Result<(i64, i64, i64, i64), WriteError> {
    Ok(sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.consignment_listings),
                (SELECT count(*) FROM public.consignment_commands),
                (SELECT count(*) FROM public.consignment_events),
                (SELECT count(*) FROM public.audit_records WHERE action LIKE 'consignment.%')",
    )
    .fetch_one(pool)
    .await?)
}
