//! 自动重列写端：对冻结开关为开的未付收回处置，同事务追加 g+1 重列代、推进当前指针并把挂单
//! 从 `held_off_market` 回到 `available`（7A-2）。
//!
//! 保护不变量 2、3、6、10：判定与手动重列同一组前置条件（领域 `decide_relist`），全部取自锁内事实；
//! 新代、指针 CAS、重列事件与成功回执同事务提交，数据库触发器在提交前再核对一次；每个处置只作一次决定
//! （`UNIQUE(source_consumption_id)`）；开关只读处置里冻结的值，之后改配置不影响已作的决定。

use auction_application::consignment::{
    AutoRelist, AutoRelistOutcome, AutoRelistReceipt, ConsignmentError, DepositBusinessRejection,
    PendingAutoRelist, RecoveryOperation,
};
use auction_domain::CommandId;
use auction_domain::consignment::deposit::PositionState;
use auction_domain::consignment::recovery::{RelistDecision, RelistEligibility, decide_relist};
use auction_domain::consignment::{ListingEquipmentIdentity, ListingState};
use auction_time::Timestamp;
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::auto_relist_facts::{relist_facts, unpaid_sale_sealed};
use super::ledger::{
    Arbitration, LedgerCommand, Rejection, Step, Target, arbitrate, insert_success,
};
use super::sale_withdrawal::recall_to_custody;
use super::withdrawal::{WithdrawalPosition, lock_withdrawal_position};
use crate::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::session::DepositTransaction;
use crate::database::consignment::commands::{LockKeys, with_deposit_transaction};
use crate::database::consignment::deposit::primitives::{database_now, service_control};
use crate::database::consignment::listings::projection::lock_listings;
use crate::database::mail::originals::invariant_decoding;

/// 冻结在处置行里的、决定所需的事实。
pub(super) struct DispositionRow {
    pub(super) consumption_id: Uuid,
    pub(super) contract_id: Uuid,
    pub(super) position_id: Uuid,
    pub(super) seller_user_id: Uuid,
    pub(super) listing_id: Uuid,
    pub(super) relist: RelistEligibility,
    pub(super) auto_relist_enabled: bool,
    pub(super) config_digest: Vec<u8>,
    pub(super) closes_predecessor: bool,
    pub(super) proof_id: Uuid,
}

async fn load_disposition(
    connection: &mut PgConnection,
    disposition_id: Uuid,
) -> Result<Option<DispositionRow>, WriteError> {
    let row = sqlx::query(
        "SELECT disposition.consumption_id, disposition.contract_id, disposition.relist,
                disposition.auto_relist_enabled, disposition.config_digest,
                disposition.buyer_right, contract.position_id, contract.seller_user_id,
                contract.listing_id, consumption.proof_id
         FROM public.consignment_return_dispositions AS disposition
         JOIN public.consignment_contracts AS contract
           ON contract.contract_id = disposition.contract_id
         JOIN public.consignment_return_consumptions AS consumption
           ON consumption.id = disposition.consumption_id
         WHERE disposition.disposition_id = $1",
    )
    .bind(disposition_id)
    .fetch_optional(connection)
    .await?;
    row.map(|row| {
        let relist = match row.try_get::<&str, _>("relist")? {
            "manual_only" => RelistEligibility::ManualOnly,
            "manual_or_auto" => RelistEligibility::ManualOrAuto,
            _ => RelistEligibility::Forbidden,
        };
        Ok(DispositionRow {
            consumption_id: row.try_get("consumption_id")?,
            contract_id: row.try_get("contract_id")?,
            position_id: row.try_get("position_id")?,
            seller_user_id: row.try_get("seller_user_id")?,
            listing_id: row.try_get("listing_id")?,
            relist,
            auto_relist_enabled: row.try_get("auto_relist_enabled")?,
            config_digest: row.try_get("config_digest")?,
            // 上代买方交割已按处置封闭的两种写法；其余都不算封闭（失败关闭）。
            closes_predecessor: matches!(
                row.try_get::<&str, _>("buyer_right")?,
                "close_at_limit" | "terminate_unpaid"
            ),
            proof_id: row.try_get("proof_id")?,
        })
    })
    .transpose()
}

