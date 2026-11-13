//! 偏好 CAS 与原结果重放；仅影响未来通知，不扫描历史队列。

use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, record_error,
};
use crate::ports::core::{RequestHash, StoredCommand, TransactionPolicy, TransactionRunner};
use crate::ports::public::{
    LotWatchError, LotWatchPreference, LotWatchPreferenceTransaction, LotWatchTransaction,
    SetLotWatchPreferenceCommand,
};

/// Serializable 最多三次；先账本重放再比较版本。
pub async fn set_lot_watch_preference<R>(
    runner: &R,
    command: SetLotWatchPreferenceCommand,
) -> Result<LotWatchPreference, LotWatchError>
where
    R: TransactionRunner,
    R::Transaction: LotWatchPreferenceTransaction,
{
    const KIND: &str = "set_lot_watch_preference";
    // 渠道偏好是闭集 none/mm/pm/both；未知取值失败关闭，不静默修正。
    let Some(channels) = auction_domain::LotWatchChannels::preference(&command.channel) else {
        return Err(LotWatchError::Invalid);
    };
    if command.command_id.is_nil() || command.user_id.is_nil() || command.expected_version <= 0 {
        return Err(LotWatchError::Invalid);
    }
    let hash = RequestHash::from_request(KIND, command.user_id, &command).map_err(|error| {
        record_error(
            DiagnosticLevel::Warn,
            error.diagnostic_kind(),
            "lot_watch_preference",
            "hash_request",
            DiagnosticContext::default(),
        );
        LotWatchError::Invalid
    })?;
    runner
        .run(TransactionPolicy::serializable(), |transaction| {
            let command = command.clone();
            Box::pin(async move {
                let current = transaction.lock_preference(command.user_id).await?;
                if let Some(stored) = transaction.find_watch_command(command.command_id).await? {
                    if !stored.matches_request(KIND, command.user_id, hash) {
                        return Err(LotWatchError::Conflict);
                    }
                    let result: LotWatchPreference = serde_json::from_slice(&stored.result_payload)
                        .map_err(|error| {
                            record_error(
                                DiagnosticLevel::Warn,
                                error.diagnostic_kind(),
                                "lot_watch_preference",
                                "decode_replay_result",
                                DiagnosticContext::default(),
                            );
                            LotWatchError::Unavailable
                        })?;
                    if stored.result_code != "ok"
                        || result.user_id != command.user_id
                        || result.channel != command.channel
                        || result.version
                            != command
                                .expected_version
                                .checked_add(1)
                                .ok_or(LotWatchError::Unavailable)?
                    {
                        return Err(LotWatchError::Unavailable);
                    }
                    return Ok(result);
                }
                if current.version != command.expected_version {
                    return Err(LotWatchError::Conflict);
                }
                // 启用 PM（单独或与 MM 同时）但论坛同步未启用或账号未绑定论坛账号时拒绝保存，
                // 避免渠道静默失效；重放已完成命令不受当前可用性影响。
                if channels.pm && !transaction.forum_pm_available(command.user_id).await? {
                    return Err(LotWatchError::PmUnavailable);
                }
                let next = current.version.checked_add(1).ok_or(LotWatchError::Unavailable)?;
                transaction.write_preference(&command, next).await?;
                let result = LotWatchPreference {
                    user_id: command.user_id,
                    channel: command.channel.clone(),
                    version: next,
                };
                transaction
                    .store_watch_command(StoredCommand {
                        command_id: command.command_id,
                        command_kind: KIND.to_owned(),
                        aggregate_id: command.user_id,
                        request_hash: hash,
                        result_code: "ok".to_owned(),
                        result_payload: serde_json::to_vec(&result).map_err(|error| {
                            record_error(
                                DiagnosticLevel::Warn,
                                error.diagnostic_kind(),
                                "lot_watch_preference",
                                "encode_result",
                                DiagnosticContext::default(),
                            );
                            LotWatchError::Unavailable
                        })?,
                    })
                    .await?;
                Ok(result)
            })
        })
        .await
}
