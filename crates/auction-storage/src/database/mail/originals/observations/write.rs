//! 观察行的唯一写入路径：首次观察、隔离复制、追加观察与实物身份登记。

use auction_application::consignment::DepositBusinessRejection;
use auction_domain::consignment::deposit::EnvelopeItemKind;
use sqlx::PgConnection;
use uuid::Uuid;

use super::super::{conflict, invariant};
use super::facts::{derived_facts_json, manifest_json};
use super::{ObservationWrite, RawObservation, load_observation};
use crate::database::consignment::commands::errors::WriteError;
use crate::database::mail::originals::identity::MailKeyColumns;

/// 写入一条观察行；不更新任何既有行。
pub(crate) async fn write_observation_in_tx(
    connection: &mut PgConnection,
    write: ObservationWrite<'_>,
) -> Result<Uuid, WriteError> {
    match write {
        ObservationWrite::Fresh { raw } => {
            let columns = MailKeyColumns::from_key(raw.mail_key);
            for item in &raw.observation.items {
                if item.kind == EnvelopeItemKind::Equipment
                    && !item.display_name.as_deref().is_some_and(|name| !name.is_empty())
                {
                    return Err(invariant("consignment_observation_display_name"));
                }
                if item.display_name.as_deref().is_some_and(|name| name.len() > 256) {
                    return Err(invariant("consignment_observation_display_name_length"));
                }
            }
            if raw.received_minute_raw.len() > 64 {
                return Err(invariant("consignment_observation_received_minute"));
            }
            let raw_count = raw
                .raw_count
                .and_then(|value| (value > 0).then(|| i32::try_from(value).ok()).flatten());
            let manifest = manifest_json(raw.observation);
            let derived = derived_facts_json(
                raw.observation,
                raw.mail_key.world(),
                raw.received_minute_raw,
                &raw.legacy_evidence,
            );
            sqlx::query(
                "INSERT INTO public.mail_envelope_observations
                    (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
                     mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
                     evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
                     returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
                     season_window_id)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, NULL, NULL, $8, $9, $10, $11, $12, $13,
                         $14, $15, $16, $17, $18, $19, $20, $21, $22, $23)
                 ON CONFLICT (id) DO NOTHING",
            )
            .bind(raw.observation.observation_id)
            .bind(columns.bot_uid)
            .bind(columns.world_id)
            .bind(columns.external_mid)
            .bind(raw.remote_observation_id)
            .bind(raw.session_id)
            .bind(raw.mailbox_key)
            .bind(raw.observed_at)
            .bind(raw.parser_version)
            .bind(raw.evidence_ref)
            .bind(raw.subject_digest.as_slice())
            .bind(raw.body_digest.as_slice())
            .bind(raw.observation.manifest_digest.as_slice())
            .bind(raw.sender)
            .bind(raw.recipient)
            .bind(raw.observation.returnable)
            .bind(raw.observation.takeable)
            .bind(raw.observation.credits)
            .bind(raw.observation.cod)
            .bind(raw_count)
            .bind(&manifest)
            .bind(&derived)
            .bind(raw.season_window_id)
            .execute(&mut *connection)
            .await?;
            Ok(raw.observation.observation_id)
        }
        ObservationWrite::Copy {
            source_id,
            observation_id,
            remote_observation_id,
            quarantine_id,
            quarantine_version,
            legacy_auction_owner,
        } => {
            let legacy_auction_owner =
                legacy_auction_owner.map(auction_domain::consignment::LegacyAuctionOwner::as_str);
            let inserted = sqlx::query(
                "INSERT INTO public.mail_envelope_observations
                    (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
                     mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
                     evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
                     returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
                     season_window_id)
                 SELECT $1, bot_uid, world_id, external_mid, $2, session_id, mailbox_key,
                        $3, $4, observed_at, parser_version, evidence_ref, subject_digest,
                        body_digest, manifest_digest, sender, recipient, returnable, takeable,
                        credits, cod, raw_count, manifest,
                        CASE WHEN $6::text IS NULL THEN derived_facts
                             ELSE jsonb_set(derived_facts, '{legacy_auction_owner}',
                                            to_jsonb($6::text), false)
                        END,
                        season_window_id
                 FROM public.mail_envelope_observations WHERE id = $5",
            )
            .bind(observation_id)
            .bind(remote_observation_id)
            .bind(quarantine_id)
            .bind(i64::try_from(quarantine_version)?)
            .bind(source_id)
            .bind(legacy_auction_owner)
            .execute(&mut *connection)
            .await?;
            if inserted.rows_affected() != 1 {
                return Err(conflict(DepositBusinessRejection::EvidenceConflict));
            }
            Ok(observation_id)
        }
    }
}