/// 预读：卖家与规范实物，只用来决定取哪些会话锁，不具授权含义。
async fn pre_read(
    connection: &mut PgConnection,
    disposition_id: Uuid,
) -> Result<Option<(Uuid, Uuid, ListingEquipmentIdentity)>, WriteError> {
    let row = sqlx::query(
        "SELECT contract.listing_id, contract.seller_user_id, contract.world_id,
                contract.canonical_eid
         FROM public.consignment_return_dispositions AS disposition
         JOIN public.consignment_contracts AS contract
           ON contract.contract_id = disposition.contract_id
         WHERE disposition.disposition_id = $1",
    )
    .bind(disposition_id)
    .fetch_optional(connection)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let equipment = ListingEquipmentIdentity::new(
        &row.try_get::<String, _>("world_id")?,
        &row.try_get::<String, _>("canonical_eid")?,
    )
    .map_err(invariant_decoding("consignment_relist_equipment"))?;
    Ok(Some((row.try_get("listing_id")?, row.try_get("seller_user_id")?, equipment)))
}

/// 自动重列端口实现：预读锁集合后在一个顶层 Serializable 事务内完成。
pub(super) async fn auto_relist(
    store: &DatabaseStore,
    command: AutoRelist,
) -> Result<AutoRelistReceipt, ConsignmentError> {
    if command.max_open_per_user < 1 {
        return Err(ConsignmentError::Unavailable);
    }
    let mut pre =
        store.pool().acquire().await.map_err(|error| WriteError::from(error).into_application())?;
    let plan =
        pre_read(&mut pre, command.disposition_id).await.map_err(WriteError::into_application)?;
    drop(pre);
    let Some((listing_id, seller, equipment)) = plan else {
        return Err(ConsignmentError::Deposit(DepositBusinessRejection::SourceMismatch));
    };
    if listing_id != command.listing_id {
        return Err(ConsignmentError::Deposit(DepositBusinessRejection::SourceMismatch));
    }
    let (sellers, equipment) = (vec![seller], vec![equipment]);
    let keys = LockKeys {
        command_id: command.command_id.as_uuid(),
        sellers: &sellers,
        equipment: &equipment,
    };
    let (locked_sellers, locked_equipment) = (sellers.clone(), equipment.clone());
    let step = with_deposit_transaction(store, keys, &[], &command, |command, tx| {
        let (sellers, equipment) = (locked_sellers.clone(), locked_equipment.clone());
        Box::pin(async move {
            tx.ensure_lock_set(&sellers, &equipment, &[])?;
            relist_in_tx(tx, command).await
        })
    })
    .await
    .map_err(WriteError::into_application)?;
    step.into_result()
}

