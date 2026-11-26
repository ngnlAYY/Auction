//! 预约签发、换发与退休的 PostgreSQL 写入。

use std::collections::BTreeSet;

use auction_application::consignment::{
    ConsignmentError, ConsignmentFuture, DepositActor, DepositBusinessRejection, DepositOperation,
    ExpireReservationCommand, IssueReservationCommand, ReservationInvalidation, ReservationReceipt,
    ReservationRetired, ReservationSecretAad, ReservationStore, SealedReservationSecret,
};
use auction_application::core::RequestHash;
use auction_domain::CommandId;
use auction_domain::SettlementWorld;
use auction_domain::UserId;
use auction_domain::consignment::{ListingEquipmentIdentity, ListingState, check_listing_history};
use auction_time::Duration;
use sqlx::Row;
use uuid::Uuid;

use super::crypto::generate_reservation_token;
use super::outcome::{FirstOutcome, PortOutcome, reject, replay};
use super::primitives::{
    bot_party, claim_holder, current_snapshot, database_now, eligible, mail_readiness,
    next_version, pre_read_identity, season_state, seller_eligibility, seller_name,
    service_control,
};
use super::repository::DepositRepository;
use crate::database::consignment::commands::DepositWriter;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::ledger::{DepositArbitration, DepositTarget};
use crate::database::consignment::commands::session::{LockKeys, with_deposit_transaction};
use crate::database::consignment::eligibility::claims::{hold_reservation, release_reservation};
use crate::database::consignment::eligibility::history::read_listing_history;
use crate::database::consignment::listings::projection::ListingLockIdentity;
impl ReservationStore for DepositRepository {
    fn issue_reservation(
        &self,
        command: IssueReservationCommand,
    ) -> ConsignmentFuture<'_, ReservationReceipt> {
        Box::pin(async move {
            if command.command_id.as_uuid().is_nil() {
                return Err(ConsignmentError::InvalidCommandId);
            }
            if command.listing_id.is_nil() {
                return Err(ConsignmentError::InvalidListingId);
            }
            let identity = pre_read_identity(self.store(), command.listing_id)
                .await
                .map_err(WriteError::into_application)?;
            let mut sellers = BTreeSet::from([command.seller_user_id.as_uuid()]);
            if let Some(identity) = &identity {
                sellers.insert(identity.seller_user_id);
            }
            let sellers: Vec<_> = sellers.into_iter().collect();
            let equipment: Vec<_> =
                identity.iter().map(|identity| identity.equipment.clone()).collect();
            let input = (self, command, sellers, equipment);
            let keys = LockKeys {
                command_id: input.1.command_id.as_uuid(),
                sellers: &input.2,
                equipment: &input.3,
            };
            with_deposit_transaction(
                self.store(),
                keys,
                &[],
                &input,
                |(repository, command, sellers, equipment), tx| {
                    Box::pin(async move {
                        tx.ensure_lock_set(sellers, equipment, &[])?;
                        match tx
                            .arbitrate(
                                DepositActor::PublicUser(command.seller_user_id),
                                DepositOperation::IssueReservation,
                                DepositTarget::Listing(command.listing_id),
                                command.request_hash,
                                command.request_id.as_deref(),
                            )
                            .await?
                        {
                            DepositArbitration::Replay(stored) => replay(&stored),
                            DepositArbitration::First(mut writer) => {
                                let now = database_now(writer.connection()).await?;
                                match issue_stage(repository, command, writer, now).await? {
                                    FirstOutcome::Rejected(rejection) => {
                                        Ok(PortOutcome::Rejected(rejection))
                                    }
                                    FirstOutcome::Accepted { receipt, children } => {
                                        issue_children(tx.connection(), children).await?;
                                        Ok(PortOutcome::Accepted(receipt))
                                    }
                                }
                            }
                        }
                    })
                },
            )
            .await
            .map_err(WriteError::into_application)?
            .into_result()
        })
    }

    fn expire_reservation(
        &self,
        command: ExpireReservationCommand,
    ) -> ConsignmentFuture<'_, ReservationRetired> {
        Box::pin(async move {
            if command.command_id.as_uuid().is_nil() {
                return Err(ConsignmentError::InvalidCommandId);
            }
            let identity = pre_read_reservation(self.store(), command.reservation_id)
                .await
                .map_err(WriteError::into_application)?;
            let sellers: Vec<_> =
                identity.iter().map(|(_, identity)| identity.seller_user_id).collect();
            let equipment: Vec<_> =
                identity.iter().map(|(_, identity)| identity.equipment.clone()).collect();
            let input = (self, command, sellers, equipment);
            let keys = LockKeys {
                command_id: input.1.command_id.as_uuid(),
                sellers: &input.2,
                equipment: &input.3,
            };
            with_deposit_transaction(
                self.store(),
                keys,
                &[],
                &input,
                |(repository, command, sellers, equipment), tx| {
                    Box::pin(async move {
                        tx.ensure_lock_set(sellers, equipment, &[])?;
                        match tx
                            .arbitrate(
                                DepositActor::Worker,
                                DepositOperation::ExpireReservation,
                                DepositTarget::Reservation(command.reservation_id),
                                command.request_hash,
                                None,
                            )
                            .await?
                        {
                            DepositArbitration::Replay(stored) => replay(&stored),
                            DepositArbitration::First(mut writer) => {
                                let now = database_now(writer.connection()).await?;
                                match expire_stage(repository, command, writer, now).await? {
                                    FirstOutcome::Rejected(rejection) => {
                                        Ok(PortOutcome::Rejected(rejection))
                                    }
                                    FirstOutcome::Accepted { receipt, children } => {
                                        expire_children(tx.connection(), children).await?;
                                        Ok(PortOutcome::Accepted(receipt))
                                    }
                                }
                            }
                        }
                    })
                },
            )
            .await
            .map_err(WriteError::into_application)?
            .into_result()
        })
    }

    fn replay_issue_reservation(
        &self,
        seller_user_id: UserId,
        command_id: CommandId,
        listing_id: Uuid,
        request_hash: RequestHash,
    ) -> ConsignmentFuture<'_, Option<ReservationReceipt>> {
        Box::pin(super::replay::replay_issue_reservation(
            self.store(),
            seller_user_id,
            command_id.as_uuid(),
            listing_id,
            request_hash,
        ))
    }
}

