//! 续办动作事实与权利状态刷新：只产生服务端当前可执行的动作子集。
//!
//! 保护不变量 2、6、8：动作子集由领域 [`allowed_actions`] 对锁内读到的订单事实求出；状态只追加新版本
//! （`consignment_right_states` 追加触发器），绝不改写历史；已离开 `active` 的权利（`fulfilled` /
//! `expired` / `revoked`）是终态，刷新不再改动。`claim_accepted` 本身只落回执，不触发任何资产或
//! 资金外写：履约仍由后台自动路径在冻结受益人下执行（见 [`super::entitlement`]），金额与收件人
//! 永远来自冻结的合同与义务，会话无从选择。

use auction_application::consignment::RightsStoreError;
use auction_domain::consignment::rights::{
    RightActionFacts, RightKind, RightRole, allowed_actions, right_id,
};
use auction_time::Timestamp;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::{PgRightsStore, TxError, lock_key};
use crate::database::consignment::commands::errors::WriteError;

/// 订单义务事实的汇总：与权利种类对应的义务里，仍待办的与已满足的条数。
struct ObligationCounts {
    /// 业务态 `ready` 且履约尚未终结。
    executable: i64,
    /// 非休眠义务里履约尚未终结的条数。
    unfinished: i64,
    /// 已满足的条数。
    satisfied: i64,
}

/// 权利种类对应的义务种类；位置权利与损失权利没有义务。
const fn obligation_kinds(kind: RightKind) -> &'static [&'static str] {
    match kind {
        RightKind::ContinueOrder => &[
            "winner_equipment_cod",
            "winner_payment_carrier_cod",
            "winner_equipment_handoff",
            "buyer_refund",
        ],
        RightKind::ClaimPayment => &["seller_payout"],
        RightKind::ClaimEquipment => &["withdrawn_return"],
        RightKind::WithdrawHeld | RightKind::LossCase => &[],
    }
}

fn parse_kind(text: &str) -> Option<RightKind> {
    [
        RightKind::ContinueOrder,
        RightKind::ClaimEquipment,
        RightKind::ClaimPayment,
        RightKind::WithdrawHeld,
        RightKind::LossCase,
    ]
    .into_iter()
    .find(|kind| kind.as_str() == text)
}

async fn contract_counts(
    connection: &mut PgConnection,
    contract_id: Uuid,
    kind: RightKind,
) -> Result<ObligationCounts, WriteError> {
    let kinds: Vec<&str> = obligation_kinds(kind).to_vec();
    let row = sqlx::query(
        "SELECT
             count(*) FILTER (WHERE obligation.business_state = 'ready'
                 AND fulfillment.execution_state IN ('pending', 'prepared', 'enqueued')) AS executable,
             count(*) FILTER (WHERE obligation.business_state <> 'contingent'
                 AND fulfillment.execution_state IN ('pending', 'prepared', 'enqueued')) AS unfinished,
             count(*) FILTER (WHERE fulfillment.execution_state = 'satisfied') AS satisfied
         FROM public.settlement_obligations AS obligation
         JOIN public.settlement_obligation_fulfillments AS fulfillment
           ON fulfillment.obligation_id = obligation.id
         WHERE obligation.contract_id = $1 AND obligation.obligation_kind = ANY($2)",
    )
    .bind(contract_id)
    .bind(&kinds)
    .fetch_one(connection)
    .await?;
    Ok(ObligationCounts {
        executable: row.try_get("executable")?,
        unfinished: row.try_get("unfinished")?,
        satisfied: row.try_get("satisfied")?,
    })
}

/// 权利应处的状态与动作子集。
struct Desired {
    state: &'static str,
    actions: Vec<String>,
}

async fn desired_state(
    connection: &mut PgConnection,
    kind: RightKind,
    contract_id: Option<Uuid>,
    position_id: Option<Uuid>,
) -> Result<Desired, WriteError> {
    let mut facts = RightActionFacts { active: true, ..RightActionFacts::default() };
    let mut state = "active";
    match (kind, contract_id, position_id) {
        (RightKind::LossCase, _, _) => {}
        (RightKind::WithdrawHeld, _, Some(position_id)) => {
            let position: Option<String> = sqlx::query_scalar(
                "SELECT state FROM public.consignment_custody_positions WHERE id = $1",
            )
            .bind(position_id)
            .fetch_optional(connection)
            .await?;
            match position.as_deref() {
                Some("in_custody") => facts.withdraw_held = true,
                Some("handed_over") => state = "fulfilled",
                Some("season_ended" | "lost") => state = "expired",
                _ => {}
            }
        }
        (_, Some(contract_id), _) if !obligation_kinds(kind).is_empty() => {
            let counts = contract_counts(connection, contract_id, kind).await?;
            if counts.unfinished == 0 && counts.satisfied > 0 {
                state = "fulfilled";
            } else {
                let executable = counts.executable > 0;
                facts.continue_order = executable;
                facts.claim_payment = executable;
                facts.claim_equipment = executable;
            }
        }
        _ => return Err(WriteError::invariant("consignment_right_target_shape")),
    }
    facts.active = state == "active";
    let actions = allowed_actions(kind, &facts)
        .into_iter()
        .map(|action| action.as_str().to_owned())
        .collect();
    Ok(Desired { state, actions })
}

