//! 将挂单、事件、回执及审计作为同一顶层事务的事实提交。

use auction_application::consignment::{
    ListingBusinessRejection, ListingDelistReason, ListingOperation, LookupTransientReason,
};
use auction_domain::consignment::{
    ListingChange, ListingPrice, ListingReason, ListingSnapshot, ListingState, ListingVersion,
};
use auction_time::Timestamp;
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::ledger::{
    Actor, CommandWriter, RegistrationProof, StoredReceipt, Target,
};
use crate::database::consignment::commands::termination::append_collision_sets;
use crate::database::consignment::listings::projection::{
    ListingLockIdentity, LockedListing, NewListing, insert, update,
};
use crate::database::operations::audit_health::AuditWriteResult as _;

/// 有挂单版本变化的操作。Admin 下架处置必须携带已校验的私有说明。
pub(crate) enum ChangedOperation<'a> {
    Register,
    Reprice,
    Withdraw,
    ApplyLookup,
    AutoClose,
    AdminDelist(&'a ListingDelistReason),
}

impl ChangedOperation<'_> {
    const fn operation(&self) -> ListingOperation {
        match self {
            Self::Register => ListingOperation::Register,
            Self::Reprice => ListingOperation::Reprice,
            Self::Withdraw => ListingOperation::Withdraw,
            Self::ApplyLookup => ListingOperation::ApplyLookup,
            Self::AutoClose => ListingOperation::AutoClose,
            Self::AdminDelist(_) => ListingOperation::AdminDelist,
        }
    }
}