pub(super) struct IssueChildren {
    reservation_id: Uuid,
    listing_id: Uuid,
    seller_user_id: Uuid,
    verified_sender: String,
    bot_uid: String,
    world: SettlementWorld,
    canonical_eid: String,
    season_window_id: Option<Uuid>,
    generation: u64,
    service_epoch: i64,
    session_id: Uuid,
    token_digest: [u8; 32],
    issued_at: auction_time::Timestamp,
    expires_at: auction_time::Timestamp,
    command_id: Uuid,
    sealed: SealedReservationSecret,
    claim_identity: ListingEquipmentIdentity,
}

pub(super) async fn issue_stage(
    repository: &DepositRepository,
    command: &IssueReservationCommand,
    mut writer: DepositWriter<'_, '_, '_>,
    now: auction_time::Timestamp,
) -> Result<FirstOutcome<ReservationReceipt, IssueChildren>, WriteError> {
    let seller = seller_eligibility(writer.connection(), command.seller_user_id).await?;
    if !eligible(seller.as_ref()) {
        return reject(writer, DepositBusinessRejection::ParticipantIneligible, now).await;
    }
    let Some(current) =
        super::primitives::lock_listing(writer.connection(), command.listing_id).await?
    else {
        return reject(writer, DepositBusinessRejection::ListingNotActive, now).await;
    };
    if current.identity.seller_user_id != command.seller_user_id.as_uuid() {
        return reject(writer, DepositBusinessRejection::ListingNotActive, now).await;
    }
    if current.snapshot.state_version != command.expected_version {
        return reject(writer, DepositBusinessRejection::StaleVersion, now).await;
    }
    if current.snapshot.state != ListingState::AwaitingDeposit {
        return reject(writer, DepositBusinessRejection::ListingNotActive, now).await;
    }
    let Some(control) = service_control(writer.connection()).await? else {
        return Err(ConsignmentError::Unavailable.into());
    };
    if !control.admitting || control.enabled_epoch != current.service_epoch {
        return reject(writer, DepositBusinessRejection::ListingNotActive, now).await;
    }
    let world = parse_world(current.identity.equipment.world_id())?;
    let Some(readiness) = mail_readiness(writer.connection(), world).await? else {
        return reject(writer, DepositBusinessRejection::MailUnavailable, now).await;
    };
    let Some(verified_sender) = seller_name(writer.connection(), command.seller_user_id).await?
    else {
        return reject(writer, DepositBusinessRejection::ParticipantIneligible, now).await;
    };
    if !readiness.accepts_sender(&verified_sender, now) {
        return reject(writer, DepositBusinessRejection::ParticipantIneligible, now).await;
    }
    if bot_party(writer.connection(), command.seller_user_id, &readiness.bot_uid).await? {
        return reject(writer, DepositBusinessRejection::BotParty, now).await;
    }
    let season = match world {
        SettlementWorld::Main => None,
        SettlementWorld::Isekai => {
            let window = super::primitives::current_season(writer.connection(), now).await?;
            if season_state(world, window.as_ref(), now)
                != auction_domain::consignment::SeasonState::Open
            {
                return reject(writer, DepositBusinessRejection::SeasonClosed, now).await;
            }
            window
        }
    };
    let identity = ListingEquipmentIdentity::new(
        current.identity.equipment.world_id(),
        current.identity.equipment.external_equipment_id(),
    )?;
    let history = read_listing_history(writer.connection(), &identity).await?;
    if let Err(error) = check_listing_history(&identity, command.seller_user_id, &history.aliases) {
        let rejection = match error {
            auction_domain::consignment::ListingError::EquipmentBusy => {
                DepositBusinessRejection::EquipmentBusy
            }
            _ => DepositBusinessRejection::ParticipantIneligible,
        };
        return reject(writer, rejection, now).await;
    }
    if let Some((reservation_id, position_id)) =
        claim_holder(writer.connection(), &current.identity).await?
    {
        let reissue = match (reservation_id, position_id) {
            (Some(reservation_id), None) => {
                let listing_id: Option<Uuid> = sqlx::query_scalar(
                    "SELECT listing_id FROM public.consignment_deposit_reservations
                     WHERE id = $1 AND state = 'active'",
                )
                .bind(reservation_id)
                .fetch_optional(writer.connection())
                .await?;
                listing_id == Some(command.listing_id)
            }
            _ => false,
        };
        if !reissue {
            return reject(writer, DepositBusinessRejection::EquipmentBusy, now).await;
        }
    }
    let generation: i64 = sqlx::query_scalar(
        "SELECT COALESCE(max(generation), 0) + 1
         FROM public.consignment_deposit_reservations WHERE listing_id = $1",
    )
    .bind(command.listing_id)
    .fetch_one(writer.connection())
    .await?;
    let generation = u64::try_from(generation)?;
    let mut expires_at = now
        .checked_add_signed(Duration::hours(repository.settings().reservation_ttl_hours))
        .ok_or_else(|| WriteError::invariant("consignment_reservation_expiry"))?;
    expires_at = expires_at.min(current.snapshot.expires_at);
    if let Some(window) = season.as_ref() {
        expires_at = expires_at.min(window.end_utc);
    }
    if expires_at <= now {
        return reject(writer, DepositBusinessRejection::ActionNotAllowed, now).await;
    }
    let reservation_id = Uuid::new_v4();
    let token = generate_reservation_token();
    let sealed = repository.crypto().seal(
        &token.text,
        ReservationSecretAad {
            reservation_id,
            generation,
            seller_user_id: command.seller_user_id.as_uuid(),
        },
    )?;
    let projection = next_version(current_snapshot(&current))?;
    writer
        .stage_listing_update(
            &current,
            projection,
            current.current_snapshot_id,
            "reservation_issued",
            None,
            now,
        )
        .await?;
    let receipt = ReservationReceipt::issued(
        command.command_id,
        command.listing_id,
        projection.state_version,
        projection.state,
        reservation_id,
        expires_at,
    );
    writer.finish(receipt.clone(), now).await?;
    Ok(FirstOutcome::Accepted {
        receipt,
        children: IssueChildren {
            reservation_id,
            listing_id: command.listing_id,
            seller_user_id: command.seller_user_id.as_uuid(),
            verified_sender,
            bot_uid: readiness.bot_uid,
            world,
            canonical_eid: current.identity.equipment.external_equipment_id().to_owned(),
            season_window_id: season.map(|window| window.id),
            generation,
            service_epoch: current.service_epoch,
            session_id: readiness.session_id,
            token_digest: token.digest.as_bytes(),
            issued_at: now,
            expires_at,
            command_id: command.command_id.as_uuid(),
            sealed,
            claim_identity: current.identity.equipment,
        },
    })
}

