//! PostgreSQL 代理授权快照、状态转换和历史写入。

use std::collections::HashSet;

use super::super::mapping::*;
use super::super::*;

pub(in crate::database::competition) async fn apply_proxy_changes(
    transaction: &mut PostgresCompetitionTransaction,
    plan: &CompetitionPlan,
) -> Result<(), CompetitionTransactionError> {
    let created_ids: HashSet<ProxyAuthorizationId> = plan
        .proxy_transitions
        .iter()
        .filter(|transition| {
            matches!(transition.kind, auction_domain::ProxyTransitionKind::Created)
        })
        .map(|transition| transition.authorization_id)
        .collect();

    for state in &plan.proxy_states {
        if created_ids.contains(&state.id) {
            continue;
        }
        let previous_version = state
            .state_version
            .checked_sub(1)
            .ok_or_else(|| invalid_state("代理快照缺少前一版本"))?;
        let reason = terminal_reason(plan, state.id);
        let transaction = transaction.transaction_mut()?;
        let result = sqlx::query(
            r#"UPDATE public.bid_proxy_authorizations
             SET maximum_amount = $2, mode = $3, status = $4, terminal_reason = $5,
                 state_version = $6, updated_at = CURRENT_TIMESTAMP
             WHERE authorization_id = $1 AND state_version = $7"#,
        )
        .bind(state.id.as_uuid())
        .bind(state.maximum_amount.value())
        .bind(proxy_mode(state.mode))
        .bind(proxy_status(state.status))
        .bind(reason.map(proxy_terminal_reason))
        .bind(i64_version(state.state_version, "代理状态版本")?)
        .bind(i64_version(previous_version, "代理前一版本")?)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
        if result.rows_affected() != 1 {
            return Err(CompetitionTransactionError::Failed(
                "代理快照版本与数据库不一致".to_owned(),
            ));
        }
    }

    for state in &plan.proxy_states {
        if !created_ids.contains(&state.id) {
            continue;
        }
        let reason = terminal_reason(plan, state.id);
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            r#"INSERT INTO public.bid_proxy_authorizations
                (authorization_id, lot_id, user_id, authorization_sequence, usage_number,
                 maximum_amount, mode, status, terminal_reason, state_version,
                 creation_command_id, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)"#,
        )
        .bind(state.id.as_uuid())
        .bind(state.lot_id.as_uuid())
        .bind(state.user_id.as_uuid())
        .bind(i64_version(state.authorization_sequence, "代理序号")?)
        .bind(i64_version(state.usage_number, "代理使用次数")?)
        .bind(state.maximum_amount.value())
        .bind(proxy_mode(state.mode))
        .bind(proxy_status(state.status))
        .bind(reason.map(proxy_terminal_reason))
        .bind(i64_version(state.state_version, "代理状态版本")?)
        .bind(state.creation_command_id.as_uuid())
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
    }

    for transition in &plan.proxy_transitions {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            r#"INSERT INTO public.bid_proxy_authorization_transitions
                (authorization_id, transition_number, transition_kind, origin_command_id,
                 old_maximum_amount, new_maximum_amount, old_mode, new_mode, old_status,
                 new_status, reason, resulting_state_version, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, CURRENT_TIMESTAMP)"#,
        )
        .bind(transition.authorization_id.as_uuid())
        .bind(i64_version(transition.state_version, "代理转换序号")?)
        .bind(proxy_transition_kind(transition.kind))
        .bind(transition.command_id.as_uuid())
        .bind(transition.old_maximum.map(BidAmount::value))
        .bind(transition.new_maximum.value())
        .bind(transition.old_mode.map(proxy_mode))
        .bind(proxy_mode(transition.new_mode))
        .bind(transition.old_status.map(proxy_status))
        .bind(proxy_status(transition.new_status))
        .bind(transition.reason.map(proxy_terminal_reason))
        .bind(i64_version(transition.state_version, "代理转换版本")?)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
    }
    Ok(())
}
