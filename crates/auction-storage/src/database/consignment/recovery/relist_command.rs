//! 手动重列：把已止售、Sale 已封闭且原件在库的件重新上架为新销售代（g+1）。
//!
//! 保护不变量 1、2、3、6：命令经全局账本仲裁（同 ID 同请求返回原回执）；在同一实物 session 锁与
//! Serializable 事务内核对挂单版本、卖家资格、额度、季界、服务代次、运营止售与旧 Sale 的封闭事实，交
//! `decide_relist` 判定；通过后回库（需要时）、追加 g+1 `relist` 代并 CAS 当前指针、挂单
//! `held_off_market -> available`、事件与回执同提。任一事实缺失、未知或未封闭一律失败关闭，不静默修正。

use auction_application::consignment::{
    ConsignmentError, ConsignmentFuture, CustodyRelistStore, DEPOSIT_SCHEMA_VERSION, DepositActor,
    DepositBusinessRejection, DepositOperation, ListingStateChange, ReceiptDisposition,
    RelistCommand, RelistReceipt,
};
use auction_domain::consignment::deposit::{DepositError, PositionState};
use auction_domain::consignment::recovery::{
    CurrentGeneration, RelistDecision, RelistEligibility, RelistFacts, RelistMode, decide_relist,
};
use auction_domain::consignment::{
    HeldOffMarketReason, ListingPrice, ListingState, ListingVersion, parse_settlement_world,
};
use auction_domain::{CommandId, UserId};
use auction_time::Timestamp;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::sale_withdrawal::{append_successor_generation, recall_to_custody};
use super::withdrawal::{
    WithdrawalPosition, current_generation, lock_withdrawal_position, position_season_window,
    read_withdrawal_position, unknown_dispatch_open,
};
use crate::database::consignment::commands::errors::{WriteError, invalid_projection};
use crate::database::consignment::commands::ledger::{
    DepositArbitration, DepositTarget, StoredReceipt,
};
use crate::database::consignment::commands::{
    DepositReceipt, DepositRejection, DepositWriter, LockKeys, with_deposit_transaction,
};
use crate::database::consignment::deposit::decode_rejection;
use crate::database::consignment::deposit::primitives::{
    active_hold, database_now, lock_listing, pre_read_identity, quota_available, season_state,
    seller_eligibility, service_control, transition,
};
use crate::database::consignment::deposit::replay::replay_relist;
use crate::database::consignment::deposit::repository::DepositRepository;
use crate::database::consignment::eligibility::accounts::SellerEligibility;
use crate::database::consignment::eligibility::predicates::listing_sql;

/// 重列新代的用途码；与 000137 的 `c7_generation_purpose_ck` 一致。
const RELIST_PURPOSE: &str = "relist";

/// 同一事务内已落库的成功回执或确定拒绝。
enum RelistOutcome {
    Accepted(RelistReceipt),
    Rejected(DepositBusinessRejection),
}

impl RelistOutcome {
    /// 提交后才把拒绝变成错误，保证拒绝回执不会随回滚丢失。
    fn into_result(self) -> Result<RelistReceipt, ConsignmentError> {
        match self {
            Self::Accepted(receipt) => Ok(receipt),
            Self::Rejected(rejection) => Err(ConsignmentError::Deposit(rejection)),
        }
    }
}

/// 已存回执损坏：固定机器码，不回显任何内容。
fn corrupt() -> WriteError {
    WriteError::invariant("consignment_v2_receipt")
}

fn corrupt_from<E>(_: E) -> WriteError {
    corrupt()
}

impl DepositReceipt for RelistReceipt {
    fn operation(&self) -> DepositOperation {
        DepositOperation::Relist
    }

    fn status(&self) -> i16 {
        200
    }

    fn disposition(&self) -> ReceiptDisposition {
        ReceiptDisposition::Relisted
    }

    fn command_id(&self) -> CommandId {
        self.command_id
    }