pub(super) async fn issue_children(
    connection: &mut sqlx::PgConnection,
    children: IssueChildren,
) -> Result<(), WriteError> {
    sqlx::query(
        "UPDATE public.consignment_deposit_reservations
         SET state = 'retired', retired_at = $2, reason = 'reissued', last_command_id = $3
         WHERE listing_id = $1 AND state = 'active'",
    )
    .bind(children.listing_id)
    .bind(children.issued_at)
    .bind(children.command_id)
    .execute(&mut *connection)
    .await?;
    // 不变量 2、7：换发后旧码只保留不可读摘要与审计事实，不能继续解封。
    sqlx::query(
        "UPDATE public.consignment_reservation_secrets secret
         SET ciphertext = NULL, nonce = NULL, encryption_key_id = NULL, fingerprint = NULL,
             retired_at = $2, updated_at = $2
         FROM public.consignment_deposit_reservations reservation
         WHERE secret.reservation_id = reservation.id
           AND reservation.listing_id = $1 AND reservation.state = 'retired'
           AND reservation.reason = 'reissued' AND reservation.last_command_id = $3",
    )
    .bind(children.listing_id)
    .bind(children.issued_at)
    .bind(children.command_id)
    .execute(&mut *connection)
    .await?;
    sqlx::query(
        "INSERT INTO public.consignment_deposit_reservations
            (id, listing_id, seller_user_id, verified_sender, bot_uid, world_id, canonical_eid,
             season_window_id, generation, service_epoch, session_id, token_digest, issued_at,
             expires_at, state, last_command_id, retired_at, reason)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
                 'active', $15, NULL, NULL)",
    )
    .bind(children.reservation_id)
    .bind(children.listing_id)
    .bind(children.seller_user_id)
    .bind(&children.verified_sender)
    .bind(&children.bot_uid)
    .bind(auction_domain::consignment::settlement_world_code(children.world))
    .bind(&children.canonical_eid)
    .bind(children.season_window_id)
    .bind(i64::try_from(children.generation)?)
    .bind(children.service_epoch)
    .bind(children.session_id)
    .bind(children.token_digest.as_slice())
    .bind(children.issued_at)
    .bind(children.expires_at)
    .bind(children.command_id)
    .execute(&mut *connection)
    .await?;
    sqlx::query(
        "INSERT INTO public.consignment_reservation_secrets
            (reservation_id, generation, seller_user_id, ciphertext, nonce, encryption_key_id,
             fingerprint, created_at, updated_at, retired_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8, NULL)",
    )
    .bind(children.reservation_id)
    .bind(i64::try_from(children.generation)?)
    .bind(children.seller_user_id)
    .bind(children.sealed.ciphertext())
    .bind(children.sealed.nonce().as_slice())
    .bind(children.sealed.encryption_key_id())
    .bind(children.sealed.fingerprint())
    .bind(children.issued_at)
    .execute(&mut *connection)
    .await?;
    hold_reservation(
        connection,
        &children.claim_identity,
        children.reservation_id,
        children.issued_at,
    )
    .await?;
    Ok(())
}