/// 调用方已依据领域模型决定的首次结果，只有固定处置可写入账本。
pub(crate) enum Decision<'a> {
    /// 挂单有新版本。回执取本次版本事件的字段。
    Changed { listing_id: Uuid, operation: ChangedOperation<'a> },
    /// 已核实的同价，无挂单版本变化。
    PriceUnchanged(&'a LockedListing),
    /// 工作重排，无挂单版本变化。
    LookupRescheduled {
        listing_id: Uuid,
        next_attempt_at: Timestamp,
        reason: LookupTransientReason,
    },
    /// 自动关闭只追加碰撞或没有新事实。
    AutoCloseObserved { listing_id: Uuid, conflicts_recorded: bool },
    /// 已仲裁的拒绝。非本人、缺失以及登记拒绝使用无挂单外键。
    Rejected { listing_id: Option<Uuid>, reason: ListingBusinessRejection },
}

pub(super) struct PendingEvent {
    listing_id: Uuid,
    from_state: Option<ListingState>,
    projection: ListingSnapshot,
    change: ListingChange,
    actor: &'static str,
    now: Timestamp,
}

impl CommandWriter<'_, '_> {
    /// 首次登记投影。不变量 2、3、10：必须持卖家与装备会话锁，冻结策略与后续回执同提交。
    pub(crate) async fn stage_registration(
        &mut self,
        new: NewListing<'_>,
        projection: ListingSnapshot,
        now: Timestamp,
    ) -> Result<(), WriteError> {
        self.require_locks(new.identity, true)?;
        if self.operation != ListingOperation::Register
            || !registration_actor_matches(self.actor, new.identity)
            || projection.reason.is_some()
        {
            return Err(WriteError::invariant("consignment_registration_writer"));
        }
        let listing_id = new.listing_id;
        insert(self.tx.connection, new, projection, now).await?;
        self.events.push(PendingEvent {
            listing_id,
            projection,
            now,
            change: ListingChange::Registered,
            actor: self.actor.kind(),
            from_state: None,
        });
        Ok(())
    }

    /// 依据锁内领域迁移更新目标，暂存同版事件；系统触发的关闭使用 `stage_system_closure`。
    pub(crate) async fn stage_change(
        &mut self,
        current: &LockedListing,
        projection: ListingSnapshot,
        snapshot_id: Option<Uuid>,
        change: ListingChange,
        now: Timestamp,
    ) -> Result<(), WriteError> {
        self.require_locks(&current.identity, true)?;
        if !matches!(self.target, Target::Listing(listing_id) if listing_id == current.listing_id) {
            return Err(WriteError::invariant("consignment_change_target"));
        }
        self.stage_update(current, projection, snapshot_id, change, self.actor.kind(), now).await
    }

    /// 内联或独立的系统终结。原操作还须写入自身决定。允许登记或发布清退同件旧单，仅减少占用。
    pub(crate) async fn stage_system_closure(
        &mut self,
        current: &LockedListing,
        projection: ListingSnapshot,
        change: ListingChange,
        now: Timestamp,
    ) -> Result<(), WriteError> {
        let is_target =
            matches!(self.target, Target::Listing(listing_id) if listing_id == current.listing_id);
        self.require_locks(&current.identity, is_target)?;
        // 不变量 2：登记/发布可在同一 EID 锁内清退同件其他卖家的旧单。
        if !is_target
            && self
                .tx
                .keys
                .equipment
                .iter()
                .any(|equipment| equipment != &current.identity.equipment)
        {
            return Err(WriteError::invariant("consignment_cross_equipment_closure"));
        }
        if !matches!(change, ListingChange::Closed | ListingChange::Expired)
            || (!is_target
                && !matches!(
                    self.operation,
                    ListingOperation::Register | ListingOperation::ApplyLookup
                ))
        {
            return Err(WriteError::invariant("consignment_system_closure"));
        }
        self.stage_update(current, projection, current.current_snapshot_id, change, "system", now)
            .await
    }

    fn require_locks(
        &self,
        identity: &ListingLockIdentity,
        require_seller: bool,
    ) -> Result<(), WriteError> {
        if !self.tx.keys.equipment.contains(&identity.equipment)
            || (require_seller && !self.tx.keys.sellers.contains(&identity.seller_user_id))
        {
            return Err(WriteError::invariant("consignment_missing_business_lock"));
        }
        Ok(())
    }

    async fn stage_update(
        &mut self,
        current: &LockedListing,
        projection: ListingSnapshot,
        snapshot_id: Option<Uuid>,
        change: ListingChange,
        actor: &'static str,
        now: Timestamp,
    ) -> Result<(), WriteError> {
        if projection.mode != current.snapshot.mode
            || projection.state_version.value() != current.snapshot.state_version.value() + 1
        {
            return Err(WriteError::invariant("consignment_listing_transition"));
        }
        // 不变量 2、3、6：写入锁定的旧版本；清退不另取命令锁，事件引用外层 ID。
        update(self.tx.connection, current, projection, snapshot_id, now).await?;
        self.events.push(PendingEvent {
            listing_id: current.listing_id,
            projection,
            now,
            change,
            actor,
            from_state: Some(current.snapshot.state),
        });
        Ok(())
    }

    /// 写原操作回执、暂存的同版事件和审计。已仲裁拒绝也返回 Ok，交由事务 owner 提交。
    /// 此调用后可在同一事务追加引用本账本行的碰撞关联，再将结果交还事务 owner。
    /// 不变量 1、3、8：任意事实写失败必须向事务 owner 返回错误，不能继续或提交。
    pub(crate) async fn finish(
        self,
        decision: Decision<'_>,
        now: Timestamp,
    ) -> Result<StoredReceipt, WriteError> {
        self.finish_with_collisions(decision, &[], now).await
    }

    /// 首次登记的回执与调用方计算的 MAC 同事务保存。
    pub(crate) async fn finish_registration(
        self,
        proof: RegistrationProof,
        decision: Decision<'_>,
        now: Timestamp,
    ) -> Result<StoredReceipt, WriteError> {
        self.finish_with_proof(Some(proof), decision, &[], now).await
    }

    /// 定案后同步追加碰撞集合，保留原操作的同事务审计。不变量 1、3、8。
    pub(super) async fn finish_with_collisions(
        self,
        decision: Decision<'_>,
        collisions: &[Uuid],
        now: Timestamp,
    ) -> Result<StoredReceipt, WriteError> {
        self.finish_with_proof(None, decision, collisions, now).await
    }

    /// 首次登记定案与旧占用碰撞集合同时提交。
    pub(super) async fn finish_registration_with_collisions(
        self,
        proof: RegistrationProof,
        decision: Decision<'_>,
        collisions: &[Uuid],
        now: Timestamp,
    ) -> Result<StoredReceipt, WriteError> {
        if collisions.is_empty() {
            return self.finish_registration(proof, decision, now).await;
        }
        self.finish_with_proof(Some(proof), decision, collisions, now).await
    }

    async fn finish_with_proof(
        mut self,
        registration: Option<RegistrationProof>,
        decision: Decision<'_>,
        collisions: &[Uuid],
        now: Timestamp,
    ) -> Result<StoredReceipt, WriteError> {
        let (listing_id, result) = self.receipt(&decision)?;
        let command_id = self.tx.command_id;
        self.insert_receipt(registration, listing_id, &result, now).await?;
        for event in &self.events {
            append_event(self.tx.connection, command_id, event).await?;
            if event.actor == "system" && self.actor.kind() != "system" {
                audit(
                    self.tx.connection,
                    Audit {
                        actor: Actor::System,
                        request_id: None,
                        action: if matches!(event.change, ListingChange::Expired) {
                            "consignment.expired"
                        } else {
                            "consignment.closed"
                        },
                        listing_id: Some(event.listing_id),
                        accepted: true,
                        now: event.now,
                        command_id,
                        metadata: json!({"command_id": command_id, "reason_code": event.projection.reason.map(ListingReason::as_str)}),
                    },
                )
                .await?;
            }
        }
        let mut metadata = json!({
            "command_id": command_id,
            "code": result.code,
            "disposition": result.body["disposition"],
            "reason_code": result.body["reason_code"],
        });
        if let Decision::Changed { operation: ChangedOperation::AdminDelist(reason), .. } = decision
        {
            // 不变量 7：仅经过领域端口筛查的私有说明进入审计，不进入账本回执或诊断。
            metadata["admin_delist_reason"] = json!(reason.as_str());
        }
        audit(
            self.tx.connection,
            Audit {
                actor: self.actor,
                action: operation_action(self.operation),
                request_id: self.request_id,
                listing_id,
                accepted: result.status < 400,
                now,
                command_id,
                metadata,
            },
        )
        .await?;
        append_collision_sets(self.tx.connection, command_id, collisions, now).await?;
        Ok(result)
    }

    fn receipt(
        &self,
        decision: &Decision<'_>,
    ) -> Result<(Option<Uuid>, StoredReceipt), WriteError> {
        if matches!(
            decision,
            Decision::Rejected {
                listing_id: Some(_),
                reason: ListingBusinessRejection::ListingNotFound,
            }
        ) {
            return Err(WriteError::invariant("consignment_private_rejection_target"));
        }
        // 不变量 3、6：成功决定必须对应已仲裁操作，Admin 下架的说明在类型上必填。
        let decision_operation = match decision {
            Decision::Changed { operation, .. } => Some(operation.operation()),
            Decision::PriceUnchanged(_) => Some(ListingOperation::Reprice),
            Decision::LookupRescheduled { .. } => Some(ListingOperation::ApplyLookup),
            Decision::AutoCloseObserved { .. } => Some(ListingOperation::AutoClose),
            Decision::Rejected { .. } => None,
        };
        if decision_operation.is_some_and(|operation| operation != self.operation) {
            return Err(WriteError::invariant("consignment_command_decision_operation"));
        }
        let (status, code) = match decision {
            Decision::Rejected { reason, .. } => (reason.status().cast_signed(), reason.code()),
            _ if self.operation == ListingOperation::Register => (202, "accepted"),
            _ => (200, "ok"),
        };
        let (listing_id, disposition, version, state, price, extra) = match decision {
            Decision::Changed { listing_id, .. } => {
                let event =
                    self.events.iter().find(|event| event.listing_id == *listing_id).ok_or_else(
                        || WriteError::invariant("consignment_command_version_event"),
                    )?;
                let projection = event.projection;
                let extra = if matches!(
                    self.operation,
                    ListingOperation::ApplyLookup | ListingOperation::AutoClose
                ) {
                    ReceiptExtra::TerminalOrPublished(projection)
                } else {
                    ReceiptExtra::None
                };
                (
                    Some(*listing_id),
                    event.change.as_str(),
                    Some(projection.state_version),
                    Some(projection.state),
                    Some(projection.price),
                    extra,
                )
            }
            Decision::PriceUnchanged(current) => {
                self.require_locks(&current.identity, true)?;
                (
                    Some(current.listing_id),
                    "unchanged",
                    Some(current.snapshot.state_version),
                    Some(current.snapshot.state),
                    Some(current.snapshot.price),
                    ReceiptExtra::None,
                )
            }
            Decision::LookupRescheduled { listing_id, next_attempt_at, reason } => (
                Some(*listing_id),
                "rescheduled",
                None,
                None,
                None,
                ReceiptExtra::Rescheduled(*next_attempt_at, *reason),
            ),
            Decision::AutoCloseObserved { listing_id, conflicts_recorded } => (
                Some(*listing_id),
                if *conflicts_recorded { "conflicts_recorded" } else { "unchanged" },
                None,
                None,
                None,
                ReceiptExtra::None,
            ),
            Decision::Rejected { listing_id, .. } => {
                (*listing_id, "rejected", None, None, None, ReceiptExtra::None)
            }
        };

        let mut body = json!({
            "command_id": self.tx.command_id,
            "operation": self.operation.as_str(),
            "listing_id": listing_id,
            "listing_version": version.map(ListingVersion::value),
            "state": state.map(ListingState::as_str),
            "disposition": disposition,
            "price": price.map(ListingPrice::value),
        });
        extra.write(&mut body);
        Ok((listing_id, StoredReceipt { status, code: code.into(), body }))
    }
}

enum ReceiptExtra {
    None,
    TerminalOrPublished(ListingSnapshot),
    Rescheduled(Timestamp, LookupTransientReason),
}

impl ReceiptExtra {
    fn write(self, body: &mut Value) {
        match self {
            Self::None => {}
            Self::TerminalOrPublished(projection) => {
                body["expires_at"] = json!(projection.expires_at);
                body["reason_code"] = json!(projection.reason.map(ListingReason::as_str));
            }
            Self::Rescheduled(next_attempt_at, reason) => {
                body["next_attempt_at"] = json!(next_attempt_at);
                body["last_reason"] = json!(reason.as_str());
            }
        }
    }
}

fn operation_action(operation: ListingOperation) -> &'static str {
    match operation {
        ListingOperation::Register => "consignment.register",
        ListingOperation::Reprice => "consignment.reprice",
        ListingOperation::Withdraw => "consignment.withdraw",
        ListingOperation::ApplyLookup => "consignment.apply_lookup",
        ListingOperation::AutoClose => "consignment.auto_close",
        ListingOperation::AdminDelist => "consignment.admin_delist",
    }
}

