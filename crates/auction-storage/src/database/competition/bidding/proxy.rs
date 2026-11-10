//! PostgreSQL 代理授权快照、状态转换和历史写入。

use std::collections::{HashMap, HashSet};

use super::super::mapping::*;
use crate::database::competition::PostgresCompetitionTransaction;
use auction_application::core::CompetitionTransactionError;
use auction_domain::{
    BidAmount, CompetitionPlan, ProxyAuthorizationId, ProxyMode, ProxyStatus, ProxyTerminalReason,
};
use chrono::{DateTime, Utc};

pub(in crate::database::competition) async fn apply_proxy_changes(
    transaction: &mut PostgresCompetitionTransaction,
    plan: &CompetitionPlan,
    occurred_at: DateTime<Utc>,
) -> Result<(), CompetitionTransactionError> {
    // 与命令结果共用锁后时刻，不能用可能早于上一条转换的事务开始时间。
    let created_ids: HashSet<ProxyAuthorizationId> = plan
        .proxy_transitions
        .iter()
        .filter(|transition| {
            matches!(transition.kind, auction_domain::ProxyTransitionKind::Created)
        })
        .map(|transition| transition.authorization_id)
        .collect();
    // 同一授权只保留最后一条转换的原因（含 None），避免批量关闭时逐状态反向重扫转换。
    let terminal_reasons: HashMap<_, _> = plan
        .proxy_transitions
        .iter()
        .map(|transition| (transition.authorization_id, transition.reason))
        .collect();

    for state in &plan.proxy_states {
        if created_ids.contains(&state.id) {
            continue;
        }
        let previous_version = state
            .state_version
            .checked_sub(1)
            .ok_or_else(|| invalid_state("代理快照缺少前一版本"))?;
        let reason = terminal_reasons.get(&state.id).copied().flatten();
        let transaction = transaction.transaction_mut()?;
        let result = sqlx::query(
            r#"UPDATE public.bid_proxy_authorizations
             SET maximum_amount = $2, mode = $3, status = $4, terminal_reason = $5,
                 state_version = $6, updated_at = $8
             WHERE authorization_id = $1 AND state_version = $7"#,
        )
        .bind(state.id.as_uuid())
        .bind(state.maximum_amount.value())
        .bind(state.mode.as_str())
        .bind(state.status.as_str())
        .bind(reason.map(ProxyTerminalReason::as_str))
        .bind(i64_version(state.state_version, "代理状态版本")?)
        .bind(i64_version(previous_version, "代理前一版本")?)
        .bind(occurred_at)
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
        let reason = terminal_reasons.get(&state.id).copied().flatten();
        let event_id =
            crate::database::lot_watch::proxy_event(state.id.as_uuid(), state.lot_id.as_uuid());
        {
            let transaction = transaction.transaction_mut()?;
            sqlx::query(
                r#"INSERT INTO public.bid_proxy_authorizations
                    (authorization_id, lot_id, user_id, authorization_sequence, usage_number,
                     maximum_amount, mode, status, terminal_reason, state_version,
                     creation_command_id, created_at, updated_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $12)"#,
            )
            .bind(state.id.as_uuid())
            .bind(state.lot_id.as_uuid())
            .bind(state.user_id.as_uuid())
            .bind(i64_version(state.authorization_sequence, "代理序号")?)
            .bind(i64_version(state.usage_number, "代理使用次数")?)
            .bind(state.maximum_amount.value())
            .bind(state.mode.as_str())
            .bind(state.status.as_str())
            .bind(reason.map(ProxyTerminalReason::as_str))
            .bind(i64_version(state.state_version, "代理状态版本")?)
            .bind(state.creation_command_id.as_uuid())
            .bind(occurred_at)
            .execute(&mut **transaction)
            .await
            .map_err(map_sql_error)?;
            // 新建代理的自动标记同样由提交后事件派生，不在 Serializable 事务内写标记。
            crate::database::lot_watch::record_proxy_event(
                transaction,
                event_id,
                state.id.as_uuid(),
                state.user_id.as_uuid(),
                state.lot_id.as_uuid(),
                state.state_version,
                occurred_at,
            )
            .await?;
        }
        transaction.record_lot_watch_event(event_id);
    }

    for transition in &plan.proxy_transitions {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            r#"INSERT INTO public.bid_proxy_authorization_transitions
                (authorization_id, transition_number, transition_kind, origin_command_id,
                 old_maximum_amount, new_maximum_amount, old_mode, new_mode, old_status,
                 new_status, reason, resulting_state_version, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)"#,
        )
        .bind(transition.authorization_id.as_uuid())
        .bind(i64_version(transition.state_version, "代理转换序号")?)
        .bind(transition.kind.as_str())
        .bind(transition.command_id.as_uuid())
        .bind(transition.old_maximum.map(BidAmount::value))
        .bind(transition.new_maximum.value())
        .bind(transition.old_mode.map(ProxyMode::as_str))
        .bind(transition.new_mode.as_str())
        .bind(transition.old_status.map(ProxyStatus::as_str))
        .bind(transition.new_status.as_str())
        .bind(transition.reason.map(ProxyTerminalReason::as_str))
        .bind(i64_version(transition.state_version, "代理转换版本")?)
        .bind(occurred_at)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
    }
    Ok(())
}
