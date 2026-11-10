//! 入站邮件到成功出站证明的历史窗口：注册 key、投稿 Take、拒收通知。
//! Send 复用 delivery_has_frozen_sent_proof；Take 复用 conflicts/intake 的 owner 谓词。
//! 证明缺失或不符合谓词的已成功投递计入 unknown，不进入分位。

use auction_application::sli::SliSnapshotError;
use sqlx::PgConnection;

use super::{
    PERCENTILE_COLUMNS, Window, WorldWindows, count, ensure_complete, ensure_valid, flow_window,
    world_index,
};

/// 一个流程的三条语句：窗口完整性与关联校验、证明缺失计数、已证明样本分位。
struct FlowSql {
    count: &'static str,
    unknown: &'static str,
    aggregate: &'static str,
}

/// 三条语句共用窗口与候选 CTE，只在尾部的关联方式和证明谓词上不同。
macro_rules! flow_sql {
    (candidates: $candidates:expr, proven: $proven:expr, unproven: $unproven:expr $(,)?) => {
        FlowSql {
            count: concat!(
                window_deliveries!(),
                $candidates,
                "
    SELECT (SELECT count(*) FROM window_deliveries) AS window_count,
           count(*) AS candidates,
           count(*) FILTER (WHERE inbound.id IS NULL) AS invalid
    FROM candidates candidate
    LEFT JOIN public.inbound_mail inbound
      ON inbound.world_id=candidate.payload_world
     AND inbound.external_mail_id=candidate.payload->>'external_mail_id'"
            ),
            unknown: concat!(
                window_deliveries!(),
                $candidates,
                "
    SELECT inbound.world_id,count(*) AS unknown
    FROM candidates candidate
    JOIN public.inbound_mail inbound
      ON inbound.world_id=candidate.payload_world
     AND inbound.external_mail_id=candidate.payload->>'external_mail_id'
    WHERE ",
                $unproven,
                "
    GROUP BY inbound.world_id"
            ),
            aggregate: concat!(
                window_deliveries!(),
                $candidates,
                "
    SELECT inbound.world_id,count(*) AS samples,
           percentile_cont(0.5) WITHIN GROUP (
               ORDER BY extract(epoch FROM (candidate.completed_at-inbound.received_at))
           ) AS p50,
           percentile_cont(0.95) WITHIN GROUP (
               ORDER BY extract(epoch FROM (candidate.completed_at-inbound.received_at))
           ) AS p95,
           percentile_cont(0.99) WITHIN GROUP (
               ORDER BY extract(epoch FROM (candidate.completed_at-inbound.received_at))
           ) AS p99
    FROM candidates candidate
    JOIN public.inbound_mail inbound
      ON inbound.world_id=candidate.payload_world
     AND inbound.external_mail_id=candidate.payload->>'external_mail_id'
    WHERE ",
                $proven,
                "
    GROUP BY inbound.world_id"
            ),
        }
    };
}

/// 窗口内全部成功投递；LIMIT 先于来源筛选，哨兵行用于证明窗口完整。
macro_rules! window_deliveries {
    () => {
        "
    WITH window_deliveries AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,delivery.payload,delivery.payload_world,
               delivery.payload_operation,delivery.payload_template,delivery.origin_kind,
               delivery.status,delivery.failure_code,delivery.lease_token,
               delivery.lease_expires_at,delivery.sending_at,
               delivery.result_metadata,delivery.external_reference
        FROM public.deliveries delivery
        WHERE delivery.status='succeeded'
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
        ORDER BY delivery.completed_at,delivery.id
        LIMIT $3
    ),"
    };
}

/// 邮件入库产生的 Send 候选；`$template` 是模板谓词。
macro_rules! send_candidates {
    ($template:literal) => {
        concat!(
            "
    candidates AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,delivery.payload,delivery.payload_world,
               delivery.payload_operation,delivery.payload_template,delivery.origin_kind,
               delivery.result_metadata,delivery.external_reference
        FROM window_deliveries delivery
        WHERE delivery.status='succeeded' AND delivery.origin_kind='mail_intake'
          AND delivery.payload_operation='send'
          AND ",
            $template,
            "
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
        ORDER BY delivery.completed_at,delivery.id
        LIMIT $3
    )"
        )
    };
}