pub(super) struct ExpireChildren {
    reservation_id: Uuid,
    reason: &'static str,
    command_id: Uuid,
    retired_at: auction_time::Timestamp,
    identity: ListingEquipmentIdentity,
}

pub(super) async fn expire_stage(
    _repository: &DepositRepository,
    command: &ExpireReservationCommand,
    mut writer: DepositWriter<'_, '_, '_>,
    now: auction_time::Timestamp,
) -> Result<FirstOutcome<ReservationRetired, ExpireChildren>, WriteError> {
    let row = sqlx::query(
        "SELECT listing_id, generation, expires_at, state
         FROM public.consignment_deposit_reservations WHERE id = $1 FOR UPDATE",
    )
    .bind(command.reservation_id)
    .fetch_optional(writer.connection())
    .await?;
    let Some(row) = row else {
        return reject(writer, DepositBusinessRejection::ListingNotActive, now).await;
    };
    let listing_id: Uuid = row.try_get("listing_id")?;
    let generation: i64 = row.try_get("generation")?;
    let state: String = row.try_get("state")?;
    let expires_at: auction_time::Timestamp = row.try_get("expires_at")?;
    if u64::try_from(generation).ok() != Some(command.generation) {
        return reject(writer, DepositBusinessRejection::StaleVersion, now).await;
    }
    if state != "active" {
        return reject(writer, DepositBusinessRejection::ListingNotActive, now).await;
    }
    match command.invalidation {
        ReservationInvalidation::Cancelled { .. } => {
            // 裁定：cancel 只能作为 withdraw/admin_delist 的随附事实，expire 不能单独声明。
            return reject(writer, DepositBusinessRejection::ActionNotAllowed, now).await;
        }
        ReservationInvalidation::Expired { expires_at: provided } => {
            if provided != expires_at || now < expires_at {
                return reject(writer, DepositBusinessRejection::ActionNotAllowed, now).await;
            }
        }
        ReservationInvalidation::ServiceDisabled { service_epoch } => {
            let Some(control) = service_control(writer.connection()).await? else {
                return Err(ConsignmentError::Unavailable.into());
            };
            if control.admitting || u64::try_from(control.enabled_epoch).ok() != Some(service_epoch)
            {
                return reject(writer, DepositBusinessRejection::ActionNotAllowed, now).await;
            }
        }
        ReservationInvalidation::IdentityLost { .. } => {}
    }
    let Some(current) = super::primitives::lock_listing(writer.connection(), listing_id).await?
    else {
        return reject(writer, DepositBusinessRejection::ListingNotActive, now).await;
    };
    // 季末关闭可先于 Worker 退休到期预约；终态挂单只推进预约与持有，不再伪造
    // 挂单版本或事件（不变量 2、6）。
    if current.snapshot.state == ListingState::AwaitingDeposit {
        let projection = next_version(current_snapshot(&current))?;
        writer
            .stage_listing_update(
                &current,
                projection,
                current.current_snapshot_id,
                "reservation_retired",
                None,
                now,
            )
            .await?;
    }
    let receipt = ReservationRetired::new(command.command_id, command.reservation_id);
    writer.finish(receipt.clone(), now).await?;
    Ok(FirstOutcome::Accepted {
        receipt,
        children: ExpireChildren {
            reservation_id: command.reservation_id,
            reason: command.invalidation.kind(),
            command_id: command.command_id.as_uuid(),
            retired_at: now,
            identity: current.identity.equipment,
        },
    })
}

