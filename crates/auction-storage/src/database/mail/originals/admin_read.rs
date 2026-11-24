//! Admin 隔离详情的分页、观察投影与已解除决定读取。

use auction_application::consignment::{
    AdminDepositAction, AdminDepositDetail, AdminDepositQuery, AdminObservationItem,
    AdminQuarantineObservation, ConsignmentError, QuarantineState,
};
use auction_domain::SettlementWorld;
use auction_time::Timestamp;
use serde_json::Value;
use sqlx::Row;

use super::reads::read_final_decision;
use super::{invariant_decoding, unavailable_decoding};
use crate::database::consignment::commands::errors::WriteError;

/// 读取 Admin 隔离详情；非法组合失败关闭。
pub(crate) async fn read_quarantine(
    repository: &crate::database::consignment::deposit::repository::DepositRepository,
    query: AdminDepositQuery,
) -> Result<Option<AdminDepositDetail>, ConsignmentError> {
    let pool = repository.store().pool();
    let mut tx = pool.begin_read_snapshot().await.map_err(storage)?;
    let row = sqlx::query(
        "SELECT q.id, q.version, q.state, q.world_id, q.external_mid, q.initial_decision_id,
                q.reread_locator, q.responsibility_code, initial.kind AS initial_kind
         FROM public.mail_quarantines AS q
         JOIN public.mail_initial_decisions AS initial
           ON initial.bot_uid = q.bot_uid AND initial.world_id = q.world_id
          AND initial.external_mid = q.external_mid AND initial.id = q.initial_decision_id
         WHERE q.id = $1",
    )
    .bind(query.quarantine_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(storage)?;
    let Some(row) = row else {
        tx.commit().await.map_err(storage)?;
        return Ok(None);
    };
    let state_text: String = row.try_get("state").map_err(storage)?;
    let state =
        if state_text == "active" { QuarantineState::Active } else { QuarantineState::Released };
    let observation_total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.mail_envelope_observations WHERE quarantine_id = $1",
    )
    .bind(query.quarantine_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(storage)?;
    let cursor = match query.cursor {
        Some(cursor_id) => {
            let cursor_row = sqlx::query(
                "SELECT observed_at FROM public.mail_envelope_observations
                 WHERE id = $1 AND quarantine_id = $2",
            )
            .bind(cursor_id)
            .bind(query.quarantine_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            let Some(cursor_row) = cursor_row else {
                tx.rollback().await.map_err(storage)?;
                return Err(ConsignmentError::InvalidQuery);
            };
            Some((cursor_row.try_get::<Timestamp, _>("observed_at").map_err(storage)?, cursor_id))
        }
        None => None,
    };
    let world = auction_domain::consignment::parse_settlement_world(
        &row.try_get::<String, _>("world_id").map_err(storage)?,
    )
    .ok_or(ConsignmentError::Unavailable)?;
    let observations = sqlx::query(
        "SELECT id, observed_at, parser_version, manifest_digest, credits, cod, returnable,
                takeable, manifest
         FROM public.mail_envelope_observations
         WHERE quarantine_id = $1
           AND ($2::timestamptz IS NULL OR (observed_at, id) < ($2, $3))
         ORDER BY observed_at DESC, id DESC LIMIT $4",
    )
    .bind(query.quarantine_id)
    .bind(cursor.map(|value| value.0))
    .bind(cursor.map(|value| value.1))
    .bind(i64::from(query.limit))
    .fetch_all(&mut *tx)
    .await
    .map_err(storage)?;
    let observations = observations
        .into_iter()
        .map(|row| admin_observation(&row, world))
        .collect::<Result<Vec<_>, _>>()
        .map_err(WriteError::into_application)?;
    let final_decision = if state == QuarantineState::Released {
        Some(read_final_decision(&mut tx, query.quarantine_id).await?)
    } else {
        None
    };
    tx.commit().await.map_err(storage)?;
    Ok(Some(AdminDepositDetail {
        quarantine_id: query.quarantine_id,
        version: u64::try_from(row.try_get::<i64, _>("version").map_err(storage)?)
            .map_err(unavailable_decoding("consignment_quarantine_version"))?,
        state,
        world,
        external_mid: row.try_get("external_mid").map_err(storage)?,
        first_decision_id: row.try_get("initial_decision_id").map_err(storage)?,
        reason: quarantine_reason(&row)?,
        observation_total: u64::try_from(observation_total)
            .map_err(unavailable_decoding("consignment_observation_total"))?,
        observations,
        allowed_actions: if state == QuarantineState::Active {
            vec![AdminDepositAction::Recheck, AdminDepositAction::ReleaseQuarantine]
        } else {
            Vec::new()
        },
        final_decision,
    }))
}

fn admin_observation(
    row: &sqlx::postgres::PgRow,
    world: SettlementWorld,
) -> Result<AdminQuarantineObservation, WriteError> {
    let digest: &[u8] = row.try_get("manifest_digest")?;
    let manifest: Value = row.try_get("manifest")?;
    let items = super::observations::parse_manifest(&manifest, world)?;
    Ok(AdminQuarantineObservation {
        observation_id: row.try_get("id")?,
        observed_at: row.try_get("observed_at")?,
        parser_version: row.try_get("parser_version")?,
        digest: <[u8; 32]>::try_from(digest)
            .map_err(invariant_decoding("consignment_observation_manifest_digest"))?,
        credits: row.try_get("credits")?,
        cod: row.try_get("cod")?,
        returnable: row.try_get("returnable")?,
        takeable: row.try_get("takeable")?,
        items: items
            .into_iter()
            .map(|item| AdminObservationItem {
                ordinal: item.ordinal,
                kind: item.kind,
                canonical_eid: item.canonical_eid,
                amount: item.amount.and_then(|value| i64::try_from(value).ok()),
            })
            .collect(),
    })
}

fn quarantine_reason(
    row: &sqlx::postgres::PgRow,
) -> Result<auction_domain::consignment::deposit::QuarantineReason, ConsignmentError> {
    let code: String = row.try_get("responsibility_code").map_err(storage)?;
    super::identity::quarantine_reason(&code).ok_or(ConsignmentError::Unavailable)
}

fn storage(error: sqlx::Error) -> ConsignmentError {
    WriteError::from(error).into_application()
}