    fn body(&self) -> Value {
        json!({
            "schema_version": DEPOSIT_SCHEMA_VERSION,
            "operation": DepositOperation::Relist.as_str(),
            "disposition": ReceiptDisposition::Relisted.as_str(),
            "command_id": self.command_id.as_uuid().to_string(),
            "listing_id": self.listing_id.to_string(),
            "listing_version": self.listing_version.value(),
            "state": self.state.as_str(),
            "price": self.price.value(),
            "generation": self.generation,
        })
    }

    fn listing_changes(&self) -> Vec<ListingStateChange> {
        vec![ListingStateChange {
            listing_id: self.listing_id,
            listing_version: self.listing_version.value(),
            state: self.state,
        }]
    }

    fn decode(stored: &StoredReceipt) -> Result<Self, WriteError> {
        #[derive(Deserialize)]
        struct Body {
            schema_version: u16,
            operation: String,
            disposition: String,
            command_id: Uuid,
            listing_id: Uuid,
            listing_version: u64,
            state: String,
            price: i64,
            generation: u64,
        }
        let body: Body = serde_json::from_value(stored.body.clone()).map_err(corrupt_from)?;
        if stored.status != 200
            || body.schema_version != DEPOSIT_SCHEMA_VERSION
            || body.operation != DepositOperation::Relist.as_str()
            || body.disposition != ReceiptDisposition::Relisted.as_str()
            || body.command_id.is_nil()
            || body.state != ListingState::Available.as_str()
            || body.generation < 2
        {
            return Err(corrupt());
        }
        Ok(Self::new(
            CommandId::from_uuid(body.command_id),
            body.listing_id,
            ListingVersion::new(body.listing_version).map_err(corrupt_from)?,
            ListingPrice::new(body.price).map_err(corrupt_from)?,
            body.generation,
        ))
    }
}

