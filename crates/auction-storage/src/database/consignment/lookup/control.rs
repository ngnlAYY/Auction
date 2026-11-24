//! 全局解析许可单例的加锁与准入投影。

use auction_application::consignment::LookupPermitStatus;
use auction_time::Timestamp;
use sqlx::PgConnection;
use uuid::Uuid;

/// 许可单例的持久状态，内部凭证不实现 Debug。
#[allow(dead_code, reason = "由后续任务 2D / 2F 读取单例字段")]
pub(crate) struct LookupControl {
    /// 准入代次。
    pub(crate) enabled_epoch: i64,
    /// 持久准入开关。
    pub(crate) admitting: bool,
    /// 下次请求起始时刻的全局下界。
    pub(crate) next_allowed_at: Timestamp,
    /// 上游 429 留下的全局冷却截止。
    pub(crate) cooldown_until: Option<Timestamp>,
    /// 许可或在途时间围栏关联的工作。
    pub(crate) holder_work_id: Option<Uuid>,
    /// 许可持有者的工作围栏。
    pub(crate) holder_fence: Option<i64>,
    /// 许可持有者的内部 token。
    pub(crate) holder_token: Option<Uuid>,
    /// 许可或在途时间围栏的截止时刻。
    pub(crate) permit_until: Option<Timestamp>,
    /// 本代 Worker 就绪的截止时刻。
    pub(crate) worker_ready_until: Option<Timestamp>,
    /// 维护循环下次补扫的位置。
    pub(crate) scan_after: Option<Uuid>,
}

/// 在调用方事务中锁单例。先锁资格与挂单（若路径需要），再锁单例，最后锁工作。
/// 不变量 2、5：领取从这里开始，持单例期间不能反向取得挂单写锁。
#[allow(dead_code, reason = "由后续任务 2D / 2F 调用")]
pub(crate) async fn lock_control(conn: &mut PgConnection) -> Result<LookupControl, sqlx::Error> {
    let (
        enabled_epoch,
        admitting,
        next_allowed_at,
        cooldown_until,
        holder_work_id,
        holder_fence,
        holder_token,
        permit_until,
        worker_ready_until,
        scan_after,
    ) = sqlx::query_as(
        "SELECT enabled_epoch, admitting, next_allowed_at, cooldown_until,
                holder_work_id, holder_fence, holder_token, permit_until, worker_ready_until, scan_after
         FROM public.consignment_lookup_control WHERE id = 1 FOR UPDATE",
    )
    .fetch_one(conn)
    .await?;
    Ok(LookupControl {
        enabled_epoch,
        admitting,
        next_allowed_at,
        cooldown_until,
        holder_work_id,
        holder_fence,
        holder_token,
        permit_until,
        worker_ready_until,
        scan_after,
    })
}

/// 准入、许可调度状态与当前代次的 Worker 心跳是否新鲜。不包含持有者凭证。
#[allow(dead_code, reason = "由后续任务 2D / 2F 读取准入及调度投影")]
pub(crate) struct LookupAdmission {
    /// 许可的只读投影及本次数据库时刻。
    pub(crate) permit: LookupPermitStatus,
    /// 已开启且 Worker 心跳严格晚于本次数据库时刻。
    pub(crate) worker_ready: bool,
}

/// 在调用方的读快照中读取准入和心跳，不取行锁。登记应在锁住单例后调用，
/// 以便按最新数据库墙钟核新鲜度，并让准入判定与入队在同一事务内串行化。
#[allow(dead_code, reason = "由后续任务 2D / 2F 调用")]
pub(crate) async fn read_admission(
    conn: &mut PgConnection,
) -> Result<LookupAdmission, sqlx::Error> {
    let (
        as_of,
        enabled_epoch,
        admitting,
        next_allowed_at,
        cooldown_until,
        permit_until,
        worker_ready,
    ): (Timestamp, i64, bool, Timestamp, Option<Timestamp>, Option<Timestamp>, bool) =
        sqlx::query_as(
            "WITH db_clock AS MATERIALIZED (SELECT clock_timestamp() AS at)
         SELECT at, enabled_epoch, admitting, next_allowed_at, cooldown_until, permit_until,
                admitting AND COALESCE(worker_ready_until > at, false)
         FROM public.consignment_lookup_control CROSS JOIN db_clock WHERE id = 1",
        )
        .fetch_one(conn)
        .await?;
    Ok(LookupAdmission {
        permit: LookupPermitStatus {
            as_of,
            service_epoch: u64::try_from(enabled_epoch)
                .map_err(|error| sqlx::Error::Protocol(format!("准入代次超出存储范围：{error}")))?,
            admitting,
            next_allowed_at,
            cooldown_until,
            permit_until,
        },
        worker_ready,
    })
}
