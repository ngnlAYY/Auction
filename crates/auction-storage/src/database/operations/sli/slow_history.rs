//! 慢速代理历史执行滞后：只读审计的 due_at → occurred_at 窗口。

use auction_application::sli::SliSnapshotError;
use sqlx::PgConnection;

use super::{
    PERCENTILE_COLUMNS, Window, WorldWindows, ensure_complete, ensure_valid, flow_window,
    world_index,
};

/// 窗口内的慢速代理执行审计；元数据与实体标识先按格式校验再转换，格式不符记为 NULL。
macro_rules! candidates {
    () => {
        "
    WITH candidates AS MATERIALIZED (
        SELECT audit.id,audit.occurred_at,audit.entity_id,audit.metadata,
               CASE WHEN audit.metadata->>'due_at' ~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}T'
                    THEN (audit.metadata->>'due_at')::timestamptz END AS due_at,
               CASE WHEN audit.entity_id ~
                         '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
                    THEN audit.entity_id::uuid END AS lot_id
        FROM public.audit_records audit
        WHERE audit.action='lot.slow_proxy_tick'
          AND audit.occurred_at >= $1 AND audit.occurred_at <= $2
        ORDER BY audit.occurred_at,audit.id
        LIMIT $3
    )"
    };
}

const SLOW_COUNT: &str = concat!(
    candidates!(),
    "
    SELECT count(*) AS candidates,
           count(*) FILTER (WHERE due_at IS NULL OR lot_id IS NULL) AS invalid
    FROM candidates",
);

const SLOW_AGGREGATE: &str = concat!(
    candidates!(),
    "
    SELECT auction.world_id,count(*) AS samples,
           percentile_cont(0.5) WITHIN GROUP (
               ORDER BY extract(epoch FROM (candidate.occurred_at-candidate.due_at))
           ) AS p50,
           percentile_cont(0.95) WITHIN GROUP (
               ORDER BY extract(epoch FROM (candidate.occurred_at-candidate.due_at))
           ) AS p95,
           percentile_cont(0.99) WITHIN GROUP (
               ORDER BY extract(epoch FROM (candidate.occurred_at-candidate.due_at))
           ) AS p99
    FROM candidates candidate
    JOIN public.lots lot ON lot.id=candidate.lot_id
    JOIN public.auctions auction ON auction.id=lot.auction_id
    WHERE candidate.due_at IS NOT NULL
    GROUP BY auction.world_id",
);

pub(super) async fn read(
    connection: &mut PgConnection,
    window: Window,
) -> Result<WorldWindows, SliSnapshotError> {
    let totals = window.one(connection, SLOW_COUNT).await?;
    ensure_complete(&totals, &["candidates"])?;
    ensure_valid(&totals)?;
    let mut windows = WorldWindows::default();
    for row in window.all(connection, SLOW_AGGREGATE).await? {
        windows[world_index(&row)?] = flow_window(&row, PERCENTILE_COLUMNS, 0)?;
    }
    Ok(windows)
}
