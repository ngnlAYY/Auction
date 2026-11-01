//! 成交撤销后的补偿池冲正写入。
//!
//! 只由恢复流程在 `resolution` 首次落定的同一 Serializable 事务内调用：此时成交已确定撤销
//! （买方未付款、装备退回卖家或已付款后退款），原结算计入池中的附加费与从池中支出的补偿
//! 都不会兑现。冲正金额由领域规则计算，这里负责锁池、版本 CAS、追加冲正事实与审计；
//! 原结算明细、补偿账本与冻结策略保持不变。

use auction_domain::{
    COMPENSATION_REVERSAL_RULE_VERSION, CompensationReversal, SettlementAmount,
    legacy_natural_key_uuid,
};
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use crate::database::audit_health::AuditWriteResult as _;

/// 冲正写入失败；数据库错误保留原值，供调用方识别可重试的序列化冲突。
#[derive(Debug)]
pub(crate) enum CompensationReversalError {
    Database(sqlx::Error),
    Invalid(&'static str),
}

impl From<sqlx::Error> for CompensationReversalError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

/// 预先锁定世界补偿池。结算先锁池、后经外键对 `worlds` 取 KEY SHARE；恢复事务必须在锁
/// `worlds` 或写入引用它的义务之前先锁池，保持同一顺序，避免与同世界结算互相等待。
pub(crate) async fn lock_world_pool(
    connection: &mut PgConnection,
    world_id: &str,
) -> Result<(), CompensationReversalError> {
    sqlx::query("SELECT world_id FROM public.settlement_pools WHERE world_id = $1 FOR UPDATE")
        .bind(world_id)
        .fetch_optional(&mut *connection)
        .await?
        .ok_or(CompensationReversalError::Invalid("世界补偿池不存在"))?;
    Ok(())
}

/// 为已落定撤销的恢复记录冲正补偿池。附加费与补偿都为零（含无补偿池的旧策略）时不写任何事实。
/// 同一恢复记录重复冲正会触发唯一约束并整体回滚，绝不重复改变余额。
pub(crate) async fn reverse_voided_sale(
    connection: &mut PgConnection,
    recovery_id: Uuid,
    occurred_at: DateTime<Utc>,
) -> Result<(), CompensationReversalError> {
    let sale = sqlx::query(
        r#"SELECT recovery.world_id,
                  recovery.auction_id,
                  recovery.lot_id,
                  recovery.resolution,
                  settlement.pool_enabled,
                  line.tax_amount,
                  line.compensation_amount
           FROM public.settlement_recoveries AS recovery
           INNER JOIN public.auctions AS auction
               ON auction.id = recovery.auction_id AND auction.world_id = recovery.world_id
           INNER JOIN public.auction_settlements AS settlement
               ON settlement.auction_id = recovery.auction_id
           INNER JOIN public.lot_settlements AS line ON line.lot_id = recovery.lot_id
           WHERE recovery.id = $1 AND recovery.resolution IS NOT NULL"#,
    )
    .bind(recovery_id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(CompensationReversalError::Invalid("撤销的恢复记录缺少结算事实"))?;

    let world_id: String = sale.try_get("world_id")?;
    let auction_id: Uuid = sale.try_get("auction_id")?;
    let lot_id: Uuid = sale.try_get("lot_id")?;
    let resolution: String = sale.try_get("resolution")?;
    let pool_enabled: bool = sale.try_get("pool_enabled")?;
    let tax = amount(sale.try_get("tax_amount")?)?;
    let compensation = amount(sale.try_get("compensation_amount")?)?;
    if !pool_enabled {
        // 无补偿池的旧策略从未改变池余额，也就没有可冲正的金额。
        if tax.value() != 0 || compensation.value() != 0 {
            return Err(CompensationReversalError::Invalid("无补偿池场次存在附加费或补偿"));
        }
        return Ok(());
    }

    let pool = sqlx::query(
        r#"SELECT balance, state_version
           FROM public.settlement_pools
           WHERE world_id = $1
           FOR UPDATE"#,
    )
    .bind(&world_id)
    .fetch_one(&mut *connection)
    .await?;
    let opening_balance = amount(pool.try_get("balance")?)?;
    let opening_version: i64 = pool.try_get("state_version")?;
    let Some(reversal) = CompensationReversal::calculate(opening_balance, tax, compensation)
        .map_err(|_| CompensationReversalError::Invalid("补偿池冲正金额无效"))?
    else {
        return Ok(());
    };
    let closing_version = opening_version
        .checked_add(1)
        .ok_or(CompensationReversalError::Invalid("补偿池状态版本溢出"))?;

    let updated = sqlx::query(
        r#"UPDATE public.settlement_pools
           SET balance = $2, state_version = $3, updated_at = $4
           WHERE world_id = $1 AND balance = $5 AND state_version = $6"#,
    )
    .bind(&world_id)
    .bind(reversal.closing_balance.value())
    .bind(closing_version)
    .bind(occurred_at)
    .bind(opening_balance.value())
    .bind(opening_version)
    .execute(&mut *connection)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(CompensationReversalError::Invalid("补偿池版本与数据库不一致"));
    }

    let rule_version = i32::try_from(COMPENSATION_REVERSAL_RULE_VERSION)
        .map_err(|_| CompensationReversalError::Invalid("补偿池冲正规则版本无效"))?;
    sqlx::query(
        r#"INSERT INTO public.compensation_pool_reversals
               (id, recovery_id, world_id, auction_id, lot_id, rule_version, resolution,
                tax_amount, compensation_amount, tax_reversed, opening_balance, closing_balance,
                opening_pool_version, closing_pool_version, created_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)"#,
    )
    .bind(legacy_natural_key_uuid("auction.compensation.reversal.v1", &[recovery_id.as_bytes()]))
    .bind(recovery_id)
    .bind(&world_id)
    .bind(auction_id)
    .bind(lot_id)
    .bind(rule_version)
    .bind(&resolution)
    .bind(tax.value())
    .bind(compensation.value())
    .bind(reversal.tax_reversed.value())
    .bind(reversal.opening_balance.value())
    .bind(reversal.closing_balance.value())
    .bind(opening_version)
    .bind(closing_version)
    .bind(occurred_at)
    .execute(&mut *connection)
    .await?;

    sqlx::query(
        r#"INSERT INTO public.audit_records
               (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id,
                result, request_id, source, metadata)
           VALUES ($1, $2, 'system', 'compensation-pool', 'auction.compensation.reverse',
                   'settlement_recovery', $3, 'success', NULL, 'system', $4)"#,
    )
    .bind(legacy_natural_key_uuid(
        "auction.compensation.reversal-audit.v1",
        &[recovery_id.as_bytes()],
    ))
    .bind(occurred_at)
    .bind(recovery_id.to_string())
    .bind(json!({
        "world_id": world_id,
        "auction_id": auction_id,
        "lot_id": lot_id,
        "resolution": resolution,
        "rule_version": COMPENSATION_REVERSAL_RULE_VERSION,
        "tax_amount": tax.value(),
        "compensation_amount": compensation.value(),
        "tax_reversed": reversal.tax_reversed.value(),
        "tax_shortfall": reversal.tax_shortfall.value(),
        "opening_balance": reversal.opening_balance.value(),
        "closing_balance": reversal.closing_balance.value(),
        "opening_pool_version": opening_version,
        "closing_pool_version": closing_version,
    }))
    .execute(&mut *connection)
    .await
    .record_audit_failure()?;
    Ok(())
}

fn amount(value: i64) -> Result<SettlementAmount, CompensationReversalError> {
    SettlementAmount::new(value).map_err(|_| CompensationReversalError::Invalid("结算金额无效"))
}