impl CustodyRelistStore for DepositRepository {
    /// 只读账本：不取锁、不写行，因而不受开关、资格与额度影响（不变量 1）。
    fn replay_relist(
        &self,
        command: RelistCommand,
    ) -> ConsignmentFuture<'_, Option<RelistReceipt>> {
        Box::pin(async move {
            replay_relist(
                self.store(),
                command.seller_user_id,
                command.command_id.as_uuid(),
                command.listing_id,
                command.request_hash,
            )
            .await
        })
    }

    fn relist(&self, command: RelistCommand) -> ConsignmentFuture<'_, RelistReceipt> {
        Box::pin(async move {
            if command.command_id.as_uuid().is_nil() {
                return Err(ConsignmentError::InvalidCommandId);
            }
            let identity = pre_read_identity(self.store(), command.listing_id)
                .await
                .map_err(WriteError::into_application)?;
            let sellers: Vec<_> = identity.iter().map(|id| id.seller_user_id).collect();
            let equipment: Vec<_> = identity.iter().map(|id| id.equipment.clone()).collect();
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
                                DepositOperation::Relist,
                                DepositTarget::Listing(command.listing_id),
                                command.request_hash,
                                command.request_id.as_deref(),
                            )
                            .await?
                        {
                            DepositArbitration::Replay(stored) => replay(&stored),
                            DepositArbitration::First(writer) => {
                                relist_stage(repository, command, writer).await
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
}

/// 解码已有账本行：拒绝只恢复安全码，成功还原回执。
fn replay(stored: &StoredReceipt) -> Result<RelistOutcome, WriteError> {
    if stored.body.get("disposition").and_then(Value::as_str) == Some("rejected") {
        let code = stored.body.get("code").and_then(Value::as_str).unwrap_or_default();
        let rejection = decode_rejection(code)
            .ok_or_else(|| WriteError::invariant("consignment_deposit_rejection_code"))?;
        return Ok(RelistOutcome::Rejected(rejection));
    }
    Ok(RelistOutcome::Accepted(RelistReceipt::decode(stored)?))
}

async fn reject(
    writer: DepositWriter<'_, '_, '_>,
    rejection: DepositBusinessRejection,
    now: Timestamp,
) -> Result<RelistOutcome, WriteError> {
    writer.finish_rejection(DepositRejection::Business(rejection), now).await?;
    Ok(RelistOutcome::Rejected(rejection))
}

/// 领域拒绝到确定业务码；没有专门码的一律 `action_not_allowed`。
const fn business_rejection(error: &DepositError) -> DepositBusinessRejection {
    match error {
        DepositError::ListingNotActive => DepositBusinessRejection::ListingNotActive,
        DepositError::SeasonClosed | DepositError::SeasonUnknown => {
            DepositBusinessRejection::SeasonClosed
        }
        DepositError::SellerIneligible => DepositBusinessRejection::ParticipantIneligible,
        DepositError::EvidenceUnproven => DepositBusinessRejection::EvidenceUnproven,
        DepositError::EquipmentBusy => DepositBusinessRejection::EquipmentBusy,
        _ => DepositBusinessRejection::ActionNotAllowed,
    }
}

/// 重列判据需要的挂单事实；写端取自锁内行，读侧取自同一只读快照。
struct RelistSubject {
    listing_id: Uuid,
    listing_state: ListingState,
    service_epoch: i64,
    blacklist_count_at_register: i64,
}

/// 写端与读侧共用的手动重列判定事实。
struct ManualRelist {
    facts: RelistFacts,
    /// 当前代是已封闭、原件在库的 Sale 时给出（代序号，Sale 合同 ID）；其余为空。
    sealed: Option<(u64, Uuid)>,
    /// 存在未解除的运营止售；写端据此给出 `operations_hold` 拒绝码。
    hold: bool,
}

impl ManualRelist {
    /// 写端会接受这次手动重列：有可再售的上代，且领域判定为重列。
    fn allowed(&self) -> bool {
        self.sealed.is_some() && matches!(decide_relist(&self.facts), RelistDecision::Relist { .. })
    }
}

/// 回库只发生在已承诺的 position 上；核对中、在途或已终结的件不可重列。
const fn relistable_position(state: PositionState) -> bool {
    matches!(state, PositionState::InCustody | PositionState::Committed)
}

/// 手动重列的前置事实装配：写端（锁内）与本人详情读侧（只读快照）共用这一份，读侧不另写判据。
///
/// 不变量 2、6：只读，不取锁、不写行；行锁由调用方在传入 `position` 与 `seller` 前自行决定。
async fn assess_manual_relist(
    connection: &mut PgConnection,
    subject: &RelistSubject,
    position: &WithdrawalPosition,
    seller: Option<SellerEligibility>,
    max_open_per_user: i64,
    now: Timestamp,
) -> Result<ManualRelist, WriteError> {
    let world = parse_settlement_world(&position.world_id)
        .ok_or_else(|| WriteError::invariant("consignment_relist_world"))?;
    let window = position_season_window(connection, position).await?;
    let season = season_state(world, window.as_ref(), now);
    let generation = current_generation(connection, position).await?;
    // 只有当前代是已封闭、原件在库的 Sale 才有“再售资格”；其余没有处置依据，一律禁止。
    let (relist, sealed) = match generation {
        CurrentGeneration::SaleSealed { generation, predecessor_proof } => {
            let closure_kind: String = sqlx::query_scalar(
                "SELECT closure_kind FROM public.consignment_sale_closures WHERE contract_id = $1",
            )
            .bind(predecessor_proof)
            .fetch_one(&mut *connection)
            .await?;
            // 未付取消可自动重列；已付退款后的重列只准手动（D9 §5.1）。
            let eligibility = match closure_kind.as_str() {
                "cancelled_unpaid" => RelistEligibility::ManualOrAuto,
                _ => RelistEligibility::ManualOnly,
            };
            (eligibility, Some((generation, predecessor_proof)))
        }
        CurrentGeneration::Unsold | CurrentGeneration::SaleOpen => {
            (RelistEligibility::Forbidden, None)
        }
    };
    let held_reason: Option<String> =
        sqlx::query_scalar("SELECT reason_code FROM public.consignment_listings WHERE id = $1")
            .bind(subject.listing_id)
            .fetch_one(&mut *connection)
            .await?;
    let held_reason = held_reason.as_deref().map(HeldOffMarketReason::parse).transpose()?;
    let unknown_dispatch = unknown_dispatch_open(connection, position.position_id).await?;
    let hold = active_hold(connection, position.position_id).await?.is_some();
    let seller_eligible = seller.is_some_and(|seller| {
        seller.active
            && !seller.active_blacklist
            && seller.blacklist_count == subject.blacklist_count_at_register
    });
    let control = service_control(connection).await?;
    let quota = quota_available(
        connection,
        UserId::from_uuid(position.seller_user_id),
        now,
        max_open_per_user,
    )
    .await?;
    let facts = RelistFacts {
        listing_state: subject.listing_state,
        held_reason,
        active_hold: hold,
        relist,
        seller_eligible,
        quota_available: quota,
        service_enabled: control.as_ref().is_some_and(|control| control.admitting),
        season_state: season,
        original_in_custody: sealed.is_some(),
        predecessor_closed: sealed.is_some(),
        unknown_dispatch_open: unknown_dispatch,
        epoch_current: control
            .as_ref()
            .is_some_and(|control| control.enabled_epoch == subject.service_epoch),
        mode: RelistMode::Manual,
    };
    Ok(ManualRelist { facts, sealed, hold })
}

/// 不取锁读取卖家资格：与锁定读 `lock_seller_eligibility` 同一组 `listing_sql!` 片段，只少行锁子句。
async fn read_seller_eligibility(
    connection: &mut PgConnection,
    seller_user_id: UserId,
) -> Result<Option<SellerEligibility>, WriteError> {
    let row = sqlx::query_as::<_, (bool, bool, i64)>(concat!(
        "SELECT ",
        listing_sql!(seller_active),
        ", ",
        listing_sql!(active_blacklist),
        ", ",
        listing_sql!(blacklist_count),
        " FROM public.users s WHERE s.id = $1"
    ))
    .bind(seller_user_id.as_uuid())
    .fetch_optional(connection)
    .await?;
    Ok(row.map(|(active, active_blacklist, blacklist_count)| SellerEligibility {
        active,
        active_blacklist,
        blacklist_count,
    }))
}

/// 本人详情的 `relist_allowed`：在调用方的只读快照内不取锁读出同一组事实，交同一判定。
///
/// 只表达“此刻可以发起重列”；写端仍在锁内重读并裁决（不变量 2、6）。非本人、不存在、不在库下架或
/// 没有 position 一律为否；任何读取失败向上传播，由调用方失败关闭。
pub(crate) async fn owned_relist_allowed(
    connection: &mut PgConnection,
    seller_user_id: UserId,
    listing_id: Uuid,
    max_open_per_user: i64,
    now: Timestamp,
) -> Result<bool, WriteError> {
    let row = sqlx::query(
        "SELECT state, service_epoch, blacklist_count_at_register
         FROM public.consignment_listings WHERE id = $1 AND seller_user_id = $2",
    )
    .bind(listing_id)
    .bind(seller_user_id.as_uuid())
    .fetch_optional(&mut *connection)
    .await?;
    let Some(row) = row else { return Ok(false) };
    let state: &str = row.try_get("state")?;
    let subject = RelistSubject {
        listing_id,
        listing_state: state.parse().map_err(invalid_projection)?,
        service_epoch: row.try_get("service_epoch")?,
        blacklist_count_at_register: row.try_get("blacklist_count_at_register")?,
    };
    // 不在库下架的件领域判定必拒；提前返回只省去其余读取，不是另一条判据。
    if subject.listing_state != ListingState::HeldOffMarket {
        return Ok(false);
    }
    let Some(position) = read_withdrawal_position(connection, listing_id).await? else {
        return Ok(false);
    };
    if !relistable_position(position.state) {
        return Ok(false);
    }
    let seller =
        read_seller_eligibility(connection, UserId::from_uuid(position.seller_user_id)).await?;
    let assessed =
        assess_manual_relist(connection, &subject, &position, seller, max_open_per_user, now)
            .await?;
    Ok(assessed.allowed())
}

/// 不变量 2、3、6：锁内取事实 → 纯函数判定 → 回库、g+1 代、挂单投影、事件、回执同提。
async fn relist_stage(
    repository: &DepositRepository,
    command: &RelistCommand,
    mut writer: DepositWriter<'_, '_, '_>,
) -> Result<RelistOutcome, WriteError> {
    let now = database_now(writer.connection()).await?;
    let Some(current) = lock_listing(writer.connection(), command.listing_id).await? else {
        return reject(writer, DepositBusinessRejection::ListingNotActive, now).await;
    };
    if current.identity.seller_user_id != command.seller_user_id.as_uuid() {
        return reject(writer, DepositBusinessRejection::ListingNotActive, now).await;
    }
    if current.snapshot.state_version != command.expected_version {
        return reject(writer, DepositBusinessRejection::StaleVersion, now).await;
    }
    let Some(position) = lock_withdrawal_position(writer.connection(), command.listing_id).await?
    else {
        return reject(writer, DepositBusinessRejection::CustodyUnproven, now).await;
    };
    if !relistable_position(position.state) {
        return reject(writer, DepositBusinessRejection::CustodyUnproven, now).await;
    }
    let seller =
        seller_eligibility(writer.connection(), UserId::from_uuid(position.seller_user_id)).await?;
    let subject = RelistSubject {
        listing_id: current.listing_id,
        listing_state: current.snapshot.state,
        service_epoch: current.service_epoch,
        blacklist_count_at_register: current.blacklist_count_at_register,
    };
    let assessed = assess_manual_relist(
        writer.connection(),
        &subject,
        &position,
        seller,
        repository.settings().max_open_per_user,
        now,
    )
    .await?;
    let Some((predecessor, sale_contract_id)) = assessed.sealed else {
        return reject(writer, DepositBusinessRejection::ActionNotAllowed, now).await;
    };
    match decide_relist(&assessed.facts) {
        RelistDecision::Relist { .. } => {}
        // 手动模式不会产生“留库”；出现即装配错误，失败关闭。
        RelistDecision::Keep { .. } => {
            return reject(writer, DepositBusinessRejection::ActionNotAllowed, now).await;
        }
        RelistDecision::Reject(error) => {
            let rejection = if assessed.hold {
                DepositBusinessRejection::OperationsHold
            } else {
                business_rejection(&error)
            };
            return reject(writer, rejection, now).await;
        }
    }
    let command_id = writer.command_id();
    let mut version = position.version;
    if position.state == PositionState::Committed {
        version = recall_to_custody(
            writer.connection(),
            &position,
            sale_contract_id,
            command_id,
            (current.listing_id, current.snapshot.state_version.value()),
            now,
        )
        .await?;
    }
    append_successor_generation(
        writer.connection(),
        &position,
        version,
        predecessor,
        RELIST_PURPOSE,
        command_id,
        now,
    )
    .await?;
    let projection = transition(current.snapshot, ListingState::Available, None)?;
    writer
        .stage_listing_update(
            &current,
            projection,
            current.current_snapshot_id,
            "relisted",
            None,
            now,
        )
        .await?;
    let receipt = RelistReceipt::new(
        command.command_id,
        current.listing_id,
        projection.state_version,
        current.snapshot.price,
        predecessor + 1,
    );
    writer.finish(receipt.clone(), now).await?;
    Ok(RelistOutcome::Accepted(receipt))
}