/// 记录原信、观察与逐件实物身份；相同身份重放只核对，不覆盖首次证据。
pub(crate) async fn record_observation_in_tx(
    connection: &mut PgConnection,
    raw: &RawObservation<'_>,
) -> Result<Uuid, WriteError> {
    let key = raw.mail_key;
    let columns = MailKeyColumns::from_key(key);
    let session = sqlx::query(
        "SELECT bot_uid, world_id, binding_digest
         FROM public.mail_evidence_sessions WHERE id = $1",
    )
    .bind(raw.session_id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or_else(|| conflict(DepositBusinessRejection::EvidenceUnproven))?;
    let session_bot: String = sqlx::Row::try_get(&session, "bot_uid")?;
    let session_world: String = sqlx::Row::try_get(&session, "world_id")?;
    let binding: &[u8] = sqlx::Row::try_get(&session, "binding_digest")?;
    if session_bot != columns.bot_uid
        || session_world != columns.world_id
        || binding != raw.observation.session_binding_digest
    {
        return Err(conflict(DepositBusinessRejection::EvidenceUnproven));
    }
    sqlx::query(
        "INSERT INTO public.mail_originals
            (bot_uid, world_id, external_mid, first_session_id, received_minute_raw,
             received_timezone, first_seen_at, season_window_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         ON CONFLICT (bot_uid, world_id, external_mid) DO NOTHING",
    )
    .bind(columns.bot_uid)
    .bind(columns.world_id)
    .bind(columns.external_mid)
    .bind(raw.session_id)
    .bind(raw.received_minute_raw)
    .bind(raw.received_timezone)
    .bind(raw.first_seen_at)
    .bind(raw.season_window_id)
    .execute(&mut *connection)
    .await?;
    write_observation_in_tx(connection, ObservationWrite::Fresh { raw }).await?;
    let stored = load_observation(connection, key, raw.observation.observation_id)
        .await?
        .ok_or_else(|| invariant("consignment_observation_missing_after_insert"))?;
    if stored.manifest_digest != raw.observation.manifest_digest
        || stored.session_binding_digest != raw.observation.session_binding_digest
        || stored.parser_version != raw.parser_version
    {
        return Err(conflict(DepositBusinessRejection::EvidenceConflict));
    }
    let season_id: Option<String> = match raw.season_window_id {
        None => None,
        Some(window_id) => Some(
            sqlx::query_scalar(
                "SELECT season_id FROM public.consignment_season_windows WHERE id = $1",
            )
            .bind(window_id)
            .fetch_optional(&mut *connection)
            .await?
            .ok_or_else(|| invariant("consignment_observation_season_window"))?,
        ),
    };
    for item in &raw.observation.items {
        let (Some(canonical_eid), EnvelopeItemKind::Equipment) =
            (item.canonical_eid.as_deref(), item.kind)
        else {
            continue;
        };
        ensure_asset(
            connection,
            columns.world_id,
            canonical_eid,
            season_id.as_deref(),
            raw.observation.observation_id,
        )
        .await?;
    }
    Ok(raw.observation.observation_id)
}

async fn ensure_asset(
    connection: &mut PgConnection,
    world_id: &str,
    canonical_eid: &str,
    season_id: Option<&str>,
    observation_id: Uuid,
) -> Result<(), WriteError> {
    let existing: Option<Uuid> = sqlx::query_scalar(
        "SELECT equipment_id FROM public.consignment_custody_assets
         WHERE world_id = $1 AND canonical_eid = $2 AND season_id IS NOT DISTINCT FROM $3",
    )
    .bind(world_id)
    .bind(canonical_eid)
    .bind(season_id)
    .fetch_optional(&mut *connection)
    .await?;
    if existing.is_some() {
        return Ok(());
    }
    let equipment_id = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("auction/consignment/custody-asset/v1\0{world_id}\0{canonical_eid}\0{season_id:?}")
            .as_bytes(),
    );
    sqlx::query(
        "INSERT INTO public.consignment_custody_assets
            (equipment_id, world_id, canonical_eid, season_id, first_observation_id,
             auction_equipment_id)
         VALUES ($1, $2, $3, $4, $5, NULL)
         ON CONFLICT (world_id, canonical_eid, season_id) DO NOTHING",
    )
    .bind(equipment_id)
    .bind(world_id)
    .bind(canonical_eid)
    .bind(season_id)
    .bind(observation_id)
    .execute(&mut *connection)
    .await?;
    Ok(())
}
