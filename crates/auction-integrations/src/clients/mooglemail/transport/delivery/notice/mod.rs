//! 纯文本通知发信入口：权利通知等零资产邮件不经 delivery outbox，但与 delivery Send 走同一路径——
//! 同一独占凭据会话内先做 compose 干净预检并取得写前 Sent 基线，再写入，并以发后 Sent 差异证明结果。
//!
//! 不变量 4、7：本入口自身不重试、不重发；写入一旦开始而结果无法证明，一律返回
//! [`PlainNoticeOutcome::Unknown`]，由调用方转人工对账。收件人、主题与正文（含挑战码）只进入本次
//! 请求，不进日志与错误；日志只记世界与固定分类码。

use std::fmt;
use std::time::Duration;

use crate::clients::mooglemail::forms::valid_body;
use crate::clients::mooglemail::protocol::valid_line;
use crate::clients::mooglemail::transport::types::MoogleMailDeliveryTransport;
use crate::clients::mooglemail::{
    MoogleMailError, MoogleMailMessageFields, MoogleMailSendOutcome, MoogleMailSendRequest,
    MoogleMailWorld,
};

#[cfg(test)]
mod tests;

/// 写前准备（排队取得独占会话、compose 预检与 Sent 基线）的总预算。超出时尚未发出任何写请求，
/// 按可重试的未发出处理；写入阶段不设外层截止，避免取消把已开始的写入误当成未发送。
const PREPARE_BUDGET: Duration = Duration::from_secs(120);

/// 一封零资产纯文本通知；`Debug` 只输出世界。
pub struct PlainNoticeMail {
    /// 发信世界。
    pub world: MoogleMailWorld,
    /// 收件人：调用方冻结的身份。
    pub recipient: String,
    /// 主题。
    pub subject: String,
    /// 正文；可能含一次性挑战码。
    pub body: String,
}

impl fmt::Debug for PlainNoticeMail {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PlainNoticeMail")
            .field("world", &self.world)
            .finish_non_exhaustive()
    }
}

/// 纯文本通知发信的结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlainNoticeOutcome {
    /// 新增 Sent 邮件已按收件人、主题与正文核实，且 compose 已回到空表单。
    Sent,
    /// 写入已开始但结果无法证明；调用方不得重发。
    Unknown,
    /// 明确没有发出；`retryable` 为真表示写入前的临时阻断，为假表示远端明确拒绝或内容无效。
    NotSent {
        /// 是否可稍后重试。
        retryable: bool,
    },
}

impl MoogleMailDeliveryTransport {
    /// 发送一封零资产纯文本通知，只写一次、不重试。
    ///
    /// 与 delivery 共用凭据集合的独占锁与发送器状态，因此不会与投递、处置或维护的 compose 写入重叠。
    pub async fn send_plain_notice(&self, mail: PlainNoticeMail) -> PlainNoticeOutcome {
        let world = mail.world;
        let request = MoogleMailSendRequest {
            world,
            message: MoogleMailMessageFields {
                recipient: mail.recipient,
                subject: mail.subject,
                body: mail.body,
            },
            credits: 0,
            cod: 0,
            equipment_ids: Vec::new(),
            category: None,
        };
        if !valid_line(&request.message.subject, 512, false)
            || !valid_body(&request.message.body)
            || !request.validate()
        {
            tracing::warn!(
                world = world.as_str(),
                code = "invalid_message",
                "纯文本通知内容无效，不发送"
            );
            return PlainNoticeOutcome::NotSent { retryable: false };
        }
        // 与 delivery 可用性闸门同口径：凭据缺失或账号忙熔断时不排队取锁、不发网络。
        if !self.credentials.available(world) || !self.client.permits_write(world) {
            return PlainNoticeOutcome::NotSent { retryable: true };
        }
        let mut prepared =
            match tokio::time::timeout(PREPARE_BUDGET, self.prepare_credentials(world)).await {
                Ok(Ok(prepared)) => prepared,
                Ok(Err(error)) => {
                    tracing::warn!(
                        world = world.as_str(),
                        code = error.failure_code.as_str(),
                        "纯文本通知写前准备失败，未发出"
                    );
                    return PlainNoticeOutcome::NotSent { retryable: true };
                }
                Err(elapsed) => {
                    tracing::warn!(
                        world = world.as_str(),
                        code = "prepare_timeout",
                        error = %elapsed,
                        "纯文本通知写前准备超时，未发出"
                    );
                    return PlainNoticeOutcome::NotSent { retryable: true };
                }
            };
        let result = self.send_with_credentials(&mut prepared, request).await;
        plain_notice_outcome(world, result)
    }
}

/// 把一次发送的结构化结果收口为三态；只有发后证明成立才算 `Sent`。
fn plain_notice_outcome(
    world: MoogleMailWorld,
    result: Result<MoogleMailSendOutcome, MoogleMailError>,
) -> PlainNoticeOutcome {
    match result {
        Ok(MoogleMailSendOutcome::Sent(_)) => PlainNoticeOutcome::Sent,
        // 零资产邮件：收件人不存在、自寄与邮资不足都是远端明确拒绝，没有资产要取回，不重试。
        Ok(
            MoogleMailSendOutcome::RecipientNotFound { .. }
            | MoogleMailSendOutcome::RecipientIsSender { assets_staged: false }
            | MoogleMailSendOutcome::Rejected { assets_staged: false },
        ) => {
            tracing::warn!(world = world.as_str(), code = "send_rejected", "纯文本通知被远端明确拒绝");
            PlainNoticeOutcome::NotSent { retryable: false }
        }
        // 零资产请求不会暂存资产；一旦报告已暂存，compose 状态无法证明，按未知收口。
        Ok(
            MoogleMailSendOutcome::RecipientIsSender { assets_staged: true }
            | MoogleMailSendOutcome::Rejected { assets_staged: true },
        )
        // 与 delivery 同口径：同一独占会话里预检已证明干净，发送时却无法证明，不能判为未发出。
        | Ok(MoogleMailSendOutcome::PreflightUnproven { .. }) => {
            tracing::warn!(world = world.as_str(), code = "send_unproven", "纯文本通知结果无法证明");
            PlainNoticeOutcome::Unknown
        }
        Err(error) if error.write_started() => {
            tracing::warn!(
                world = world.as_str(),
                code = error.category.as_str(),
                "纯文本通知写入已开始但结果无法证明"
            );
            PlainNoticeOutcome::Unknown
        }
        Err(error) => {
            tracing::warn!(
                world = world.as_str(),
                code = error.category.as_str(),
                "纯文本通知写入前失败，未发出"
            );
            PlainNoticeOutcome::NotSent { retryable: true }
        }
    }
}
