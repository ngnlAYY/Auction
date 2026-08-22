//! PostgreSQL Admin recovery 报告读取适配器。
//!
//! 只读报告在一个 Serializable 快照内读取 recovery case、action 和 lot/equipment 展示字段。
//! claim token、lease、request digest、外部 payload 等内部事实不会跨过应用端口。

use auction_application::{
    ApplicationError, ApplicationFuture, RecoveryReport, RecoveryReportReader,
};
use auction_domain::AuctionId;

#[cfg(feature = "postgres")]
use auction_application::{RecoveryActionReport, RecoveryReportEntry, allowed_recovery_commands};
#[cfg(feature = "postgres")]
use auction_domain::{LotId, SettlementId};

use super::DatabaseStore;

impl RecoveryReportReader for DatabaseStore {
    fn recovery_report(
        &self,
        auction_id: AuctionId,
    ) -> ApplicationFuture<'_, Option<RecoveryReport>> {
        Box::pin(async move {
            if auction_id.is_nil() {
                return Err(storage_error("recovery 报告拍卖标识无效"));
            }

            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => postgres_report(store, auction_id).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "recovery 报告仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }
}

#[cfg(feature = "postgres")]
async fn postgres_report(
    store: &super::PostgresStore,
    auction_id: AuctionId,
) -> Result<Option<RecoveryReport>, ApplicationError> {
    use sqlx::Row;
    use uuid::Uuid;

    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("打开 recovery 报告快照失败"))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("设置 recovery 报告快照失败"))?;

    let auction_exists = sqlx::query("SELECT id FROM public.auctions WHERE id = $1")
        .bind(auction_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| storage_error("读取 recovery 报告拍卖失败"))?;
    if auction_exists.is_none() {
        transaction.commit().await.map_err(|_| storage_error("提交 recovery 报告快照失败"))?;
        return Ok(None);
    }

    let recovery_rows = sqlx::query(
        r#"SELECT
               recovery.id,
               recovery.auction_id,
               recovery.lot_id,
               recovery.world_id,
               recovery.stage,
               recovery.resolution,
               recovery.state_version,
               recovery.payment_deadline_at,
               recovery.reminder_count,
               recovery.reminder_limit,
               recovery.custody_proof_kind,
               recovery.custody_proof_observed_at,
               recovery.blocking_code,
               recovery.manual_reconcile_delivery_id,
               recovery.relisted_auction_id,
               recovery.relisted_lot_id,
               recovery.updated_at,
               lot.lot_number,
               equipment.name AS equipment_name
           FROM public.settlement_recoveries AS recovery
           INNER JOIN public.lots AS lot
               ON lot.id = recovery.lot_id AND lot.auction_id = recovery.auction_id
           INNER JOIN public.equipment AS equipment ON equipment.id = lot.equipment_id
           WHERE recovery.auction_id = $1
           ORDER BY lot.lot_number, recovery.id"#,
    )
    .bind(auction_id.as_uuid())
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 recovery case 失败"))?;

    let mut recoveries = Vec::with_capacity(recovery_rows.len());
    for row in recovery_rows {
        let recovery_id: Uuid =
            row.try_get("id").map_err(|_| storage_error("读取 recovery 标识失败"))?;
        let stored_auction_id: Uuid =
            row.try_get("auction_id").map_err(|_| storage_error("读取 recovery 拍卖标识失败"))?;
        let stored_lot_id: Uuid =
            row.try_get("lot_id").map_err(|_| storage_error("读取 recovery lot 标识失败"))?;
        if stored_auction_id != auction_id.as_uuid() {
            return Err(storage_error("recovery 拍卖标识与报告不一致"));
        }

        let action_rows = sqlx::query(
            r#"SELECT
                   id,
                   kind,
                   execution_state,
                   delivery_id,
                   last_error_code,
                   proof_kind,
                   proof_observed_at,
                   updated_at
               FROM public.settlement_recovery_actions
               WHERE recovery_id = $1
               ORDER BY kind, id"#,
        )
        .bind(recovery_id)
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| storage_error("读取 recovery action 失败"))?;

