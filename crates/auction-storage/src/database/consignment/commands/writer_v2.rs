//! v2 命令的同一事务写入：挂单、事件、回执与审计一次提交。

use auction_application::consignment::{
    DepositActor, DepositBusinessRejection, DepositOperation, ListingStateChange,
};
use auction_application::core::RequestHash;
use auction_domain::consignment::{ListingSnapshot, ListingState};
use auction_time::Timestamp;
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use super::errors::WriteError;
use super::facts::{Audit, append_event_row, audit};
use super::ledger::{Actor, DepositTarget, StoredReceipt};
use super::receipts_v2::DepositReceipt;
use super::session::DepositTransaction;
use crate::database::consignment::listings::projection::LockedListing;
use crate::database::pool::TrackedTransaction;

/// 一条 v2 命令暂存到内存、待账本行写入后统一追加的事件。
pub(crate) struct DepositEvent {
    listing_id: Uuid,
    from_state: Option<ListingState>,
    projection: ListingSnapshot,
    event_kind: &'static str,
    reason_code: Option<&'static str>,
    actor: &'static str,
    now: Timestamp,
}

/// 已通过账本仲裁的首次写入能力。事件在账本行写入前只驻留内存。
pub(crate) struct DepositWriter<'w, 'tx, 'conn> {
    pub(super) tx: &'w mut DepositTransaction<'tx, 'conn>,
    pub(super) actor: DepositActor,
    pub(super) operation: DepositOperation,
    pub(super) target: DepositTarget,
    pub(super) events: Vec<DepositEvent>,
    pub(super) request_hash: RequestHash,
    pub(super) request_id: Option<&'w str>,
}

impl<'w, 'tx, 'conn> DepositWriter<'w, 'tx, 'conn> {
    /// 供业务 SQL 使用同一顶层事务连接。
    pub(crate) fn connection(&mut self) -> &mut PgConnection {
        self.tx.connection()
    }

    /// 当前事务的命令 ID；命令行由 [`Self::finish`] 最后写入，子事实可先引用。
    pub(crate) const fn command_id(&self) -> Uuid {
        self.tx.command_id()
    }

