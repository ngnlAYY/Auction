//! PostgreSQL Admin 业务世界启停事务。

use auction_application::admin::{AdminWorldSetEnabledCommand, AdminWorldTransaction};
use auction_application::auction::WorldSnapshot;
use auction_application::core::CompetitionTransactionError;
use serde_json::json;
use sqlx::Row;
use sqlx::postgres::PgRow;
use uuid::Uuid;

use crate::database::auction::competition::PostgresCompetitionTransaction;
use crate::database::auction::competition::mapping::invalid_state;
use crate::database::auction::competition::mapping::map_sql_error;
use crate::database::operations::audit_health::AuditWriteResult;

#[allow(clippy::manual_async_fn)]
impl AdminWorldTransaction for PostgresCompetitionTransaction {
    fn load_world_for_update(
        &mut self,
        world_id: &str,
    ) -> impl Future<Output = Result<Option<WorldSnapshot>, CompetitionTransactionError>> + Send
    {
        async move { load_world_for_update_inner(self, world_id).await }
    }

    fn has_world_deactivation_blocker(
        &mut self,
        world_id: &str,
    ) -> impl Future<Output = Result<bool, CompetitionTransactionError>> + Send {
        async move { has_world_deactivation_blocker_inner(self, world_id).await }
    }

    fn update_world_enabled(
        &mut self,
        world_id: &str,
        enabled: bool,
        expected_policy_revision: i64,
    ) -> impl Future<Output = Result<WorldSnapshot, CompetitionTransactionError>> + Send {
        async move {
            update_world_enabled_inner(self, world_id, enabled, expected_policy_revision).await
        }
    }

    fn append_admin_world_record(
        &mut self,
        command: AdminWorldSetEnabledCommand,
        previous: WorldSnapshot,
        result: WorldSnapshot,
        changed: bool,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        async move { append_admin_world_record_inner(self, &command, &previous, &result, changed).await }
    }
}

async fn load_world_for_update_inner(
    transaction: &mut PostgresCompetitionTransaction,
    world_id: &str,
) -> Result<Option<WorldSnapshot>, CompetitionTransactionError> {
    let transaction = transaction.transaction_mut()?;
    let row = sqlx::query(
        "SELECT id, enabled, availability, policy_revision
         FROM public.worlds
         WHERE id = $1
         FOR UPDATE",
    )
    .bind(world_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?;

    row.map(world_snapshot).transpose()
}

/// world 行已由调用方先锁定；随后只读取子对象，保持 world → round/auction 的统一锁序。
async fn has_world_deactivation_blocker_inner(
    transaction: &mut PostgresCompetitionTransaction,
    world_id: &str,
) -> Result<bool, CompetitionTransactionError> {
    let transaction = transaction.transaction_mut()?;
    sqlx::query_scalar::<_, bool>(
        r#"SELECT EXISTS (
               SELECT 1
               FROM public.auctions AS auction
               WHERE auction.world_id = $1
                 AND auction.status NOT IN ('settled', 'cancelled')
               UNION ALL
               SELECT 1
               FROM public.scheduled_rounds AS round
               INNER JOIN public.auctions AS auction
                   ON auction.id = round.auction_id
                  AND auction.world_id = round.world_id
               WHERE round.world_id = $1
                 AND round.status = 'active'
                 AND round.final_completed_at IS NULL
           )"#,
    )
    .bind(world_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)
}

async fn update_world_enabled_inner(
    transaction: &mut PostgresCompetitionTransaction,
    world_id: &str,
    enabled: bool,
    expected_policy_revision: i64,
) -> Result<WorldSnapshot, CompetitionTransactionError> {
    let transaction = transaction.transaction_mut()?;
    let row = sqlx::query(
        "UPDATE public.worlds
         SET enabled = $2,
             policy_revision = policy_revision + 1,
             updated_at = transaction_timestamp()
         WHERE id = $1
           AND policy_revision = $3
         RETURNING id, enabled, availability, policy_revision",
    )
    .bind(world_id)
    .bind(enabled)
    .bind(expected_policy_revision)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(CompetitionTransactionError::Conflict)?;
    world_snapshot(row)
}

async fn append_admin_world_record_inner(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AdminWorldSetEnabledCommand,
    previous: &WorldSnapshot,
    result: &WorldSnapshot,
    changed: bool,
) -> Result<(), CompetitionTransactionError> {
    let metadata = json!({
        "command_id": command.command_id.as_uuid(),
        "world_id": result.id,
        "previous_enabled": previous.enabled,
        "enabled": result.enabled,
        "previous_policy_revision": previous.policy_revision,
        "policy_revision": result.policy_revision,
        "changed": changed,
    });
    let transaction = transaction.transaction_mut()?;
    sqlx::query(
        r#"INSERT INTO public.audit_records
            (id, actor_type, actor_id, action, entity_type, entity_id, result, request_id, source, metadata)
         VALUES ($1, 'admin', $2, 'world.set_enabled', 'world', $3, 'success', $4, 'admin', $5)"#,
    )
    .bind(Uuid::new_v4())
    .bind(&command.actor_id)
    .bind(&result.id)
    .bind(command.request_id.as_deref())
    .bind(metadata)
    .execute(&mut **transaction)
    .await
    .record_audit_failure()
    .map_err(map_sql_error)?;
    Ok(())
}

fn world_snapshot(row: PgRow) -> Result<WorldSnapshot, CompetitionTransactionError> {
    let snapshot = WorldSnapshot {
        id: row.try_get("id").map_err(map_sql_error)?,
        enabled: row.try_get("enabled").map_err(map_sql_error)?,
        availability: row.try_get("availability").map_err(map_sql_error)?,
        policy_revision: row.try_get("policy_revision").map_err(map_sql_error)?,
    };
    if !snapshot.validate() {
        return Err(invalid_state("业务世界投影无效"));
    }
    Ok(snapshot)
}
