//! 命令账本重放：同 ID 同摘要命中既有结果时返回原结果，不重复应用计划。

use auction_domain::{CompetitionPlan, ProxyAuthorizationId};

use super::commands::CompetitionCommand;
use super::legacy_ledger;
use crate::errors::ApplicationError;
use crate::ports::auction::CompetitionCommandOutcome;
use crate::ports::core::{CompetitionTransaction, StoredCommand};

/// 按命令账本结果重放；顺序重试与并发同 ID 提交共用同一路径，保证同 ID 同请求返回原结果。
pub(super) async fn replay_stored_command<T>(
    transaction: &mut T,
    command: &CompetitionCommand,
    stored: StoredCommand,
) -> Result<CompetitionCommandOutcome, ApplicationError>
where
    T: CompetitionTransaction,
{
    if !stored.matches_request(command.kind(), command.lot_id().as_uuid(), command.request_hash()) {
        return Err(crate::ports::core::CommandLedgerError::Conflict.into());
    }
    let lot_id = command.lot_id();
    match stored.result_code.as_str() {
        "accepted" => {
            let replayed: CompetitionPlan = serde_json::from_slice(&stored.result_payload)?;
            match command {
                CompetitionCommand::PlaceBid(_) => {
                    legacy_ledger::replay_accepted(command, replayed, &[], 0)
                }
                CompetitionCommand::CreateProxy(create) => {
                    let authorization_id =
                        ProxyAuthorizationId::from_uuid(legacy_ledger::deterministic_id(
                            create.command_id,
                            "auction-proxy-authorization",
                            1,
                        ));
                    let transitions =
                        transaction.proxy_transition_history(authorization_id).await?;
                    let quota_limit = transaction.proxy_quota_limit(lot_id).await?;
                    legacy_ledger::replay_accepted(command, replayed, &transitions, quota_limit)
                }
                CompetitionCommand::LowerProxy(lower) => {
                    let transitions =
                        transaction.proxy_transition_history(lower.authorization_id).await?;
                    let quota_limit = transaction.proxy_quota_limit(lot_id).await?;
                    legacy_ledger::replay_accepted(command, replayed, &transitions, quota_limit)
                }
            }
        }
        legacy_ledger::LEGACY_RESULT_CODE => legacy_ledger::replay(command, &stored.result_payload),
        // 旧版把同一请求命中的非成功结果分类为账本冲突；上方仍先核对 command kind、
        // aggregate 与摘要，保留新版更强的防篡改边界。
        _ => Err(crate::ports::core::CommandLedgerError::Conflict.into()),
    }
}
