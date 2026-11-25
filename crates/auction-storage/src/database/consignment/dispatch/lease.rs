//! 寄售轮投递与履约行的衔接及投递租约：executor 领取寄售义务后经此取得起写所需的 fence。
//!
//! 保护不变量 2、5、6：锁序与起写相同（挂单 → position → 受益用户 → 投递 → 履约行 → 判定，照 D9
//! 全局顺序），受益人保留权利时不衔接、不发租约。同一轮的衔接在轮 ID 命令锁与投递、履约行锁下串行；履约 claim 必须仍是
//! 调用方持有且未过期的那一个，轮必须属于该义务、投递必须是该轮的投递，否则失败关闭。履约行只从
//! 初始 `pending`（不指向投递）或已指向本轮投递的状态推进，绝不改指另一封投递；投递已有未过期
//! 租约时不抢占（并发领取者只得到 `Busy`），已起写或已终结的投递不再发出租约。租约与 fence
//! 持久在投递行，起写仍只经 `mark_write_started`；本操作不产生任何外部写入许可。

use auction_application::consignment::ConsignmentError;
use auction_domain::legacy_natural_key_uuid;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::attempt::authorize_round_recipient;
use super::lock_order::lock_listing_and_position;
use super::round::load_by_round_id;
use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::session::{LockKeys, with_command_transaction};
use crate::database::consignment::deposit::primitives::database_now;
use crate::database::consignment::reads::orders::record_order_stage_change_in_tx;

/// 激活命令 ID 的命名空间；由轮 ID 确定性派生，重放得到同一值。**不要改名**。
const ACTIVATE_NAMESPACE: &str = "consignment.dispatch-activate.v1";

/// 一次衔接请求：调用方持有的寄售义务领取（履约行 claim）与要领取的轮。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoundLeaseRequest {
    /// 被派发的寄售义务。
    pub obligation_id: Uuid,
    /// 寄售义务领取写在履约行上的 token。
    pub claim_token: Uuid,
    /// 寄售义务领取写在履约行上的 fence。
    pub claim_fencing: i64,
    /// 要领取的派发轮。
    pub round_id: Uuid,
}

/// 衔接结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundLease {
    /// 取得投递租约：以这三个值登记尝试并提交起写。
    Leased {
        /// 轮的投递。
        delivery_id: Uuid,
        /// 投递租约 token（`StartAttempt` / `MarkWriteStarted` 的 claim token）。
        claim_token: Uuid,
        /// 投递 fence（尝试键的 fence）。
        fence: u64,
    },
    /// 投递已有未过期的租约（并发领取者）：本次不领取，过租后再试。
    Busy,
    /// 投递已起写或已终结：不再发租约，结果由证据与人工核对收口。
    Settled,
    /// 履约 claim 失效、轮不属于该义务、投递不符或履约已指向别的投递：失败关闭，不重试。
    Rejected,
}

impl DatabaseStore {
    /// 把寄售义务的履约行衔接到轮投递并领取该投递（W-5 executor 的存储侧）。
    ///
    /// 一个顶层 Serializable 事务；序列化冲突有界重试。命令锁键取轮 ID，使同一轮的并发领取者
    /// 先在会话锁上排队。
    pub async fn lease_dispatch_round(
        &self,
        request: &RoundLeaseRequest,
    ) -> Result<RoundLease, ConsignmentError> {
        let keys = LockKeys { command_id: request.round_id, sellers: &[], equipment: &[] };
        with_command_transaction(self, keys, request, |request, tx| {
            Box::pin(lease_in_tx(request, tx.connection()))
        })
        .await
        .map_err(WriteError::into_application)
    }
}

/// 锁下读到的履约行。
struct Fulfillment {
    state: String,
    delivery_id: Option<Uuid>,
    owned: bool,
}

/// 衔接写入的投递与激活命令。
struct Link {
    delivery_id: Uuid,
    activate_command_id: Uuid,
}

/// 锁下读到的投递行。
struct Delivery {
    status: String,
    command_id: Uuid,
    request_digest: Vec<u8>,
    lease_live: bool,
}

