//! Delivery Admin重试的两阶段恢复锁；不取得资金、delivery或业务中间层锁。

use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

/// 资金/所需auction之后、lot之前锁父recovery；即使无关联也验证delivery存在。
pub(crate) async fn lock_recovery_delivery_retry_scope(
    tx: &mut Transaction<'_, Postgres>,
    delivery_id: Uuid,
) -> Result<(), sqlx::Error> {
    let association = read_association(tx, delivery_id).await?;
    if let Some((_, recovery_id)) = association {
        sqlx::query(
            "SELECT id FROM public.settlement_recoveries WHERE id=$1 ORDER BY id FOR UPDATE",
        )
        .bind(recovery_id)
        .fetch_one(&mut **tx)
        .await?;
    }
    if read_association(tx, delivery_id).await? != association {
        return Err(invalid_association());
    }
    Ok(())
}

/// 调用方已完成scope及所需lot/equipment/credentials/users/obligations锁，再锁actions。
/// 必须和scope、delivery锁、reset保持同一Serializable事务。
pub(crate) async fn lock_recovery_delivery_retry_actions(
    tx: &mut Transaction<'_, Postgres>,
    delivery_id: Uuid,
) -> Result<(), sqlx::Error> {
    let association = read_association(tx, delivery_id).await?;
    if let Some((action_id, _)) = association {
        sqlx::query(
            "SELECT id FROM public.settlement_recovery_actions WHERE id=$1 ORDER BY id FOR UPDATE",
        )
        .bind(action_id)
        .fetch_one(&mut **tx)
        .await?;
    }
    if read_association(tx, delivery_id).await? != association {
        return Err(invalid_association());
    }
    Ok(())
}

async fn read_association(
    tx: &mut Transaction<'_, Postgres>,
    delivery_id: Uuid,
) -> Result<Option<(Uuid, Uuid)>, sqlx::Error> {
    if delivery_id.is_nil() {
        return Err(invalid_association());
    }
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM public.deliveries WHERE id=$1)")
            .bind(delivery_id)
            .fetch_one(&mut **tx)
            .await?;
    if !exists {
        return Err(invalid_association());
    }
    let rows = sqlx::query("SELECT a.id,a.recovery_id,r.id AS parent_id FROM public.settlement_recovery_actions a LEFT JOIN public.settlement_recoveries r ON r.id=a.recovery_id WHERE a.delivery_id=$1 ORDER BY a.id")
        .bind(delivery_id).fetch_all(&mut **tx).await?;
    let Some(row) = rows.first() else { return Ok(None) };
    let recovery_id: Uuid = row.try_get("recovery_id")?;
    if rows.len() != 1 || row.try_get::<Option<Uuid>, _>("parent_id")? != Some(recovery_id) {
        return Err(invalid_association());
    }
    Ok(Some((row.try_get("id")?, recovery_id)))
}

fn invalid_association() -> sqlx::Error {
    sqlx::Error::Protocol("恢复重试持久关联缺失或冲突".to_owned())
}
