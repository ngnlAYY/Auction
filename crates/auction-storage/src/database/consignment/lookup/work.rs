//! 工作登记、撤销、围栏核验与观察落库。

use auction_application::consignment::{
    LookupFence, LookupLeaseUpdate, LookupRetryAfter, LookupTransientReason,
};
use auction_time::Timestamp;
use sqlx::PgConnection;
use uuid::Uuid;

use super::protocol::{LEASE_GRACE, MAX_LISTING_LOOKUP_ATTEMPTS, rate_limit_delay, retry_delay};

/// 在挂单登记事务中写入第一代工作。不变量 2、5：调用方已写入挂单并持有单例锁，
/// 使用登记事实的数据库时刻，首次工作立即可领。
#[allow(dead_code, reason = "由后续任务 2D 调用")]
pub(crate) async fn enqueue_lookup(
    conn: &mut PgConnection,
    listing_id: Uuid,
    work_id: Uuid,
    registered_at: Timestamp,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO public.consignment_lookup_work (
             id, listing_id, generation, state, attempt_count, fence, next_attempt_at,
             lease_owner, token, lease_until, network_deadline, last_reason,
             completed_at, created_at, updated_at
         ) VALUES ($1, $2, 1, 'queued', 0, 0, $3, NULL, NULL, NULL, NULL, NULL, NULL, $3, $3)",
    )
    .bind(work_id)
    .bind(listing_id)
    .bind(registered_at)
    .execute(conn)
    .await?;
    Ok(())
}

/// 同提交取消挂单的未完成工作，返回被撤销的行数。调用方先持挂单锁，再持单例锁。
/// 不变量 2、5、6：清除租约即废止提交围栏；可能在途时给原持有者保留时间栅栏。
#[allow(dead_code, reason = "由后续任务 2D 调用")]
pub(crate) async fn cancel_lookup(
    conn: &mut PgConnection,
    listing_id: Uuid,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "WITH db_clock AS MATERIALIZED (SELECT clock_timestamp() AS at),
         cancelled AS (
             UPDATE public.consignment_lookup_work AS w
             SET state = 'cancelled', next_attempt_at = NULL, lease_owner = NULL,
                 token = NULL, lease_until = NULL, network_deadline = NULL,
                 completed_at = at, updated_at = at
             FROM db_clock
             WHERE w.listing_id = $1 AND w.state IN ('queued', 'leased')
             RETURNING OLD.id AS work_id, OLD.fence, OLD.token, OLD.network_deadline
         ),
         holder AS (
             SELECT c.id, GREATEST(c.permit_until,
                                   w.network_deadline + make_interval(secs => $2)) AS until_at, at
             FROM public.consignment_lookup_control AS c CROSS JOIN db_clock
             JOIN cancelled AS w
               ON w.work_id = c.holder_work_id AND w.fence = c.holder_fence
                   AND w.token = c.holder_token
             WHERE c.id = 1
         ),
         updated_permit AS (
             UPDATE public.consignment_lookup_control AS c
             SET holder_work_id = CASE WHEN h.until_at > h.at THEN c.holder_work_id END,
                 holder_fence = CASE WHEN h.until_at > h.at THEN c.holder_fence END,
                 holder_token = CASE WHEN h.until_at > h.at THEN c.holder_token END,
                 permit_until = CASE WHEN h.until_at > h.at THEN h.until_at END
             FROM holder AS h WHERE c.id = h.id
         )
         SELECT count(*) FROM cancelled",
    )
    .bind(listing_id)
    .bind(LEASE_GRACE.as_secs_f64())
    .fetch_one(conn)
    .await
}

/// 活租约初次仲裁的结果。
#[allow(dead_code, reason = "由后续任务 2D 区分围栏结果")]
pub(crate) enum LookupFenceCheck {
    /// 工作与全局许可均匹配且未到期。
    Matched(MatchedLookupLease),
    /// 目标工作或代次不符，或工作并不属于命令所指挂单。
    MissingOrWrongGeneration,
    /// 工作存在，但提交围栏或全局许可已失效。
    LeaseLost,
}

/// 已在当前事务核验的活租约。不可跨事务使用，不记录内部 token。
pub(crate) struct MatchedLookupLease {
    fence: LookupFence,
    attempt_count: i16,
}

impl MatchedLookupLease {
    /// 本次领取已经持久化的计次。达到尝试上限时，调用方先终结挂单再完成工作。
    #[allow(dead_code, reason = "由后续任务 2D 选择解析结果的挂单迁移")]
    pub(crate) const fn attempt_count(&self) -> i16 {
        self.attempt_count
    }
}

/// 重放仲裁后，在挂单锁、单例锁之后锁工作，再读数据库墙钟。
/// 不变量 2、5、6：首次提交只承认活围栏；失败分支不写工作、许可或账本。
/// 准入代次及挂单资格由调用方在本事务内核验。
#[allow(dead_code, reason = "由后续任务 2D / 2F 调用")]
pub(crate) async fn check_lookup_fence(
    conn: &mut PgConnection,
    fence: LookupFence,
) -> Result<LookupFenceCheck, sqlx::Error> {
    check_lookup_fence_at(conn, fence, None).await
}