pub(super) async fn expire_children(
    connection: &mut sqlx::PgConnection,
    children: ExpireChildren,
) -> Result<(), WriteError> {
    sqlx::query(
        "UPDATE public.consignment_deposit_reservations
         SET state = 'retired', retired_at = $2, reason = $3, last_command_id = $4
         WHERE id = $1 AND state = 'active'",
    )
    .bind(children.reservation_id)
    .bind(children.retired_at)
    .bind(children.reason)
    .bind(children.command_id)
    .execute(&mut *connection)
    .await?;
    // 不变量 2、7：退休预约的令牌不再可解封，正文只留在受控外写记录之外。
    sqlx::query(
        "UPDATE public.consignment_reservation_secrets
         SET ciphertext = NULL, nonce = NULL, encryption_key_id = NULL, fingerprint = NULL,
             retired_at = $2, updated_at = $2
         WHERE reservation_id = $1 AND retired_at IS NULL",
    )
    .bind(children.reservation_id)
    .bind(children.retired_at)
    .execute(&mut *connection)
    .await?;
    release_reservation(
        connection,
        &children.identity,
        children.reservation_id,
        children.retired_at,
    )
    .await
}

pub(super) async fn pre_read_reservation(
    store: &crate::database::DatabaseStore,
    reservation_id: Uuid,
) -> Result<Option<(Uuid, ListingLockIdentity)>, WriteError> {
    let row = sqlx::query(
        "SELECT r.listing_id, l.seller_user_id, l.world_id, l.external_equipment_id
         FROM public.consignment_deposit_reservations r
         JOIN public.consignment_listings l ON l.id = r.listing_id
         WHERE r.id = $1",
    )
    .bind(reservation_id)
    .fetch_optional(store.pool())
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some((
        row.try_get("listing_id")?,
        ListingLockIdentity {
            // 预约只属于寄售挂单，卖家必然是本站账户。
            seller_user_id: row.try_get("seller_user_id")?,
            seller_game_name: None,
            equipment: ListingEquipmentIdentity::new(
                row.try_get("world_id")?,
                row.try_get("external_equipment_id")?,
            )?,
        },
    )))
}

fn parse_world(value: &str) -> Result<SettlementWorld, WriteError> {
    auction_domain::consignment::parse_settlement_world(value)
        .ok_or_else(|| WriteError::invariant("consignment_reservation_world"))
}