async fn lease_in_tx(
    request: &RoundLeaseRequest,
    connection: &mut PgConnection,
) -> Result<RoundLease, WriteError> {
    let Some(round) = load_by_round_id(&mut *connection, request.round_id).await? else {
        return Ok(RoundLease::Rejected);
    };
    if round.key.obligation_id != request.obligation_id {
        return Ok(RoundLease::Rejected);
    }
    // 锁序与起写一致：挂单 → position → 受益用户 → 投递 → 履约行 → 判定；之后在已持有的行锁下
    // 重读投递与履约行的状态。
    if !lock_listing_and_position(&mut *connection, round.key.contract_id).await?
        || authorize_round_recipient(&mut *connection, request.obligation_id, round.delivery_id)
            .await?
            .is_some()
    {
        return Ok(RoundLease::Rejected);
    }
    let Some(delivery) = lock_delivery(&mut *connection, round.delivery_id, request).await? else {
        return Ok(RoundLease::Rejected);
    };
    let Some(fulfillment) = lock_fulfillment(&mut *connection, request).await? else {
        return Ok(RoundLease::Rejected);
    };
    let linked_here = fulfillment.delivery_id == Some(round.delivery_id);
    if !fulfillment.owned
        || !(linked_here || fulfillment.delivery_id.is_none() && fulfillment.state == "pending")
    {
        return Ok(RoundLease::Rejected);
    }
    match delivery.status.as_str() {
        "prepared" | "pending" => {}
        "claimed" if delivery.lease_live => return Ok(RoundLease::Busy),
        "claimed" => {}
        _ => return Ok(RoundLease::Settled),
    }
    let activate_command_id =
        legacy_natural_key_uuid(ACTIVATE_NAMESPACE, &[request.round_id.as_bytes()]);
    let link = Link { delivery_id: round.delivery_id, activate_command_id };
    let advanced =
        link_fulfillment(&mut *connection, request, &fulfillment, &delivery, &link).await?;
    if delivery.status == "prepared" {
        activate_delivery(&mut *connection, round.delivery_id, &delivery, activate_command_id)
            .await?;
    }
    let lease = claim_delivery(&mut *connection, round.delivery_id).await?;
    if advanced {
        // 履约行推进与订单阶段事件、权利刷新同事务（不变量 3）；只续租不追加。
        let now = database_now(&mut *connection).await?;
        record_order_stage_change_in_tx(&mut *connection, round.key.contract_id, now).await?;
    }
    Ok(lease)
}

async fn lock_fulfillment(
    connection: &mut PgConnection,
    request: &RoundLeaseRequest,
) -> Result<Option<Fulfillment>, WriteError> {
    let row = sqlx::query(
        "SELECT fulfillment.execution_state, fulfillment.delivery_id,
                (fulfillment.claim_token = $2 AND fulfillment.claim_fencing = $3
                 AND fulfillment.claim_expires_at > clock_timestamp()) IS TRUE AS owned
         FROM public.settlement_obligation_fulfillments AS fulfillment
         JOIN public.settlement_obligations AS obligation
           ON obligation.id = fulfillment.obligation_id
         WHERE fulfillment.obligation_id = $1 AND obligation.contract_id IS NOT NULL
           AND obligation.business_state = 'ready'
         FOR UPDATE OF fulfillment",
    )
    .bind(request.obligation_id)
    .bind(request.claim_token)
    .bind(request.claim_fencing)
    .fetch_optional(connection)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(Fulfillment {
        state: row.try_get("execution_state")?,
        delivery_id: row.try_get("delivery_id")?,
        owned: row.try_get("owned")?,
    }))
}

async fn lock_delivery(
    connection: &mut PgConnection,
    delivery_id: Uuid,
    request: &RoundLeaseRequest,
) -> Result<Option<Delivery>, WriteError> {
    let row = sqlx::query(
        "SELECT status, command_id, request_digest,
                COALESCE(lease_expires_at > clock_timestamp(), false) AS lease_live
         FROM public.deliveries
         WHERE id = $1 AND origin_kind = 'settlement' AND origin_id = $2
         FOR UPDATE",
    )
    .bind(delivery_id)
    .bind(request.obligation_id.to_string())
    .fetch_optional(connection)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(Delivery {
        status: row.try_get("status")?,
        command_id: row.try_get("command_id")?,
        request_digest: row.try_get("request_digest")?,
        lease_live: row.try_get("lease_live")?,
    }))
}