/// Send 的成功证明：冻结预期与 Sent 绑定一致。
macro_rules! sent_proof {
    () => {
        "public.delivery_has_frozen_sent_proof(
        candidate.id,candidate.result_metadata,candidate.external_reference
    )"
    };
}

const REGISTER_KEY: FlowSql = flow_sql!(
    candidates: send_candidates!("delivery.payload_template='register_key'"),
    proven: sent_proof!(),
    unproven: concat!("NOT ", sent_proof!()),
);

const SUBMISSION_REJECTION: FlowSql = flow_sql!(
    candidates: send_candidates!(
        "delivery.payload_template IN
              ('auction_equipment_rejected','auction_no_attachments','auction_assets_rejected')"
    ),
    proven: sent_proof!(),
    unproven: concat!("NOT ", sent_proof!()),
);

// Take 没有 Sent 证明；`proof_ok` 在候选 CTE 内按 Take 结果元数据判定，NULL 视为未证明。
const SUBMISSION_TAKE: FlowSql = flow_sql!(
    candidates: "
    candidates AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,delivery.payload,delivery.payload_world,
               delivery.payload_operation,delivery.payload_template,
               (delivery.failure_code IS NULL AND delivery.lease_token IS NULL
                AND delivery.lease_expires_at IS NULL AND delivery.sending_at IS NOT NULL
                AND delivery.completed_at >= delivery.sending_at
                AND delivery.external_reference =
                    'mooglemail:'||(delivery.payload_world)||':take:'
                    ||(delivery.payload->>'external_mail_id')
                AND (
                    (delivery.result_metadata->>'proof'='action_removed_or_success_page'
                     AND delivery.result_metadata->>'operation'='take'
                     AND delivery.result_metadata->>'external_mail_id'=
                         delivery.payload->>'external_mail_id'
                     AND COALESCE(delivery.result_metadata->>'outcome_unknown','false')='false'
                     AND delivery.result_metadata->>'expected_credits'='0'
                     AND delivery.result_metadata->>'expected_cod'='0')
                    OR
                    (delivery.result_metadata->>'proof'='action_recovered_inbox_v1'
                     AND delivery.result_metadata->>'external_reference_proven'='true'
                     AND delivery.result_metadata->>'recovered_mid'=
                         delivery.payload->>'external_mail_id'
                     AND delivery.result_metadata->>'reason' IN
                         ('inbox_mail_absent','attachments_absent'))
                )) AS proof_ok
        FROM window_deliveries delivery
        WHERE delivery.status='succeeded' AND delivery.origin_kind='mail_intake'
          AND delivery.payload_operation='take'
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
        ORDER BY delivery.completed_at,delivery.id
        LIMIT $3
    )",
    proven: "candidate.proof_ok",
    unproven: "candidate.proof_ok IS NOT TRUE",
);

async fn read_flow(
    connection: &mut PgConnection,
    window: Window,
    sql: &FlowSql,
) -> Result<WorldWindows, SliSnapshotError> {
    let totals = window.one(connection, sql.count).await?;
    ensure_complete(&totals, &["window_count", "candidates"])?;
    ensure_valid(&totals)?;
    let mut windows = WorldWindows::default();
    for row in window.all(connection, sql.unknown).await? {
        windows[world_index(&row)?].unknown = count(&row, "unknown")?;
    }
    for row in window.all(connection, sql.aggregate).await? {
        let index = world_index(&row)?;
        windows[index] = flow_window(&row, PERCENTILE_COLUMNS, windows[index].unknown)?;
    }
    Ok(windows)
}

pub(super) async fn register_key(
    connection: &mut PgConnection,
    window: Window,
) -> Result<WorldWindows, SliSnapshotError> {
    read_flow(connection, window, &REGISTER_KEY).await
}

pub(super) async fn submission_take(
    connection: &mut PgConnection,
    window: Window,
) -> Result<WorldWindows, SliSnapshotError> {
    read_flow(connection, window, &SUBMISSION_TAKE).await
}

pub(super) async fn submission_rejection(
    connection: &mut PgConnection,
    window: Window,
) -> Result<WorldWindows, SliSnapshotError> {
    read_flow(connection, window, &SUBMISSION_REJECTION).await
}
