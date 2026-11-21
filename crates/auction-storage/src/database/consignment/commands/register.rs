//! 登记新挂单、冻结资格基线并原子建立解析工作。

use auction_application::admin::{AdminIntakeError, EquipmentKeySealerPort};
use auction_application::consignment::{
    ConsignmentError, ListingBusinessRejection, ListingCommandOutcome, ListingCommandRejection,
    ListingCredentialError, ListingOperation, ListingRegistered, RegisterListingCommand,
    RegistrationReplayCrypto,
};
use auction_domain::consignment::{
    FeeBps, ForcedTerminationFacts, ListingTransition, ListingTrigger, check_listing_history,
    transition_listing,
};

use super::credentials::{insert_secret, registration_proof};
use super::errors::{WriteError, invalid_projection};
use super::facts::{ChangedOperation, Decision};
use super::ledger::{Actor, Arbitration, CommandWriter, RequestProof, Target};
use super::receipts::{registered, replay};
use super::session::{CommandTransaction, LockKeys, with_command_transaction};
use super::target::{now, result};
use super::termination::retire_occupants;
use crate::database::DatabaseStore;
use crate::database::consignment::eligibility::accounts::lock_seller_eligibility;
use crate::database::consignment::eligibility::history::read_listing_history;
use crate::database::consignment::eligibility::lots::insert_registration_baselines;
use crate::database::consignment::eligibility::occupancy::{
    effective_activity_count, hourly_registration_count, lock_equipment_occupants,
};
use crate::database::consignment::listings::projection::{ListingLockIdentity, NewListing};
use crate::database::consignment::lookup::control::{lock_control, read_admission};
use crate::database::consignment::lookup::work::enqueue_lookup;

struct Registration<'a> {
    command: RegisterListingCommand<'a>,
    sealer: Option<&'a dyn EquipmentKeySealerPort>,
    crypto: Option<&'a dyn RegistrationReplayCrypto>,
    identity: ListingLockIdentity,
}

/// 先全局比较原登记，再核当前准入。不变量 1、2、3、5、6、7、10。
pub(super) async fn register(
    store: &DatabaseStore,
    command: RegisterListingCommand<'_>,
    sealer: Option<&dyn EquipmentKeySealerPort>,
    crypto: Option<&dyn RegistrationReplayCrypto>,
) -> Result<ListingCommandOutcome<ListingRegistered>, ConsignmentError> {
    let keys = LockKeys {
        command_id: command.command_id.as_uuid(),
        sellers: &[command.seller_user_id.as_uuid()],
        equipment: std::slice::from_ref(command.input.identity),
    };
    let identity = ListingLockIdentity {
        seller_user_id: command.seller_user_id.as_uuid(),
        equipment: command.input.identity.clone(),
    };
    let input = Registration { command, sealer, crypto, identity };
    result(
        with_command_transaction(store, keys, &input, |input, tx| Box::pin(execute(input, tx)))
            .await,
    )
}

