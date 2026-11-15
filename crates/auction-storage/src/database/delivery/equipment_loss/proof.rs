//! 装备损失处置的只读证明；只追加事实必须与当前退休投递、人工证明及资金来源同时匹配。

use std::sync::LazyLock;

use auction_application::settlement::FundingError;
use sqlx::Postgres;
use uuid::Uuid;

use crate::database::{TrackedTransaction as Transaction, settlement::funding};

/// `deliveries` 整行快照比较，由 `delivery::row_snapshot` 的 `delivery_row_snapshot_equality!`
/// 展开（时区归一化、`?&` 存在性守卫与派生列剔除见其说明）；本证明另行断言并剔除
/// `failure_code` 与 `completed_at`，退休投递允许这两列与快照不同。
const DELIVERY_SNAPSHOT_EQUALITY: &str = concat!(
    "\n           AND ",
    crate::database::delivery::row_snapshot::delivery_row_snapshot_equality!(
        "x",
        "-'failure_code'-'completed_at'"
    ),
    "\n"
);

pub(crate) async fn trusted_losses(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
) -> Result<Vec<(Uuid, Uuid)>, FundingError> {
    // 只缓存固定 SQL 文本，不缓存任何 UID 的证明结果；拼接值仅为已审查的编译期常量。
    static QUERY: LazyLock<String> = LazyLock::new(|| {
        "SELECT x.lot_id,x.delivery_id FROM public.settlement_equipment_losses x
         JOIN public.deliveries d ON d.id=x.delivery_id
         JOIN public.settlement_obligation_fulfillments pf ON pf.obligation_id=x.payment_obligation_id
         JOIN public.settlement_obligation_fulfillments bf ON bf.obligation_id=x.refund_obligation_id
         JOIN public.settlement_recoveries r ON r.id=x.recovery_id
         JOIN public.settlement_funding_lots fl ON fl.lot_id=x.lot_id
         WHERE x.uid=$1 AND fl.uid=x.uid AND fl.payment_obligation_id=x.payment_obligation_id
           AND fl.source_kind='operator_equipment_loss' AND fl.disposition IN ('seller','retained')
           AND d.status='failed' AND d.failure_code='equipment_destroyed'
           AND d.fencing_version=x.expected_delivery_fencing_version+1 AND d.external_reference IS NULL
           AND d.result_metadata=(x.original_snapshot#>'{delivery,result_metadata}')
             ||jsonb_build_object('equipment_loss_command_id',x.command_id::text,'retired',true)
           /* delivery_row_equality */
           AND pf.delivery_id=x.delivery_id AND pf.execution_state='satisfied'
           AND pf.proof_kind='operator_equipment_loss_v1' AND pf.proof_digest=x.request_digest
           AND pf.proof_reference='operator_equipment_loss_v1:'||x.command_id::text
           AND bf.delivery_id IS NULL AND bf.execution_state='failed'
           AND bf.last_error_code='operator_external_buyer_compensation'
           AND bf.proof_kind=pf.proof_kind AND bf.proof_reference=pf.proof_reference AND bf.proof_digest=x.request_digest
           AND r.stage='completed' AND r.resolution IS NULL AND r.relisted_lot_id IS NULL
         ORDER BY x.lot_id"
            .replace("/* delivery_row_equality */", DELIVERY_SNAPSHOT_EQUALITY)
    });
    sqlx::query_as(sqlx::AssertSqlSafe(QUERY.as_str()))
        .bind(uid)
        .fetch_all(&mut **tx)
        .await
        .map_err(funding::storage)
}

/// 集成测试入口：只返回静态 SQL 文本，别名固定为 `d`、`x`，不暴露任何证明结果或凭据。
#[cfg(feature = "test-db")]
pub fn delivery_snapshot_equality_sql() -> &'static str {
    DELIVERY_SNAPSHOT_EQUALITY
}