async fn relist_in_tx(
    tx: &mut DepositTransaction<'_, '_>,
    command: &AutoRelist,
) -> Result<Step<AutoRelistReceipt>, WriteError> {
    let ledger = LedgerCommand {
        command_id: command.command_id.as_uuid(),
        operation: RecoveryOperation::AutoRelist,
        target: Target::Listing(command.listing_id),
        digest: &command.request_hash,
    };
    match arbitrate(tx.connection(), &ledger).await? {
        Step::Rejected(rejection) => return Ok(Step::Rejected(rejection)),
        Step::Done(Arbitration::Replay(body)) => {
            return replay_receipt(command, &body).map(Step::Done);
        }
        Step::Done(Arbitration::First) => {}
    }
    let Some(disposition) = load_disposition(tx.connection(), command.disposition_id).await? else {
        return Ok(Step::Rejected(Rejection::SourceMismatch));
    };
    if disposition.listing_id != command.listing_id {
        return Ok(Step::Rejected(Rejection::SourceMismatch));
    }
    let Some(listing) =
        lock_listings(tx.connection(), &[command.listing_id]).await?.into_iter().next()
    else {
        return Ok(Step::Rejected(Rejection::SourceMismatch));
    };
    let reason: Option<String> =
        sqlx::query_scalar("SELECT reason_code FROM public.consignment_listings WHERE id = $1")
            .bind(command.listing_id)
            .fetch_one(tx.connection())
            .await?;
    // 开关冻结为关、或处置不允许自动再售、或挂单还不是未付收回下架态：没有决定可作，不写任何行。
    // 冻结值是处置里的那一份，当前配置不参与（J12）。
    let held_unpaid = listing.snapshot.state == ListingState::HeldOffMarket
        && reason.as_deref() == Some("unpaid_recovered");
    if !disposition.auto_relist_enabled
        || disposition.relist != RelistEligibility::ManualOrAuto
        || !held_unpaid
    {
        return Ok(Step::Done(receipt(command, AutoRelistOutcome::NotReady, false)));
    }
    // 每个处置只作一次决定；异命令 ID 再来是另造决定，拒绝。
    let decided: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.consignment_relist_decisions
                        WHERE source_consumption_id = $1)",
    )
    .bind(disposition.consumption_id)
    .fetch_one(tx.connection())
    .await?;
    if decided {
        return Ok(Step::Rejected(Rejection::DispositionConflict));
    }
    let now = database_now(tx.connection()).await?;
    // position 与上代 Sale 的封闭事实：未付取消封闭了本处置的 Sale、原件从未离库或已精确收回，
    // 才算上代交割已封闭；position 仍是 committed 时重列写端先回库（与手动重列同一条边）。
    let Some(position) = lock_withdrawal_position(tx.connection(), disposition.listing_id).await?
    else {
        return Ok(Step::Rejected(Rejection::SourceMismatch));
    };
    let sealed = unpaid_sale_sealed(tx.connection(), &position, disposition.contract_id).await?;
    let facts = relist_facts(
        tx.connection(),
        command,
        &disposition,
        &listing,
        reason.as_deref(),
        (&position, sealed),
        now,
    )
    .await?;
    // 原件还没有回到“在库”（Sale 未封闭或原件可能在途、reconcile 待核对）是暂态：
    // 不作决定，等条件具备后再评估；否则每个处置唯一的一次决定会被暂态用掉（不变量 6）。
    if !facts.original_in_custody {
        return Ok(Step::Done(receipt(command, AutoRelistOutcome::NotReady, false)));
    }
    match decide_relist(&facts) {
        RelistDecision::Relist { .. } => {
            write_relist(tx.connection(), command, &disposition, &listing, &position, now)
                .await
                .map(Step::Done)
        }
        RelistDecision::Keep { .. } => {
            write_held(tx.connection(), command, &disposition, &listing, now).await.map(Step::Done)
        }
        RelistDecision::Reject(error) => {
            tracing::warn!(%error, "自动重列事实不成立，失败关闭");
            Ok(Step::Rejected(Rejection::EvidenceConflict))
        }
    }
}

fn receipt(command: &AutoRelist, outcome: AutoRelistOutcome, replayed: bool) -> AutoRelistReceipt {
    AutoRelistReceipt {
        command_id: CommandId::from_uuid(command.command_id.as_uuid()),
        outcome,
        replayed,
    }
}

/// 重放：账本回执里的结果原样返回，不重算。
fn replay_receipt(command: &AutoRelist, body: &Value) -> Result<AutoRelistReceipt, WriteError> {
    let number = |key: &str| body.get(key).and_then(Value::as_u64);
    let listing_version = number("listing_version")
        .ok_or_else(|| WriteError::invariant("consignment_relist_replay"))?;
    let outcome = match body.get("disposition").and_then(Value::as_str) {
        Some("relisted") => AutoRelistOutcome::Relisted {
            listing_id: command.listing_id,
            listing_version,
            generation: number("generation")
                .ok_or_else(|| WriteError::invariant("consignment_relist_replay"))?,
        },
        Some("held") => AutoRelistOutcome::Held { listing_id: command.listing_id, listing_version },
        _ => return Err(WriteError::invariant("consignment_relist_replay")),
    };
    Ok(receipt(command, outcome, true))
}

