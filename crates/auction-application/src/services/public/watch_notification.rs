//! PM 公开通知内容与缺收件人事实；不包含代理上限、装备密钥或凭据。
use uuid::Uuid;

use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, record_error,
};
use crate::ports::admin::runtime_config::AdminRuntimeMailTemplatesConfig;
use crate::ports::core::RequestHash;
use crate::ports::forum::ForumPmMessage;
use crate::ports::lot_watch::pm::{forum_pm_digest, valid_forum_pm_id};
use crate::ports::public::LotWatchError;
use crate::services::public::OutbidNotice;

/// 冻结来源：缺失身份用 NULL，而非可发送 message 的虚构数字 ID。
pub struct LotWatchPmNotification {
    pub notification_id: Uuid,
    pub recipient: Option<String>,
    pub payload: serde_json::Value,
    pub digest: RequestHash,
}

/// 每个领先周期在 PM 渠道最多一条来源。身份带渠道前缀，与同周期的 MM 提醒互不相同：
/// 账号同时启用两个渠道时，两条来源各自拥有通知 ID、摘要与投递状态。
///
/// 标题与正文同样写明 lot 代码、装备名、被超价格、有效截止与网站链接，
/// 正文在创建时按 `lot_watch_pm` 模板渲染后冻结，摘要绑定该冻结正文。
///
/// 论坛私信的线格式比投递正文更严；模板或装备名不合规时回落到默认正文与不含装备名的
/// 标题，仍不合规则抑制本次渠道，绝不让通知内容阻断竞价事务。
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
        format!("auction/lot-watch-pm-v1/{lot_id}/{user_id}/{cycle}").as_bytes(),
    );
    let mut recipient = recipient.filter(|id| valid_forum_pm_id(id));
    let mut title = format!("Outbid {} - {}", notice.lot_code, notice.item_name);
    let mut body = notice
        .frozen_body(lot_id, amount, templates, "lot_watch_pm", notification_id)
        .ok_or(LotWatchError::Invalid)?;
    // 私信正文只允许可打印 ASCII 且不含方括号等线格式字符；模板或装备名不合规时先回落到
    // 不含装备名的标题与默认正文，仍不合规则抑制本次渠道，而不是让竞价事务失败。
    let mut digest = None;
    if let Some(recipient_id) = recipient.as_deref() {
        digest = pm_digest(notification_id, recipient_id, &title, &body);
        if digest.is_none() {
            title = format!("Outbid {}", notice.lot_code);
            digest = pm_digest(notification_id, recipient_id, &title, &body);
        }
        if digest.is_none()
            && let Some(default_body) = notice.frozen_body(
                lot_id,
                amount,
                &AdminRuntimeMailTemplatesConfig::default(),
                "lot_watch_pm",
                notification_id,
            )
        {
            body = default_body;
            digest = pm_digest(notification_id, recipient_id, &title, &body);
        }
    }
    // 无收件人本来就抑制；有收件人但内容无法构成可发送私信时同样抑制并保留冻结载荷。
    if digest.is_none() {
        recipient = None;
    }
    let payload = serde_json::json!({"notification_id":notification_id,"recipient_forum_account_id":recipient,"title":title,"body":body});
    let digest = match digest {
        Some(digest) => digest,
        None => payload_digest(notification_id, &payload)?,
    };
    Ok(LotWatchPmNotification { notification_id, recipient, payload, digest })
}

/// 尝试用给定标题与正文构造可发送的私信摘要；不合论坛私信线格式时返回 `None`。
fn pm_digest(
    notification_id: Uuid,
    recipient: &str,
    title: &str,
    body: &str,
) -> Option<RequestHash> {
    forum_pm_digest(&ForumPmMessage {
        notification_id,
        recipient_forum_account_id: recipient.to_owned(),
        title: title.to_owned(),
        body: body.to_owned(),
    })
    .ok()
}

/// 无收件人时按冻结载荷计算摘要；失败只记录诊断，不改变已有的通知身份。
fn payload_digest(
    notification_id: Uuid,
    payload: &serde_json::Value,
) -> Result<RequestHash, LotWatchError> {
    RequestHash::from_request("lot_watch_pm", notification_id, payload).map_err(|error| {
        record_error(
            DiagnosticLevel::Warn,
            error.diagnostic_kind(),
            "lot_watch_pm",
            "hash_notification",
            DiagnosticContext::default(),
        );
        LotWatchError::Unavailable
    })
}
