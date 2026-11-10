//! 结算初次领取时的 lot 锁和 due 世代校验。

use crate::database::TrackedTransaction as Transaction;
use chrono::{DateTime, Utc};
use sqlx::{Postgres, Row};
use uuid::Uuid;

/// 锁定同一拍卖的全部 lot，并确认软截止没有在拍卖行锁之后发生变化。
pub(super) async fn lock_initial_lots(
    transaction: &mut Transaction<'_, Postgres>,
    auction_id: Uuid,
    now: DateTime<Utc>,
    expected_due_at: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, sqlx::Error> {
    let rows = sqlx::query(
        r#"SELECT status, effective_end_at
           FROM public.lots
           WHERE auction_id = $1
           ORDER BY lot_number, id
           FOR UPDATE"#,
    )
    .bind(auction_id)
    .fetch_all(&mut **transaction)
    .await?;
    if rows.is_empty() {
        return Ok(None);
    }

    let mut latest_end = None;
    for row in rows {
        let status: String = row.try_get("status")?;
        // 撤回记录仍保留行锁，但不参与结算截止世代；其他非 live 状态继续失败关闭。
        if status == "withdrawn" {
            continue;
        }
        let effective_end_at: Option<DateTime<Utc>> = row.try_get("effective_end_at")?;
        let Some(effective_end_at) = effective_end_at else {
            return Ok(None);
        };
        if status != "live" || effective_end_at > now {
            return Ok(None);
        }
        latest_end = Some(
            latest_end
                .map_or(effective_end_at, |current: DateTime<Utc>| current.max(effective_end_at)),
        );
    }
    Ok(latest_end.filter(|latest| *latest == expected_due_at))
}