/// 精确时刻由 PostgreSQL 测试在夹具中指定；正常调用在获取工作锁后采样墙钟。
pub(super) async fn check_lookup_fence_at(
    conn: &mut PgConnection,
    fence: LookupFence,
    as_of: Option<Timestamp>,
) -> Result<LookupFenceCheck, sqlx::Error> {
    let locked: Option<(i64,)> = sqlx::query_as(
        "SELECT generation FROM public.consignment_lookup_work
         WHERE id = $1 AND listing_id = $2 AND generation = $3 FOR UPDATE",
    )
    .bind(fence.work_id())
    .bind(fence.listing_id())
    .bind(generation(fence)?)
    .fetch_optional(&mut *conn)
    .await?;
    if locked.is_none() {
        return Ok(LookupFenceCheck::MissingOrWrongGeneration);
    }

    let attempt: Option<i16> = sqlx::query_scalar(
        "WITH db_clock AS MATERIALIZED (SELECT COALESCE($4, clock_timestamp()) AS at)
         SELECT w.attempt_count
         FROM public.consignment_lookup_work AS w CROSS JOIN db_clock
         JOIN public.consignment_lookup_control AS c ON c.id = 1
         WHERE w.id = $1 AND w.state = 'leased' AND w.fence = $2 AND w.token = $3 AND w.lease_until > at
           AND c.holder_work_id = w.id AND c.holder_fence = $2 AND c.holder_token = $3
           AND c.permit_until > at",
    )
    .bind(fence.work_id())
    .bind(fence.fence())
    .bind(fence.token())
    .bind(as_of)
    .fetch_optional(conn)
    .await?;
    Ok(match attempt {
        Some(attempt_count) => {
            LookupFenceCheck::Matched(MatchedLookupLease { fence, attempt_count })
        }
        None => LookupFenceCheck::LeaseLost,
    })
}

/// 活租约已得到的本次观察结论。
#[allow(dead_code, reason = "由后续任务 2D 提交观察")]
pub(crate) enum LookupResolution {
    /// 已完成解析，不再安排下次尝试。
    Done,
    /// 暂时失败。第 1–5 次重排，第六次终结；429 同时写全局冷却。
    Transient {
        /// 可持久化的安全原因。
        reason: LookupTransientReason,
        /// 429 的已解析提示，无效或缺失时为空。其他原因不应用提示。
        retry_after: Option<LookupRetryAfter>,
    },
}

/// 在原挂单、单例、工作锁下提交观察。调用方先应用本次挂单事实；第六次暂时失败
/// 须先终结挂单。SQL CAS 再核墙钟，若返回 Lost 则调用方须回滚整项命令。
/// 不变量 2、3、5、6：重排、完成、耗尽与释放原许可原子写入；旧租约不写任何一行。
/// Applied(Some) 为下次解析时刻，Applied(None) 为完成或耗尽。
#[allow(dead_code, reason = "由后续任务 2D 调用")]
pub(crate) async fn finish_lookup(
    conn: &mut PgConnection,
    lease: MatchedLookupLease,
    resolution: LookupResolution,
) -> Result<LookupLeaseUpdate<Option<Timestamp>>, sqlx::Error> {
    let (retry, reason, cooldown) = match resolution {
        LookupResolution::Done => (None, None, None),
        LookupResolution::Transient { reason, retry_after } => {
            let retry = retry_delay(lease.attempt_count);
            if retry.is_none() && lease.attempt_count != i16::from(MAX_LISTING_LOOKUP_ATTEMPTS) {
                return Err(sqlx::Error::Protocol("持久解析计次不在协议范围内".to_owned()));
            }
            let cooldown = (reason == LookupTransientReason::SourceRateLimited)
                .then(|| rate_limit_delay(retry, retry_after).as_secs_f64());
            (retry.map(|delay| delay.as_secs_f64()), Some(reason.as_str()), cooldown)
        }
    };
    let fence = lease.fence;
    let changed: Option<Option<Timestamp>> = sqlx::query_scalar(
        "WITH db_clock AS MATERIALIZED (SELECT clock_timestamp() AS at),
         active AS (
             SELECT at, at + make_interval(secs => $6) AS retry_at,
                        at + make_interval(secs => $8) AS cooldown_at
             FROM public.consignment_lookup_control CROSS JOIN db_clock
             WHERE id = 1 AND holder_work_id = $1 AND holder_fence = $4 AND holder_token = $5
               AND permit_until > at
         ),
         changed_work AS (
             UPDATE public.consignment_lookup_work AS w
             SET state = CASE WHEN active.retry_at IS NULL THEN 'done' ELSE 'queued' END,
                 next_attempt_at = active.retry_at,
                 lease_owner = NULL, token = NULL, lease_until = NULL, network_deadline = NULL,
                 last_reason = COALESCE($7, w.last_reason),
                 completed_at = CASE WHEN active.retry_at IS NULL THEN active.at END,
                 updated_at = active.at
             FROM active
             WHERE w.id = $1 AND w.listing_id = $2 AND w.generation = $3 AND w.state = 'leased'
               AND w.fence = $4 AND w.token = $5 AND w.lease_until > active.at
             RETURNING w.next_attempt_at, active.cooldown_at
         )
         UPDATE public.consignment_lookup_control AS c
         SET holder_work_id = NULL, holder_fence = NULL, holder_token = NULL, permit_until = NULL,
             cooldown_until = GREATEST(c.cooldown_until, w.cooldown_at)
         FROM changed_work AS w WHERE c.id = 1
         RETURNING w.next_attempt_at",
    )
    .bind(fence.work_id())
    .bind(fence.listing_id())
    .bind(generation(fence)?)
    .bind(fence.fence())
    .bind(fence.token())
    .bind(retry)
    .bind(reason)
    .bind(cooldown)
    .fetch_optional(conn)
    .await?;
    Ok(match changed {
        Some(next_attempt_at) => LookupLeaseUpdate::Applied(next_attempt_at),
        None => LookupLeaseUpdate::Lost,
    })
}

fn generation(fence: LookupFence) -> Result<i64, sqlx::Error> {
    i64::try_from(fence.generation())
        .map_err(|error| sqlx::Error::Protocol(format!("解析代次超出存储范围：{error}")))
}