/// 事务内列出合同名下的全部权利（Sale 的买方 / 卖方，Withdrawal 的卖方），按 `right_id` 排序，
/// 让同一合同的刷新顺序固定。只读，不加锁：调用方已持有合同所在写口的锁。
pub(crate) async fn contract_right_ids_in_tx(
    connection: &mut PgConnection,
    contract_id: Uuid,
) -> Result<Vec<Uuid>, WriteError> {
    Ok(sqlx::query_scalar(
        "SELECT right_id FROM public.consignment_rights WHERE contract_id = $1 ORDER BY right_id",
    )
    .bind(contract_id)
    .fetch_all(connection)
    .await?)
}

/// 事务内取一项权利的 advisory 锁：与 `refresh_contract_rights`、续办同一把锁、同一键，
/// 锁随事务结束释放。多项权利按 `right_id` 升序取（与权利事务按锁键文本排序的次序一致），不成环。
pub(crate) async fn lock_right_in_tx(
    connection: &mut PgConnection,
    right: Uuid,
) -> Result<(), WriteError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(lock_key("right", right.as_bytes()))
        .execute(connection)
        .await?;
    Ok(())
}

/// 刷新一项权利的状态：无状态行时写版本 1，否则仅在状态或动作子集变化时追加新版本。
/// 返回新写入的版本号；没有变化、已是终态或并发方已写入同版本都返回 `None`。
pub(crate) async fn refresh_right_state_in_tx(
    connection: &mut PgConnection,
    right: Uuid,
    now: Timestamp,
) -> Result<Option<u64>, WriteError> {
    let row = sqlx::query(
        "SELECT kind, contract_id, position_id FROM public.consignment_rights WHERE right_id = $1",
    )
    .bind(right)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or_else(|| WriteError::invariant("consignment_right_missing"))?;
    let kind = parse_kind(&row.try_get::<String, _>("kind")?)
        .ok_or_else(|| WriteError::invariant("consignment_right_kind"))?;
    let desired =
        desired_state(connection, kind, row.try_get("contract_id")?, row.try_get("position_id")?)
            .await?;
    let latest = sqlx::query(
        "SELECT version, state, allowed_actions FROM public.consignment_right_states
         WHERE right_id = $1 ORDER BY version DESC LIMIT 1",
    )
    .bind(right)
    .fetch_optional(&mut *connection)
    .await?;
    let next_version = match &latest {
        None => 1,
        Some(latest) => {
            if latest.try_get::<String, _>("state")? != "active" {
                return Ok(None);
            }
            let mut stored: Vec<String> = latest.try_get("allowed_actions")?;
            let mut wanted = desired.actions.clone();
            stored.sort();
            wanted.sort();
            if latest.try_get::<String, _>("state")? == desired.state && stored == wanted {
                return Ok(None);
            }
            latest.try_get::<i64, _>("version")? + 1
        }
    };
    let written = sqlx::query(
        "INSERT INTO public.consignment_right_states
            (right_id, version, state, allowed_actions, occurred_at)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT ON CONSTRAINT c7_right_state_pk DO NOTHING",
    )
    .bind(right)
    .bind(next_version)
    .bind(desired.state)
    .bind(&desired.actions)
    .bind(now)
    .execute(connection)
    .await?
    .rows_affected();
    Ok(if written == 1 { Some(u64::try_from(next_version)?) } else { None })
}

/// 登记 / 刷新原语的错误进入权利事务：并发中止可重试，其余只记固定分类、对外不可用。
fn tx_error(error: WriteError) -> TxError {
    if matches!(error, WriteError::RetryConflict) {
        return TxError::Conflict;
    }
    tracing::warn!(%error, "受限权利状态刷新失败");
    TxError::Store(RightsStoreError::Unavailable)
}

impl PgRightsStore {
    /// 按合同的最新订单事实刷新它的权利状态（买方 / 卖方各一项），返回新追加的版本条数。
    ///
    /// 订单阶段变化的写口在同事务里用 `refresh_right_state_in_tx` 刷新；这个入口供后台补刷与测试。
    /// 与续办共用 `right` 锁，所以刷新与续办互斥。
    pub async fn refresh_contract_rights(
        &self,
        contract_id: Uuid,
    ) -> Result<u64, RightsStoreError> {
        let roles = [RightRole::Buyer, RightRole::Seller];
        let locks =
            roles.iter().map(|role| lock_key("right", right_id(contract_id, *role).as_bytes()));
        self.transact(locks.collect(), &contract_id, |contract_id, connection, now| {
            Box::pin(async move {
                let mut appended = 0;
                for role in roles {
                    let right = right_id(*contract_id, role);
                    let exists: bool = sqlx::query_scalar(
                        "SELECT EXISTS (SELECT 1 FROM public.consignment_rights WHERE right_id = $1)",
                    )
                    .bind(right)
                    .fetch_one(&mut *connection)
                    .await?;
                    if !exists {
                        continue;
                    }
                    if refresh_right_state_in_tx(connection, right, now)
                        .await
                        .map_err(tx_error)?
                        .is_some()
                    {
                        appended += 1;
                    }
                }
                Ok(Ok(appended))
            })
        })
        .await
    }
}
