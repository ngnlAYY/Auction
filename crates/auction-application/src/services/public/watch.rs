//! 装备标记命令的验证、摘要、CAS 与持久重放；不访问 SQL 或外部邮件。

use auction_domain::LotWatchChannel;

use crate::diagnostics::{DiagnosticContext, DiagnosticError, DiagnosticLevel, record_error};
use crate::{
    LotWatchError, LotWatchTransaction, RequestHash, SetLotWatchCommand, SetLotWatchResult,
    StoredCommand, TransactionPolicy, TransactionRunner,
};

const COMMAND_KIND: &str = "set_lot_watch";

/// 执行标记命令，重放返回原响应而非当前快照；冲突不会写入任何标记或通知。
pub async fn set_lot_watch<R>(
    runner: &R,
    command: SetLotWatchCommand,
) -> Result<SetLotWatchResult, LotWatchError>
where
    R: TransactionRunner,
    R::Transaction: LotWatchTransaction,
{
    if command.command_id.is_nil()
        || command.user_id.is_nil()
        || command.lot_id.is_nil()
        || command.expected_version < 0
        || LotWatchChannel::available(&command.channel).is_none()
    {
        return Err(LotWatchError::Invalid);
    }
    let hash =
        RequestHash::from_request(COMMAND_KIND, command.lot_id, &command).map_err(|error| {
            record_error(
                DiagnosticLevel::Warn,
                error.diagnostic_kind(),
                "lot_watch",
                "hash_request",
                DiagnosticContext::default(),
            );
            LotWatchError::Invalid
        })?;
    runner
        .run(TransactionPolicy::serializable(), |transaction| {
            let command = command.clone();
            Box::pin(async move {
                let auction_id = transaction.lock_watch_lot(command.lot_id).await?;
                if let Some(stored) = transaction.find_watch_command(command.command_id).await? {
                    if !stored.matches_request(COMMAND_KIND, command.lot_id, hash) {
                        return Err(LotWatchError::Conflict);
                    }
                    let result: SetLotWatchResult = serde_json::from_slice(&stored.result_payload)
                        .map_err(|error| {
                            record_error(
                                DiagnosticLevel::Warn,
                                error.diagnostic_kind(),
                                "lot_watch",
                                "decode_replay_result",
                                DiagnosticContext::default(),
                            );
                            LotWatchError::Unavailable
                        })?;
                    if stored.result_code != "ok"
                        || result.user_id != command.user_id
                        || result.auction_id != auction_id
                        || result.watch.lot_id != command.lot_id
                        || result.watch.version <= 0
                        || result.watch.channel != "mm"
                        || result.watch.watched != command.watched
                    {
                        return Err(LotWatchError::Unavailable);
                    }
                    return Ok(result);
                }
                let watch = transaction.write_watch(&command).await?;
                let result = SetLotWatchResult { user_id: command.user_id, auction_id, watch };
                transaction
                    .store_watch_command(StoredCommand {
                        command_id: command.command_id,
                        command_kind: COMMAND_KIND.to_owned(),
                        aggregate_id: command.lot_id,
                        request_hash: hash,
                        result_code: "ok".to_owned(),
                        result_payload: serde_json::to_vec(&result).map_err(|error| {
                            record_error(
                                DiagnosticLevel::Warn,
                                error.diagnostic_kind(),
                                "lot_watch",
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

/// 为一个已失去的领先周期建立唯一通知身份；正文只含公开 lot UUID 与已接受价格。
pub fn lot_watch_notification(
    lot_id: uuid::Uuid,
    user_id: uuid::Uuid,
    cycle: u64,
    amount: i64,
    world_id: String,
    recipient: String,
) -> Result<crate::MailDeliveryCommand, LotWatchError> {
    use crate::{
        MailDeliveryCommand, MailDeliveryOperation, MailDeliveryOrigin, MailNotificationTemplate,
    };
    let notification_id = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_URL,
        format!("auction/lot-watch-v1/{lot_id}/{user_id}/{cycle}").as_bytes(),
    );
    // 英文正文随领先周期冻结，并继续参与原请求摘要。
    let body = format!(
        "Your leading bid has been outbid.\n\n\
         Accepted bid: {amount} Credits\n\n\
         Check the auction website for the latest price\n\
         and place another bid if you wish to continue.\n\n\
         Lot reference: {lot_id}"
    );
    let hash = RequestHash::from_request(
        "lot_watch_notification",
        notification_id,
        &(lot_id, user_id, cycle, amount, &world_id, &recipient, &body),
    )
    .map_err(|error| {
        record_error(
            DiagnosticLevel::Warn,
            error.diagnostic_kind(),
            "lot_watch_mail",
            "hash_notification",
            DiagnosticContext::default(),
        );
        LotWatchError::Unavailable
    })?;
    Ok(MailDeliveryCommand {
        command_id: auction_domain::CommandId::from_uuid(notification_id),
        request_hash: hash,
        origin: MailDeliveryOrigin::LotWatch,
        origin_id: format!("lot_watch:{notification_id}"),
        operation: MailDeliveryOperation::Send,
        world_id,
        external_mail_id: notification_id.to_string(),
        recipient,
        template: Some(MailNotificationTemplate::LotWatch),
        result_code: Some("outbid".to_owned()),
        user_key_user_id: None,
        credits: 0,
        cod: 0,
        attachment_ids: vec![],
        notification_body: Some(body),
    })
}