/// 留库：不改挂单与代，只追加“held”决定与回执，该处置之后不再自动重列。
async fn write_held(
    connection: &mut PgConnection,
    command: &AutoRelist,
    disposition: &DispositionRow,
    listing: &crate::database::consignment::listings::projection::LockedListing,
    now: Timestamp,
) -> Result<AutoRelistReceipt, WriteError> {
    let listing_version = listing.snapshot.state_version.value();
    let ledger = ledger_of(command);
    insert_success(
        connection,
        &ledger,
        "held",
        json!({
            "listing_id": command.listing_id.to_string(),
            "listing_version": listing_version,
            "state": "held_off_market",
        }),
    )
    .await?;
    insert_decision(connection, command, disposition, None, now).await?;
    Ok(receipt(
        command,
        AutoRelistOutcome::Held { listing_id: command.listing_id, listing_version },
        false,
    ))
}

fn ledger_of(command: &AutoRelist) -> LedgerCommand<'_> {
    LedgerCommand {
        command_id: command.command_id.as_uuid(),
        operation: RecoveryOperation::AutoRelist,
        target: Target::Listing(command.listing_id),
        digest: &command.request_hash,
    }
}

/// 追加 g+1 重列代、推进当前指针、把挂单回到 `available` 并写事件、决定与回执（同一事务）。
///
/// 写序由 000137 的守卫决定：指针 CAS 的行级守卫要求同事务已有成功命令与 g+1 代行，
/// 所以先写回执，再写代，再推进指针，最后写挂单版本与事件。
async fn write_relist(
    connection: &mut PgConnection,
    command: &AutoRelist,
    disposition: &DispositionRow,
    listing: &crate::database::consignment::listings::projection::LockedListing,
    position: &WithdrawalPosition,
    now: Timestamp,
) -> Result<AutoRelistReceipt, WriteError> {
    let predecessor = i64::try_from(
        position
            .generation
            .ok_or_else(|| WriteError::invariant("consignment_relist_no_generation"))?,
    )?;
    let generation = predecessor + 1;
    let listing_version = listing.snapshot.state_version.value() + 1;
    let price = listing.snapshot.price.value();
    let control = service_control(connection)
        .await?
        .ok_or_else(|| WriteError::invariant("consignment_relist_service_control"))?;
    let ledger = ledger_of(command);
    insert_success(
        connection,
        &ledger,
        "relisted",
        json!({
            "listing_id": command.listing_id.to_string(),
            "listing_version": listing_version,
            "state": "available",
            "price": price,
            "generation": generation,
        }),
    )
    .await?;
    // position 仍是 committed（Sale 已按未付取消封闭、原件在库）：先回库，与手动重列同一条
    // committed -> in_custody 边；事件引用 Sale 封闭事实，版本随之 +1。
    let mut position_version = position.version;
    if position.state == PositionState::Committed {
        position_version = recall_to_custody(
            connection,
            position,
            disposition.contract_id,
            command.command_id.as_uuid(),
            (command.listing_id, listing.snapshot.state_version.value()),
            now,
        )
        .await?;
    }
    // 新代冻结当前服务代次与卖家当前拉黑基线（D9 §5.3）；证明沿用精确 Take 收回证明。
    let baseline: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM public.user_blacklist_history WHERE user_id = $1 ORDER BY id",
    )
    .bind(disposition.seller_user_id)
    .fetch_all(&mut *connection)
    .await?;
    sqlx::query(
        "INSERT INTO public.consignment_disposition_generations
            (position_id, generation, purpose, predecessor, proof_id, service_epoch,
             blacklist_baseline_ids, season_window_id, created_command_id, created_at)
         VALUES ($1, $2, 'relist', $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(disposition.position_id)
    .bind(generation)
    .bind(predecessor)
    .bind(disposition.proof_id)
    .bind(control.enabled_epoch)
    .bind(&baseline)
    .bind(position.season_window_id)
    .bind(command.command_id.as_uuid())
    .bind(now)
    .execute(&mut *connection)
    .await?;
    let moved = sqlx::query(
        "UPDATE public.consignment_custody_positions
         SET current_generation = $3, version = version + 1, updated_at = $4
         WHERE id = $1 AND version = $2 AND current_generation = $5",
    )
    .bind(disposition.position_id)
    .bind(position_version)
    .bind(generation)
    .bind(now)
    .bind(predecessor)
    .execute(&mut *connection)
    .await?;
    if moved.rows_affected() != 1 {
        return Err(WriteError::invariant("consignment_relist_pointer_cas"));
    }
    let updated = sqlx::query(
        "UPDATE public.consignment_listings
         SET state = 'available', state_version = $3, reason_code = NULL, updated_at = $4
         WHERE id = $1 AND state_version = $2",
    )
    .bind(command.listing_id)
    .bind(listing.snapshot.state_version.value().cast_signed())
    .bind(listing_version.cast_signed())
    .bind(now)
    .execute(&mut *connection)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(WriteError::invariant("consignment_relist_listing_version"));
    }
    sqlx::query(
        "INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state, to_state,
             reason_code, price, occurred_at)
         VALUES ($1, $2, $3, 'worker', 'relisted', 'held_off_market', 'available', NULL, $4, $5)",
    )
    .bind(command.listing_id)
    .bind(listing_version.cast_signed())
    .bind(command.command_id.as_uuid())
    .bind(price)
    .bind(now)
    .execute(&mut *connection)
    .await?;
    insert_decision(connection, command, disposition, Some(generation), now).await?;
    Ok(receipt(
        command,
        AutoRelistOutcome::Relisted {
            listing_id: command.listing_id,
            listing_version,
            generation: generation.cast_unsigned(),
        },
        false,
    ))
}

