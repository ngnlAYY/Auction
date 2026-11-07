//! 装备标记命令的验证、摘要、CAS 与持久重放；不访问 SQL 或外部邮件。

use auction_domain::LotWatchChannel;

use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, record_error,
};
use crate::ports::admin::runtime_config::AdminRuntimeMailTemplatesConfig;
use crate::ports::core::{RequestHash, StoredCommand, TransactionPolicy, TransactionRunner};
use crate::ports::public::{
    LotWatchError, LotWatchTransaction, SetLotWatchCommand, SetLotWatchResult,
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

/// 被超提醒的公开上下文（W14.2）：lot 代码、装备名、有效截止与网站链接。
///
/// 所有字段都来自同一事务内的权威快照，正文只使用这些公开信息；
/// 不含代理上限、装备 key 或任何凭据。
#[derive(Debug, Clone, Copy)]
pub struct OutbidNotice<'a> {
    pub lot_code: &'a str,
    pub item_name: &'a str,
    /// 当前有效截止（含软截止延长）；尚未进入在拍状态时为空。
    pub deadline: Option<chrono::DateTime<chrono::Utc>>,
    /// 公开站点 origin；未配置时省略链接而不伪造地址。
    pub site_url: Option<&'a str>,
}

impl OutbidNotice<'_> {
    fn lot_url(&self) -> Option<String> {
        let site = self.site_url?;
        Some(format!("{}/lots/{}", site.trim_end_matches('/'), self.lot_code))
    }

    /// 模板变量：`{deadline_line}`、`{lot_url_line}` 是带标签的整行（含换行），缺失时为空串。
    /// 渲染结果必须满足投递正文上限，避免创建出永远无法发送的载荷。
    fn render_body(
        &self,
        amount: i64,
        templates: &AdminRuntimeMailTemplatesConfig,
        key: &str,
        notification_id: uuid::Uuid,
    ) -> Option<String> {
        let amount = crate::services::public::format_credits(amount);
        let deadline_line = self
            .deadline
            .map(|deadline| {
                format!("Bidding closes: {}\n", crate::services::public::format_deadline(deadline))
            })
            .unwrap_or_default();
        let lot_url_line =
            self.lot_url().map(|url| format!("View the lot: {url}\n")).unwrap_or_default();
        let notification_id = notification_id.to_string();
        let variables = [
            ("item_name", self.item_name),
            ("lot_code", self.lot_code),
            ("amount", amount.as_str()),
            ("deadline_line", deadline_line.as_str()),
            ("lot_url_line", lot_url_line.as_str()),
            ("notification_id", notification_id.as_str()),
        ];
        let body = templates.render(key, &variables)?;
        crate::ports::mail::valid_notification_body(Some(&body)).then_some(body)
    }

    /// 自定义模板不可用（缺键、缺变量或展开越界）时回落到默认正文，避免模板问题阻断竞价事务。
    pub(super) fn frozen_body(
        &self,
        amount: i64,
        templates: &AdminRuntimeMailTemplatesConfig,
        key: &str,
        notification_id: uuid::Uuid,
    ) -> Option<String> {
        self.render_body(amount, templates, key, notification_id).or_else(|| {
            self.render_body(
                amount,
                &AdminRuntimeMailTemplatesConfig::default(),
                key,
                notification_id,
            )
        })
    }
}

/// 为一个已失去的领先周期建立唯一通知身份；正文在创建时按模板渲染并冻结进载荷。
// 参数逐项承载稳定身份、冻结渠道与模板快照；拆成临时结构会掩盖同事务冻结边界。
#[allow(clippy::too_many_arguments)]
pub fn lot_watch_notification(
    lot_id: uuid::Uuid,
    user_id: uuid::Uuid,
    cycle: u64,
    amount: i64,
    notice: OutbidNotice<'_>,
    templates: &AdminRuntimeMailTemplatesConfig,
    world_id: String,
    recipient: String,
) -> Result<crate::ports::mail::MailDeliveryCommand, LotWatchError> {
    use crate::ports::mail::{
        MailDeliveryCommand, MailDeliveryOperation, MailDeliveryOrigin, MailNotificationTemplate,
    };
    if notice.lot_code.is_empty() || notice.item_name.is_empty() {
        return Err(LotWatchError::Invalid);
    }
    let notification_id = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_URL,
        format!("auction/lot-watch-v1/{lot_id}/{user_id}/{cycle}").as_bytes(),
    );
    // 英文正文随领先周期冻结，并继续参与原请求摘要；已创建的投递不会因模板更新而改变。
    let body = notice
        .frozen_body(amount, templates, "lot_watch", notification_id)
        .ok_or(LotWatchError::Invalid)?;
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