/// 履约行 `pending → prepared → enqueued`，指向轮投递；已指向本轮投递时只补齐未完成的一步。
/// 返回是否推进了状态。准备命令与摘要取投递行自身（首轮的确定性命令 ID 与冻结身份摘要）。
async fn link_fulfillment(
    connection: &mut PgConnection,
    request: &RoundLeaseRequest,
    fulfillment: &Fulfillment,
    delivery: &Delivery,
    link: &Link,
) -> Result<bool, WriteError> {
    let mut advanced = false;
    if fulfillment.state == "pending" {
        let prepared = sqlx::query(
            "UPDATE public.settlement_obligation_fulfillments
             SET execution_state = 'prepared', delivery_id = $2, prepare_command_id = $3,
                 prepare_request_digest = $4, state_version = state_version + 1,
                 next_attempt_at = NULL, last_error_code = NULL,
                 updated_at = statement_timestamp()
             WHERE obligation_id = $1 AND execution_state = 'pending' AND delivery_id IS NULL
               AND claim_token = $5 AND claim_fencing = $6",
        )
        .bind(request.obligation_id)
        .bind(link.delivery_id)
        .bind(delivery.command_id)
        .bind(delivery.request_digest.as_slice())
        .bind(request.claim_token)
        .bind(request.claim_fencing)
        .execute(&mut *connection)
        .await?;
        if prepared.rows_affected() != 1 {
            return Err(WriteError::invariant("consignment_dispatch_link_prepare"));
        }
        advanced = true;
    }
    if fulfillment.state == "pending" || fulfillment.state == "prepared" {
        let enqueued = sqlx::query(
            "UPDATE public.settlement_obligation_fulfillments
             SET execution_state = 'enqueued', activate_command_id = $2,
                 activate_request_digest = $3, state_version = state_version + 1,
                 next_attempt_at = NULL, last_error_code = NULL,
                 updated_at = statement_timestamp()
             WHERE obligation_id = $1 AND execution_state = 'prepared'
               AND claim_token = $4 AND claim_fencing = $5",
        )
        .bind(request.obligation_id)
        .bind(link.activate_command_id)
        .bind(delivery.request_digest.as_slice())
        .bind(request.claim_token)
        .bind(request.claim_fencing)
        .execute(&mut *connection)
        .await?;
        if enqueued.rows_affected() != 1 {
            return Err(WriteError::invariant("consignment_dispatch_link_enqueue"));
        }
        advanced = true;
    }
    Ok(advanced)
}

/// 投递 `prepared → pending`：激活命令与摘要和履约行的激活一致。
async fn activate_delivery(
    connection: &mut PgConnection,
    delivery_id: Uuid,
    delivery: &Delivery,
    activate_command_id: Uuid,
) -> Result<(), WriteError> {
    let activated = sqlx::query(
        "UPDATE public.deliveries
         SET status = 'pending', activation_command_id = $2, activation_request_digest = $3,
             activated_at = statement_timestamp(), updated_at = statement_timestamp()
         WHERE id = $1 AND status = 'prepared'",
    )
    .bind(delivery_id)
    .bind(activate_command_id)
    .bind(delivery.request_digest.as_slice())
    .execute(connection)
    .await?;
    if activated.rows_affected() != 1 {
        return Err(WriteError::invariant("consignment_dispatch_activate"));
    }
    Ok(())
}

/// 投递转 `claimed`：新租约 token、fence 加一；租约到期与履约 claim 同时刻，过租后旧持有者的
/// 起写因 fence 不符被拒。
async fn claim_delivery(
    connection: &mut PgConnection,
    delivery_id: Uuid,
) -> Result<RoundLease, WriteError> {
    let lease_token = Uuid::new_v4();
    let fence: Option<i64> = sqlx::query_scalar(
        "UPDATE public.deliveries AS delivery
         SET status = 'claimed', lease_token = $2,
             lease_expires_at = fulfillment.claim_expires_at,
             fencing_version = delivery.fencing_version + 1,
             attempt_count = delivery.attempt_count + 1,
             claimed_at = COALESCE(delivery.claimed_at, statement_timestamp()),
             updated_at = statement_timestamp()
         FROM public.settlement_obligation_fulfillments AS fulfillment
         WHERE delivery.id = $1 AND fulfillment.delivery_id = delivery.id
           AND delivery.status IN ('pending', 'claimed')
         RETURNING delivery.fencing_version",
    )
    .bind(delivery_id)
    .bind(lease_token)
    .fetch_optional(connection)
    .await?;
    let fence = fence.ok_or_else(|| WriteError::invariant("consignment_dispatch_lease"))?;
    Ok(RoundLease::Leased { delivery_id, claim_token: lease_token, fence: u64::try_from(fence)? })
}
