#[cfg(feature = "postgres")]
use super::*;

#[cfg(feature = "postgres")]
pub(super) async fn insert_recovery_action(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    spec: RecoveryActionInsert<'_>,
) -> Result<(), auction_application::RepositoryError> {
    let action_id = deterministic_uuid(
        CommandId::from_uuid(spec.recovery_id),
        &format!("recovery-action:{}", spec.kind),
        1,
    );
    sqlx::query(
        r#"INSERT INTO public.settlement_recovery_actions
               (id, recovery_id, kind, ordinal, execution_state, state_version,
                source_mail_id, expected_credits, expected_cod, expected_attachment_ids,
                created_at, updated_at)
           VALUES ($1, $2, $3, $4, 'pending', 1, $5, $6, $7, $8, $9, $9)
           ON CONFLICT (recovery_id, kind) DO NOTHING"#,
    )
    .bind(action_id)
    .bind(spec.recovery_id)
    .bind(spec.kind)
    .bind(spec.ordinal)
    .bind(spec.source_mail_id)
    .bind(spec.expected_credits)
    .bind(spec.expected_cod)
    .bind(spec.expected_attachment_ids)
    .bind(spec.now)
    .execute(&mut **transaction)
    .await
    .map_err(map_storage_error)?;

    let row = sqlx::query(
        r#"SELECT id, ordinal, source_mail_id, expected_credits, expected_cod,
                  expected_attachment_ids
           FROM public.settlement_recovery_actions
           WHERE recovery_id = $1 AND kind = $2
           FOR UPDATE"#,
    )
    .bind(spec.recovery_id)
    .bind(spec.kind)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    let stored_id: Uuid = row.try_get("id").map_err(map_storage_error)?;
    let stored_ordinal: i32 = row.try_get("ordinal").map_err(map_storage_error)?;
    let stored_source: Option<Uuid> = row.try_get("source_mail_id").map_err(map_storage_error)?;
    let stored_credits: i64 = row.try_get("expected_credits").map_err(map_storage_error)?;
    let stored_cod: i64 = row.try_get("expected_cod").map_err(map_storage_error)?;
    let stored_attachments: Vec<String> =
        row.try_get("expected_attachment_ids").map_err(map_storage_error)?;
    if stored_id != action_id
        || stored_ordinal != spec.ordinal
        || stored_source != spec.source_mail_id
        || stored_credits != spec.expected_credits
        || stored_cod != spec.expected_cod
        || stored_attachments != spec.expected_attachment_ids
    {
        return Err(repository_error("recovery action 证据内容冲突"));
    }
    Ok(())
}