    /// 供两个旧 owner 的事务内原语使用同一持有事务。
    pub(crate) fn tracked(&mut self) -> &mut TrackedTransaction<'conn, sqlx::Postgres> {
        self.tx.tracked()
    }

    /// 锁内更新一条挂单并暂存同版本事件；调用方必须已持有卖家与实物锁。
    pub(crate) async fn stage_listing_update(
        &mut self,
        current: &LockedListing,
        projection: ListingSnapshot,
        snapshot_id: Option<Uuid>,
        event_kind: &'static str,
        reason_code: Option<&'static str>,
        now: Timestamp,
    ) -> Result<(), WriteError> {
        if !self.tx.keys.equipment.contains(&current.identity.equipment)
            || !self.tx.keys.sellers.contains(&current.identity.seller_user_id)
        {
            return Err(WriteError::invariant("consignment_deposit_missing_business_lock"));
        }
        if projection.mode != current.snapshot.mode
            || projection.state_version.value() != current.snapshot.state_version.value() + 1
        {
            return Err(WriteError::invariant("consignment_deposit_listing_transition"));
        }
        // 不变量 2、3、6：只更新锁定的旧版本；事件与回执由账本提交前统一复核。
        update_v2(self.tx.connection(), current, projection, snapshot_id, reason_code, now).await?;
        self.events.push(DepositEvent {
            listing_id: current.listing_id,
            from_state: Some(current.snapshot.state),
            projection,
            event_kind,
            reason_code,
            actor: self.actor.kind(),
            now,
        });
        Ok(())
    }

    /// 已暂存事件的逐件 L，供 4A 回执直接使用；顺序为业务写入顺序。
    pub(crate) fn listing_changes(&self) -> Vec<ListingStateChange> {
        self.events
            .iter()
            .map(|event| ListingStateChange {
                listing_id: event.listing_id,
                listing_version: event.projection.state_version.value(),
                state: event.projection.state,
            })
            .collect()
    }

    /// 测试入口为纯账本回执补一条不更新挂单的事件；命令触发器仍能看到同版本事件。
    #[cfg(feature = "test-db")]
    #[allow(clippy::too_many_arguments, reason = "测试事件列与生产事件行一一对应")]
    pub(crate) async fn stage_synthetic_event(
        &mut self,
        listing_id: Uuid,
        state_version: u64,
        from_state: Option<ListingState>,
        state: ListingState,
        event_kind: &'static str,
        reason: Option<&'static str>,
        now: Timestamp,
    ) -> Result<(), WriteError> {
        let state_version = auction_domain::consignment::ListingVersion::new(state_version)
            .map_err(super::errors::invalid_projection)?;
        let affected = sqlx::query(
            "UPDATE public.consignment_listings
             SET state = $3, state_version = $4, reason_code = $5, updated_at = $6
             WHERE id = $1 AND state_version = $4 - 1 AND state = $2",
        )
        .bind(listing_id)
        .bind(from_state.map(ListingState::as_str))
        .bind(state.as_str())
        .bind(state_version.value().cast_signed())
        .bind(reason)
        .bind(now)
        .execute(self.tx.connection())
        .await?;
        if affected.rows_affected() != 1 {
            return Err(WriteError::invariant("consignment_deposit_synthetic_listing"));
        }
        self.events.push(DepositEvent {
            listing_id,
            from_state,
            projection: ListingSnapshot {
                mode: auction_domain::consignment::ListingMode::Consigned,
                state,
                state_version,
                price: auction_domain::consignment::ListingPrice::new(101)
                    .map_err(super::errors::invalid_projection)?,
                expires_at: now,
                reason: None,
            },
            event_kind,
            reason_code: reason,
            actor: self.actor.kind(),
            now,
        });
        Ok(())
    }

    /// 写入成功回执、全部同事务事件与追加审计；任一失败由事务 owner 整体回滚。
    pub(crate) async fn finish<R: DepositReceipt>(
        mut self,
        receipt: R,
        now: Timestamp,
    ) -> Result<StoredReceipt, WriteError> {
        if receipt.operation() != self.operation
            || receipt.command_id().as_uuid() != self.tx.command_id
        {
            return Err(WriteError::invariant("consignment_deposit_receipt_operation"));
        }
        let changes = receipt.listing_changes();
        for change in &changes {
            let matched = self.events.iter().any(|event| {
                event.listing_id == change.listing_id
                    && event.projection.state_version.value() == change.listing_version
                    && event.projection.state == change.state
            });
            if !matched {
                return Err(WriteError::invariant("consignment_deposit_receipt_listing_change"));
            }
        }
        let body = receipt.body();
        let result = StoredReceipt { status: receipt.status(), code: "ok".into(), body };
        self.insert("ok", &result, now).await?;
        let command_id = self.tx.command_id;
        for event in &self.events {
            append_event_row(
                self.tx.connection(),
                command_id,
                event.listing_id,
                event.projection.state_version.value(),
                event.actor,
                event.event_kind,
                event.from_state,
                event.projection.state,
                event.reason_code,
                event.projection.price.value(),
                event.now,
            )
            .await?;
        }
        let metadata = json!({
            "command_id": command_id,
            "code": result.code,
            "disposition": self.operation_body_disposition(&result.body),
            "held_reason": result.body.get("held_reason").cloned(),
        });
        let command_id = self.tx.command_id;
        audit(
            self.tx.connection(),
            Audit {
                actor: audit_actor(&self.actor),
                request_id: self.request_id,
                action: operation_action(self.operation),
                listing_id: None,
                accepted: result.status < 400,
                now,
                command_id,
                metadata,
            },
        )
        .await?;
        Ok(result)
    }

    /// 已仲裁的确定拒绝：只写安全回执与拒绝审计，不追加业务事件。
    pub(crate) async fn finish_rejection(
        mut self,
        rejection: DepositRejection,
        now: Timestamp,
    ) -> Result<StoredReceipt, WriteError> {
        let result = StoredReceipt {
            status: rejection.status(),
            code: rejection.code().into(),
            body: json!({
                "schema_version": 2,
                "operation": self.operation.as_str(),
                "disposition": "rejected",
                "command_id": self.tx.command_id.to_string(),
                "code": rejection.code(),
            }),
        };
        self.insert(rejection.code(), &result, now).await?;
        let command_id = self.tx.command_id;
        audit(
            self.tx.connection(),
            Audit {
                actor: audit_actor(&self.actor),
                request_id: self.request_id,
                action: operation_action(self.operation),
                listing_id: None,
                accepted: false,
                now,
                command_id,
                metadata: json!({
                    "command_id": command_id,
                    "code": rejection.code(),
                    "disposition": "rejected",
                }),
            },
        )
        .await?;
        Ok(result)
    }

    fn operation_body_disposition<'a>(&self, body: &'a Value) -> &'a str {
        body.get("disposition").and_then(Value::as_str).unwrap_or("unknown")
    }

    async fn insert(
        &mut self,
        code: &str,
        result: &StoredReceipt,
        now: Timestamp,
    ) -> Result<(), WriteError> {
        let (target_kind, target_id, bot_uid, world, mid, season, calendar) = self.target.columns();
        let listing_id = match &self.target {
            DepositTarget::Listing(listing_id) => Some(*listing_id),
            _ => None,
        };
        // 不变量 1、3、7、8：回执、类型化目标与命令身份同一条插入；目标列不复制正文。
        sqlx::query(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, replay_key_id, listing_id, result_status, result_code,
                 result_body, created_at, target_kind, target_id, target_bot_uid,
                 target_world_id, target_external_mid, target_season_id,
                 target_calendar_version)
             VALUES ($1, 2, $2, $3, $4, $5, $6, NULL, $7, $8, $9, $10, $11, $12, $13, $14,
                     $15, $16, $17, $18)",
        )
        .bind(self.tx.command_id)
        .bind(self.actor.kind())
        .bind(super::ledger::actor_reference(&self.actor))
        .bind(self.operation.as_str())
        .bind(self.target.request_target())
        .bind(self.request_hash.as_bytes())
        .bind(listing_id)
        .bind(result.status)
        .bind(code)
        .bind(&result.body)
        .bind(now)
        .bind(target_kind)
        .bind(target_id)
        .bind(bot_uid)
        .bind(world)
        .bind(mid)
        .bind(season)
        .bind(calendar)
        .execute(self.tx.connection())
        .await
        .map_err(WriteError::from)?;
        Ok(())
    }

    /// 测试专用的裸回执写入：仍走同一账本与约束，不绕过 v2 目标形状。
    #[cfg(feature = "test-db")]
    pub(crate) async fn finish_raw(
        mut self,
        status: i16,
        body: Value,
        now: Timestamp,
    ) -> Result<StoredReceipt, WriteError> {
        let result = StoredReceipt { status, code: "ok".into(), body };
        self.insert("ok", &result, now).await?;
        let command_id = self.tx.command_id;
        for event in &self.events {
            append_event_row(
                self.tx.connection(),
                command_id,
                event.listing_id,
                event.projection.state_version.value(),
                event.actor,
                event.event_kind,
                event.from_state,
                event.projection.state,
                event.reason_code,
                event.projection.price.value(),
                event.now,
            )
            .await?;
        }
        Ok(result)
    }
}

