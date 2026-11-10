//! PM 公开通知内容与缺收件人事实；不包含代理上限、装备密钥或凭据。
use uuid::Uuid;

use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, record_error,
};
use crate::ports::admin::runtime_config::AdminRuntimeMailTemplatesConfig;
use crate::ports::core::RequestHash;
use crate::ports::forum::ForumPmMessage;
use crate::ports::lot_watch_pm::{forum_pm_digest, valid_forum_pm_id};
use crate::ports::public::LotWatchError;
use crate::services::public::OutbidNotice;

/// 冻结来源：缺失身份用 NULL，而非可发送 message 的虚构数字 ID。
pub struct LotWatchPmNotification {
    pub notification_id: Uuid,
    pub recipient: Option<String>,
    pub payload: serde_json::Value,
    pub digest: RequestHash,
}

/// 事件身份与既有 MM 相同，不含渠道；每领先周期跨渠道最多一条来源。
///
/// 标题与正文同样写明 lot 代码、装备名、被超价格、有效截止与网站链接，
/// 正文在创建时按 `lot_watch_pm` 模板渲染后冻结，摘要绑定该冻结正文。
pub fn lot_watch_pm_notification(
    lot_id: Uuid,
    user_id: Uuid,
    cycle: u64,
    amount: i64,
    notice: OutbidNotice<'_>,
    templates: &AdminRuntimeMailTemplatesConfig,
    recipient: Option<String>,
) -> Result<LotWatchPmNotification, LotWatchError> {
    if lot_id.is_nil() || user_id.is_nil() || amount <= 0 {
        return Err(LotWatchError::Invalid);
    }
    if notice.lot_code.is_empty() || notice.item_name.is_empty() {
        return Err(LotWatchError::Invalid);
    }
    let notification_id = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("auction/lot-watch-v1/{lot_id}/{user_id}/{cycle}").as_bytes(),
    );
    let recipient = recipient.filter(|id| valid_forum_pm_id(id));
    let title = format!("Outbid {} - {}", notice.lot_code, notice.item_name);
    let body = notice
        .frozen_body(amount, templates, "lot_watch_pm", notification_id)
        .ok_or(LotWatchError::Invalid)?;
    let payload = serde_json::json!({"notification_id":notification_id,"recipient_forum_account_id":recipient,"title":title,"body":body});
    let digest = if let Some(recipient) = &recipient {
        forum_pm_digest(&ForumPmMessage {
            notification_id,
            recipient_forum_account_id: recipient.clone(),
            title,
            body,
        })?
    } else {
        RequestHash::from_request("lot_watch_pm", notification_id, &payload).map_err(|error| {
            record_error(
                DiagnosticLevel::Warn,
                error.diagnostic_kind(),
                "lot_watch_pm",
                "hash_notification",
                DiagnosticContext::default(),
            );
            LotWatchError::Unavailable
        })?
    };
    Ok(LotWatchPmNotification { notification_id, recipient, payload, digest })
}