async fn append_event(
    connection: &mut PgConnection,
    command_id: Uuid,
    event: &PendingEvent,
) -> Result<(), WriteError> {
    append_event_row(
        connection,
        command_id,
        event.listing_id,
        event.projection.state_version.value(),
        event.actor,
        event.change.as_str(),
        event.from_state,
        event.projection.state,
        event.projection.reason.map(ListingReason::as_str),
        event.projection.price.value(),
        event.now,
    )
    .await
}

/// v1/v2 共用的持久事件写入；列由调用方按同版本事实冻结。
#[allow(clippy::too_many_arguments, reason = "事件列与 v1/v2 共用绑定一一对应")]
pub(super) async fn append_event_row(
    connection: &mut PgConnection,
    command_id: Uuid,
    listing_id: Uuid,
    state_version: u64,
    actor: &str,
    event_kind: &str,
    from_state: Option<ListingState>,
    to_state: ListingState,
    reason: Option<&str>,
    price: i64,
    occurred_at: Timestamp,
) -> Result<(), WriteError> {
    sqlx::query(
        "INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(listing_id)
    .bind(state_version.cast_signed())
    .bind(command_id)
    .bind(actor)
    .bind(event_kind)
    .bind(from_state.map(ListingState::as_str))
    .bind(to_state.as_str())
    .bind(reason)
    .bind(price)
    .bind(occurred_at)
    .execute(connection)
    .await?;
    Ok(())
}

/// 同一事务内的追加审计；v1/v2 共用同一敏感值边界。
pub(super) struct Audit<'a> {
    pub(super) actor: Actor<'a>,
    pub(super) request_id: Option<&'a str>,
    pub(super) action: &'static str,
    pub(super) listing_id: Option<Uuid>,
    pub(super) accepted: bool,
    pub(super) now: Timestamp,
    pub(super) command_id: Uuid,
    pub(super) metadata: Value,
}