async fn execute(
    input: &Registration<'_>,
    tx: &mut CommandTransaction<'_>,
) -> Result<ListingCommandOutcome<ListingRegistered>, WriteError> {
    let command = &input.command;
    let compute_mac = |replay_key_id, wrapped: &_| {
        // 缺封装端口不影响重放。缺比较端口时不能跳过含 key 的原证明。
        input.crypto.ok_or(ListingCredentialError)?.registration_mac(
            replay_key_id,
            wrapped,
            &command.input,
        )
    };
    let mut writer = match tx
        .arbitrate(
            Actor::PublicUser(command.seller_user_id.as_uuid()),
            ListingOperation::Register,
            Target::New,
            RequestProof::Registration(&compute_mac),
        )
        .await?
    {
        Arbitration::Replay(stored) => return replay(&stored, registered),
        Arbitration::First(writer) => writer,
    };

    writer.request_id = command.request_id.as_deref();
    if !command.policy.enabled {
        return Err(ConsignmentError::Disabled.into());
    }
    let (Some(sealer), Some(crypto)) = (input.sealer, input.crypto) else {
        return Err(ConsignmentError::Unavailable.into());
    };

    let seller = lock_seller_eligibility(writer.connection(), command.seller_user_id).await?;
    let mut rejection = None;
    let mut baseline_count = 0;
    if seller.as_ref().is_some_and(|seller| seller.active && !seller.active_blacklist) {
        let history = read_listing_history(writer.connection(), command.input.identity).await?;
        baseline_count = history.lot_ids.len();
        if let Err(error) =
            check_listing_history(command.input.identity, command.seller_user_id, &history.aliases)
        {
            let ConsignmentError::Business(reason) = error.into() else {
                return Err(WriteError::invariant("consignment_registration_history"));
            };
            rejection = Some(reason);
        }
    } else {
        rejection = Some(ListingBusinessRejection::ParticipantIneligible);
    }
    // 先锁同件挂单，再取单例及锁内墙钟；同一快照内重读其有效占用，覆盖等待期间的到期。
    if rejection.is_none() {
        let as_of = now(writer.connection()).await?;
        lock_equipment_occupants(writer.connection(), command.input.identity, as_of).await?;
    }
    let control = lock_control(writer.connection()).await?;
    let admission = read_admission(writer.connection()).await?;
    if !control.admitting || !admission.worker_ready {
        // 本地配置已经开启，持久准入尚未同步属于依赖未就绪。
        return Err(ConsignmentError::Unavailable.into());
    }
    let now = admission.permit.as_of;
    let proof = registration_proof(
        writer.connection(),
        crypto,
        command.candidate_replay_key_id,
        &command.input,
        now,
    )
    .await?;
    let fee = FeeBps::for_mode(command.input.mode, command.policy.consigned_fee_bps);
    let mut collisions = Vec::new();
    if rejection.is_none() {
        let occupants =
            lock_equipment_occupants(writer.connection(), command.input.identity, now).await?;
        collisions = retire_occupants(&mut writer, &occupants, None, now).await?;
        rejection = if occupants
            .iter()
            .any(|row| row.forced_reason.is_none() && row.seller_user_id == command.seller_user_id)
        {
            Some(ListingBusinessRejection::AlreadyOpen)
        } else if occupants.iter().any(|row| row.forced_reason.is_none() && row.published) {
            Some(ListingBusinessRejection::EquipmentBusy)
        } else if hourly_registration_count(writer.connection(), command.seller_user_id, now)
            .await?
            >= i64::from(command.policy.registrations_per_hour)
        {
            Some(ListingBusinessRejection::RegistrationRateLimited)
        } else if effective_activity_count(writer.connection(), command.seller_user_id, now).await?
            >= i64::from(command.policy.max_open_per_user)
        {
            Some(ListingBusinessRejection::ListingLimitReached)
        } else if fee != command.input.expected_fee_bps {
            Some(ListingBusinessRejection::FeeChanged)
        } else if !world_available(&mut writer, command).await? {
            Some(ListingBusinessRejection::WorldUnavailable)
        } else {
            None
        };
    }
    if let Some(reason) = rejection {
        writer
            .finish_registration_with_collisions(
                proof,
                Decision::Rejected { listing_id: None, reason },
                &collisions,
                now,
            )
            .await?;
        return Ok(ListingCommandOutcome::Rejected(ListingCommandRejection::Recorded(reason)));
    }

    let envelope =
        sealer.seal(command.aad_equipment_id, command.input.key).map_err(seal_unavailable)?;
    let ListingTransition::Changed { listing, .. } = transition_listing(
        None,
        ListingTrigger::Register { mode: command.input.mode, price: command.input.price },
        now,
        ForcedTerminationFacts::default(),
    )
    .map_err(invalid_projection)?
    else {
        return Err(WriteError::invariant("consignment_registration_transition"));
    };
    writer
        .stage_registration(
            NewListing {
                listing_id: command.listing_id,
                identity: &input.identity,
                fee_bps: fee,
                service_epoch: control.enabled_epoch,
            },
            listing,
            now,
        )
        .await?;
    let baselines = insert_registration_baselines(writer.connection(), command.listing_id).await?;
    if usize::try_from(baselines) != Ok(baseline_count) {
        return Err(WriteError::invariant("consignment_registration_baselines"));
    }
    insert_secret(
        writer.connection(),
        command.listing_id,
        command.aad_equipment_id,
        &envelope,
        now,
    )
    .await?;
    enqueue_lookup(writer.connection(), command.listing_id, command.work_id, now).await?;
    writer
        .finish_registration_with_collisions(
            proof,
            Decision::Changed {
                listing_id: command.listing_id,
                operation: ChangedOperation::Register,
            },
            &collisions,
            now,
        )
        .await?;
    Ok(ListingCommandOutcome::Accepted(ListingRegistered::new(command.listing_id, listing.price)?))
}

async fn world_available(
    writer: &mut CommandWriter<'_, '_>,
    command: &RegisterListingCommand<'_>,
) -> Result<bool, WriteError> {
    let world_id = command.input.identity.world_id();
    let allowed = match world_id {
        "main" => command.policy.worlds.main,
        "isekai" => command.policy.worlds.isekai,
        _ => false,
    };
    if !allowed {
        return Ok(false);
    }
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM public.worlds WHERE id = $1 AND enabled AND availability = 'available')",
    )
    .bind(world_id)
    .fetch_one(writer.connection())
    .await?)
}

fn seal_unavailable(error: AdminIntakeError) -> ConsignmentError {
    // 封装端口沿用 Admin 类型；这些封闭错误在登记中统一表示本地凭证能力不可用。
    match error {
        AdminIntakeError::KeyUnavailable
        | AdminIntakeError::InvalidCommand
        | AdminIntakeError::InvalidSource
        | AdminIntakeError::SourceUnavailable
        | AdminIntakeError::SourceUnconfigured
        | AdminIntakeError::ExternalAccessPaused
        | AdminIntakeError::CommandConflict
        | AdminIntakeError::StoredResultCorrupt => ConsignmentError::Unavailable,
    }
}