/// v2 可持久化的确定拒绝闭集；新增值必须同时扩 `000134` 的结果码 CHECK。
#[derive(Debug, Clone, Copy)]
#[allow(dead_code, reason = "部分拒绝要到第 5–7 期写路径才会构造；落库形状已由 PG 测试覆盖")]
pub(crate) enum DepositRejection {
    /// 4A 的确定业务拒绝。
    Business(DepositBusinessRejection),
    /// 两世界活动额度已满。
    ListingLimitReached,
    /// 挂单已承诺给买家。
    EquipmentCommitted,
    /// 缺少可证明的代管。
    CustodyUnproven,
    /// 当前存在运营止售。
    OperationsHold,
    /// 新业务已关闭。
    Disabled,
}

impl DepositRejection {
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::Business(rejection) => rejection.code(),
            Self::ListingLimitReached => "listing_limit_reached",
            Self::EquipmentCommitted => "equipment_committed",
            Self::CustodyUnproven => "custody_unproven",
            Self::OperationsHold => "operations_hold",
            Self::Disabled => "consignment_disabled",
        }
    }

    const fn status(self) -> i16 {
        match self {
            Self::Business(rejection) => rejection.status() as i16,
            Self::ListingLimitReached => 429,
            Self::EquipmentCommitted | Self::CustodyUnproven | Self::OperationsHold => 409,
            Self::Disabled => 404,
        }
    }
}