        let actions = action_rows
            .into_iter()
            .map(|action_row| {
                Ok(RecoveryActionReport {
                    id: SettlementId::from_uuid(
                        action_row
                            .try_get("id")
                            .map_err(|_| storage_error("读取 recovery action 标识失败"))?,
                    ),
                    kind: action_row
                        .try_get("kind")
                        .map_err(|_| storage_error("读取 recovery action 类型失败"))?,
                    state: action_row
                        .try_get("execution_state")
                        .map_err(|_| storage_error("读取 recovery action 状态失败"))?,
                    delivery_id: action_row
                        .try_get::<Option<Uuid>, _>("delivery_id")
                        .map_err(|_| storage_error("读取 recovery action 投递标识失败"))?
                        .map(SettlementId::from_uuid),
                    failure_code: action_row
                        .try_get("last_error_code")
                        .map_err(|_| storage_error("读取 recovery action 错误码失败"))?,
                    proof_kind: action_row
                        .try_get("proof_kind")
                        .map_err(|_| storage_error("读取 recovery action 证明类型失败"))?,
                    proof_observed_at: action_row
                        .try_get("proof_observed_at")
                        .map_err(|_| storage_error("读取 recovery action 证明时间失败"))?,
                    updated_at: action_row
                        .try_get("updated_at")
                        .map_err(|_| storage_error("读取 recovery action 更新时间失败"))?,
                })
            })
            .collect::<Result<Vec<_>, ApplicationError>>()?;

        let mut entry = RecoveryReportEntry {
            id: SettlementId::from_uuid(recovery_id),
            auction_id: AuctionId::from_uuid(stored_auction_id),
            lot_id: LotId::from_uuid(stored_lot_id),
            lot_number: row
                .try_get("lot_number")
                .map_err(|_| storage_error("读取 recovery lot 序号失败"))?,
            world_id: row
                .try_get("world_id")
                .map_err(|_| storage_error("读取 recovery 世界失败"))?,
            equipment_name: row
                .try_get("equipment_name")
                .map_err(|_| storage_error("读取 recovery 装备名称失败"))?,
            stage: row.try_get("stage").map_err(|_| storage_error("读取 recovery 阶段失败"))?,
            resolution: row
                .try_get("resolution")
                .map_err(|_| storage_error("读取 recovery 结论失败"))?,
            state_version: row
                .try_get("state_version")
                .map_err(|_| storage_error("读取 recovery 状态版本失败"))?,
            payment_deadline_at: row
                .try_get("payment_deadline_at")
                .map_err(|_| storage_error("读取 recovery 付款截止时间失败"))?,
            reminder_count: row
                .try_get("reminder_count")
                .map_err(|_| storage_error("读取 recovery 提醒次数失败"))?,
            reminder_limit: row
                .try_get("reminder_limit")
                .map_err(|_| storage_error("读取 recovery 提醒上限失败"))?,
            custody_proof_kind: row
                .try_get("custody_proof_kind")
                .map_err(|_| storage_error("读取 recovery custody 证明类型失败"))?,
            custody_proof_observed_at: row
                .try_get("custody_proof_observed_at")
                .map_err(|_| storage_error("读取 recovery custody 证明时间失败"))?,
            blocking_code: row
                .try_get("blocking_code")
                .map_err(|_| storage_error("读取 recovery 阻塞码失败"))?,
            manual_reconcile_delivery_id: row
                .try_get::<Option<Uuid>, _>("manual_reconcile_delivery_id")
                .map_err(|_| storage_error("读取 recovery 人工对账投递标识失败"))?
                .map(SettlementId::from_uuid),
            relisted_auction_id: row
                .try_get::<Option<Uuid>, _>("relisted_auction_id")
                .map_err(|_| storage_error("读取 recovery 重挂牌卖标识失败"))?
                .map(AuctionId::from_uuid),
            relisted_lot_id: row
                .try_get::<Option<Uuid>, _>("relisted_lot_id")
                .map_err(|_| storage_error("读取 recovery 重挂牌 lot 标识失败"))?
                .map(LotId::from_uuid),
            actions,
            allowed_commands: Vec::new(),
            updated_at: row
                .try_get("updated_at")
                .map_err(|_| storage_error("读取 recovery 更新时间失败"))?,
        };
        entry.allowed_commands = allowed_recovery_commands(&entry);
        recoveries.push(entry);
    }

    transaction.commit().await.map_err(|_| storage_error("提交 recovery 报告快照失败"))?;
    Ok(Some(RecoveryReport { auction_id, recoveries }))
}

fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(auction_application::RepositoryError::Failed(message.to_owned()))
}