async fn insert_decision(
    connection: &mut PgConnection,
    command: &AutoRelist,
    disposition: &DispositionRow,
    new_generation: Option<i64>,
    now: Timestamp,
) -> Result<(), WriteError> {
    sqlx::query(
        "INSERT INTO public.consignment_relist_decisions
            (decision_id, source_consumption_id, listing_id, position_id, mode,
             auto_relist_enabled, config_digest, outcome, new_generation, command_id, decided_at)
         VALUES ($1, $2, $3, $4, 'auto', true, $5, $6, $7, $8, $9)",
    )
    .bind(Uuid::new_v4())
    .bind(disposition.consumption_id)
    .bind(command.listing_id)
    .bind(disposition.position_id)
    .bind(disposition.config_digest.as_slice())
    .bind(if new_generation.is_some() { "relisted" } else { "held" })
    .bind(new_generation)
    .bind(command.command_id.as_uuid())
    .bind(now)
    .execute(connection)
    .await?;
    Ok(())
}

/// 冻结开关为开、尚无决定、挂单已是未付收回下架态的处置（仅供 Worker 轮询，没有授权含义）。
pub(super) async fn pending_auto_relists(
    store: &DatabaseStore,
    limit: u32,
) -> Result<Vec<PendingAutoRelist>, ConsignmentError> {
    let rows = sqlx::query(
        "SELECT disposition.disposition_id, contract.listing_id
         FROM public.consignment_return_dispositions AS disposition
         JOIN public.consignment_contracts AS contract
           ON contract.contract_id = disposition.contract_id
         JOIN public.consignment_listings AS listing ON listing.id = contract.listing_id
         WHERE disposition.relist = 'manual_or_auto' AND disposition.auto_relist_enabled
           AND listing.state = 'held_off_market' AND listing.reason_code = 'unpaid_recovered'
           AND NOT EXISTS (
               SELECT 1 FROM public.consignment_relist_decisions AS decision
               WHERE decision.source_consumption_id = disposition.consumption_id)
         ORDER BY disposition.decided_at, disposition.disposition_id
         LIMIT $1",
    )
    .bind(i64::from(limit))
    .fetch_all(store.pool())
    .await
    .map_err(|error| WriteError::from(error).into_application())?;
    rows.iter()
        .map(|row| {
            Ok(PendingAutoRelist {
                disposition_id: row.try_get("disposition_id")?,
                listing_id: row.try_get("listing_id")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(|error| WriteError::from(error).into_application())
}