impl From<DepositBusinessRejection> for DepositRejection {
    fn from(rejection: DepositBusinessRejection) -> Self {
        Self::Business(rejection)
    }
}

/// v2 投影更新允许 `held_off_market` 的原始原因码；值仍由 000134 触发器与 CHECK 复核。
async fn update_v2(
    connection: &mut PgConnection,
    current: &LockedListing,
    projection: ListingSnapshot,
    current_snapshot_id: Option<Uuid>,
    reason_code: Option<&str>,
    now: Timestamp,
) -> Result<(), WriteError> {
    let result = sqlx::query(
        "UPDATE public.consignment_listings
         SET state = $3, state_version = $4, price = $5, current_snapshot_id = $6,
             reason_code = $7, updated_at = $8, expires_at = $9
         WHERE id = $1 AND state_version = $2",
    )
    .bind(current.listing_id)
    .bind(current.snapshot.state_version.value().cast_signed())
    .bind(projection.state.as_str())
    .bind(projection.state_version.value().cast_signed())
    .bind(projection.price.value())
    .bind(current_snapshot_id)
    .bind(reason_code)
    .bind(now)
    .bind(projection.expires_at)
    .execute(connection)
    .await?;
    if result.rows_affected() != 1 {
        return Err(WriteError::invariant("consignment_locked_listing_version"));
    }
    Ok(())
}

const fn operation_action(operation: DepositOperation) -> &'static str {
    match operation {
        DepositOperation::IssueReservation => "consignment.issue_reservation",
        DepositOperation::ExpireReservation => "consignment.expire_reservation",
        DepositOperation::DecideEnvelope => "consignment.decide_envelope",
        DepositOperation::AppendQuarantineObservation => {
            "consignment.append_quarantine_observation"
        }
        DepositOperation::RecheckQuarantine => "consignment.recheck_quarantine",
        DepositOperation::ReleaseQuarantine => "consignment.release_quarantine",
        DepositOperation::StartDispositionWrite => "consignment.start_disposition_write",
        DepositOperation::ApplyMailProof => "consignment.apply_mail_proof",
        DepositOperation::Withdraw => "consignment.withdraw",
        DepositOperation::AdminDelist => "consignment.admin_delist",
        DepositOperation::ReleaseOperationsHold => "consignment.release_hold",
        DepositOperation::SeasonNotice => "consignment.season_notice",
        DepositOperation::SeasonClose => "consignment.season_close",
    }
}

fn audit_actor(actor: &DepositActor) -> Actor<'_> {
    match actor {
        DepositActor::PublicUser(user_id) => Actor::PublicUser(user_id.as_uuid()),
        DepositActor::Admin(actor) => Actor::Admin(actor),
        DepositActor::Worker => Actor::Worker,
        DepositActor::System => Actor::System,
    }
}