pub(super) async fn audit(
    connection: &mut PgConnection,
    audit: Audit<'_>,
) -> Result<(), WriteError> {
    sqlx::query(
        "INSERT INTO public.audit_records
            (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id, result,
             request_id, source, metadata)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(Uuid::new_v4())
    .bind(audit.now)
    .bind(audit.actor.audit_kind())
    .bind(audit.actor.audit_reference())
    .bind(audit.action)
    .bind(if audit.listing_id.is_some() { "consignment_listing" } else { "consignment_command" })
    .bind(audit.listing_id.unwrap_or(audit.command_id).to_string())
    .bind(if audit.accepted { "success" } else { "rejected" })
    .bind(audit.request_id)
    .bind(audit.actor.audit_source())
    .bind(audit.metadata)
    .execute(connection)
    .await
    .record_audit_failure()?;
    Ok(())
}

/// 登记写入者必须是挂单记录的卖家：账户挂单比较账户 ID，匿名挂单逐字节比较自填名。
fn registration_actor_matches(actor: Actor<'_>, identity: &ListingLockIdentity) -> bool {
    match (actor, identity.seller_game_name.as_deref()) {
        (Actor::PublicUser(seller), None) => seller == identity.seller_user_id,
        (Actor::Anonymous(name), Some(listed)) => name == listed,
        _ => false,
    }
}
