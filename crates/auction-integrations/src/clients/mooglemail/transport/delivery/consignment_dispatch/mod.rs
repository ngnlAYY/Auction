//! 寄售派发发信入口：把一封已按冻结身份渲染并核对过的付款邮件分两步写出——起写前在同一独占凭据
//! 会话里做 compose 干净预检、取得写前 Sent 基线并给出凭据绑定；持久起写拿到一次性许可后，只用这份
//! 会话与基线写一次，并以发后 Sent 差异证明结果。
//!
//! 不变量 4、6、7：本入口不重试、不重发；写入一旦开始而结果无法证明、远端明确拒绝、许可与准备不符，
//! 一律返回 [`ConsignmentDispatchOutcome::Unknown`]（持久起写已提交，结论交人工核对，绝不再写）。
//! 冻结 Credits 大于零的现金轮（载体 1、卖款 S、退款 P）另在同一独占会话里取新鲜资金证据（见 `cash`）：
//! 认证 UID 必须等于成交冻结的资金 UID，取不到、不新鲜或 UID 不符都在准备阶段失败关闭、零 POST；证据随
//! 准备交给起写事务，由资金 owner 按通用现金门核对余额。Credits 与 COD、附件一样进入请求摘要，并由发后
//! Sent 差异逐项核实。收件人、主题与正文只进入本次请求，不进日志与错误；日志只记世界与固定分类码。

use std::fmt;
use std::time::Duration;

use auction_application::consignment::PostPermit;
use auction_application::delivery::DeliverySendCredentialBinding;
use auction_application::settlement::SettlementInternalEvidenceSnapshot;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::credentials::PreparedMoogleMailCredentials;
use crate::clients::mooglemail::forms::valid_body;
use crate::clients::mooglemail::parser::canonical_mail_body_digest;
use crate::clients::mooglemail::protocol::{valid_decimal_id, valid_line};
use crate::clients::mooglemail::transport::types::MoogleMailDeliveryTransport;
use crate::clients::mooglemail::{
    MoogleMailMessageFields, MoogleMailSendOutcome, MoogleMailSendRequest, MoogleMailWorld,
};

mod cash;
mod collected;
mod recheck;
#[cfg(test)]
mod tests;

pub use collected::{
    CONSIGNMENT_DISPATCH_COLLECTED_VALIDATOR_VERSION, ConsignmentCollection,
    ConsignmentCollectionTarget,
};
pub use recheck::{ConsignmentDispatchRecheck, ConsignmentDispatchRecheckTarget};

/// 寄售派发 Sent 证明的验证器版本：发后 Sent 差异按收件人、主题、正文、Credits、COD 与附件逐项核实。
/// v2 起证据摘要包含 Credits（v1 只核对 COD 与附件，从未发出过现金轮）。
pub const CONSIGNMENT_DISPATCH_SENT_VALIDATOR_VERSION: &str = "consignment_dispatch_sent_v2";

/// 写前准备（排队取得独占会话、compose 预检与 Sent 基线）的总预算；超出时尚未写入，按不可发处理。
const PREPARE_BUDGET: Duration = Duration::from_secs(120);

/// 一封待发的寄售付款邮件；`Debug` 只输出世界、投递与 fence。
pub struct ConsignmentDispatchMail {
    /// 发信世界。
    pub world: MoogleMailWorld,
    /// 轮投递（许可必须绑定同一投递）。
    pub delivery_id: Uuid,
    /// 起写 fence（许可必须绑定同一 fence）。
    pub fence: u64,
    /// 收件人：投递冻结的身份。
    pub recipient: String,
    /// 冻结主题。
    pub subject: String,
    /// 按冻结身份重渲染、已核对摘要的正文。
    pub body: String,
    /// 冻结 Credits（载体 1、卖款 S、退款 P；原件轮为 0）。
    pub credits: i64,
    /// 冻结 COD。
    pub cod: i64,
    /// 冻结的标的装备编号（规范正十进制 EID）。
    pub equipment_ids: Vec<String>,
    /// 投递当前租约 token；现金轮的资金证据绑定它。
    pub claim_token: Uuid,
    /// 投递冻结身份摘要（即投递的请求摘要）；现金轮的资金证据绑定它。
    pub identity_digest: [u8; 32],
    /// 成交冻结的资金 UID；现金轮必须给出，发信会话的认证 UID 必须等于它。
    pub funding_uid: Option<String>,
    /// 现金轮是否观察 main 现金余额：严格现金为真，记账模式为假（与通用现金门同口径）。
    pub observe_main_cash: bool,
}

impl fmt::Debug for ConsignmentDispatchMail {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConsignmentDispatchMail")
            .field("world", &self.world)
            .field("delivery_id", &self.delivery_id)
            .field("fence", &self.fence)
            .finish_non_exhaustive()
    }
}

/// 已取得独占会话与写前基线、尚未写入的派发；只能被 [`MoogleMailDeliveryTransport::execute_consignment_dispatch`]
/// 消费一次，丢弃即释放会话、不写任何东西。
pub struct PreparedConsignmentDispatch {
    delivery_id: Uuid,
    fence: u64,
    request: MoogleMailSendRequest,
    credentials: PreparedMoogleMailCredentials,
    credential: Option<DeliverySendCredentialBinding>,
    cash_evidence: Option<SettlementInternalEvidenceSnapshot>,
}

impl PreparedConsignmentDispatch {
    /// 本会话 active 凭据的持久绑定；配置 Cookie 没有持久身份时为 `None`，调用方不得据此起写。
    pub fn credential(&self) -> Option<DeliverySendCredentialBinding> {
        self.credential.clone()
    }

    /// 取出现金轮在本会话里取得的资金证据，交起写事务核对；非现金轮为 `None`。会话本身仍留在本句柄里，
    /// 直到写入结束才释放，证据观察与发信是同一会话。
    pub fn take_cash_evidence(&mut self) -> Option<SettlementInternalEvidenceSnapshot> {
        self.cash_evidence.take()
    }
}

impl fmt::Debug for PreparedConsignmentDispatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedConsignmentDispatch")
            .field("world", &self.request.world)
            .field("delivery_id", &self.delivery_id)
            .field("fence", &self.fence)
            .finish_non_exhaustive()
    }
}

/// 发后 Sent 证明成立的那封邮件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsignmentDispatchSent {
    /// 发后 Sent 差异核实的远端邮件 ID。
    pub sent_mid: String,
    /// 产出证明的验证器版本。
    pub validator_version: &'static str,
    /// 证据摘要：版本前缀、世界、Sent MID 与实际请求的安全投影。
    pub evidence_digest: [u8; 32],
}

/// 持许可写一次的结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsignmentDispatchOutcome {
    /// 新增 Sent 邮件已按实际请求核实。
    Proven(ConsignmentDispatchSent),
    /// 结果无法证明（含写前失败、远端拒绝与许可不符）：调用方不得重发。
    Unknown,
}

impl MoogleMailDeliveryTransport {
    /// 起写前只读准备：校验内容、取得独占会话、compose 干净预检与写前 Sent 基线。不写远端；
    /// 内容无效、凭据不可用、账号忙或准备失败 / 超时返回 `None`。
    pub async fn prepare_consignment_dispatch(
        &self,
        mail: ConsignmentDispatchMail,
    ) -> Option<PreparedConsignmentDispatch> {
        let world = mail.world;
        let cash = cash::CashBinding::from_mail(&mail);
        let mut equipment_ids = mail.equipment_ids;
        equipment_ids.sort_unstable();
        let request = MoogleMailSendRequest {
            world,
            message: MoogleMailMessageFields {
                recipient: mail.recipient,
                subject: mail.subject,
                body: mail.body,
            },
            credits: mail.credits,
            cod: mail.cod,
            equipment_ids,
            category: None,
        };
        // 现金轮必须带齐资金绑定（冻结资金 UID、租约 token），缺一即内容无效、不发。
        if mail.delivery_id.is_nil()
            || mail.fence == 0
            || (request.credits > 0 && cash.is_none())
            || !valid_line(&request.message.subject, 512, false)
            || !valid_body(&request.message.body)
            || !request.equipment_ids.iter().all(|id| valid_decimal_id(id))
            || !request.validate()
        {
            tracing::warn!(
                world = world.as_str(),
                code = "invalid_message",
                "寄售派发内容无效，不发送"
            );
            return None;
        }
        // 与 delivery 可用性闸门同口径：凭据缺失或账号忙熔断时不排队取锁、不发网络。
        if !self.credentials.available(world) || !self.client.permits_write(world) {
            return None;
        }
        let credentials =
            match tokio::time::timeout(PREPARE_BUDGET, self.prepare_credentials(world)).await {
                Ok(Ok(credentials)) => credentials,
                Ok(Err(error)) => {
                    tracing::warn!(
                        world = world.as_str(),
                        code = error.failure_code.as_str(),
                        "寄售派发写前准备失败，未发出"
                    );
                    return None;
                }
                Err(elapsed) => {
                    tracing::warn!(
                        world = world.as_str(),
                        code = "prepare_timeout",
                        error = %elapsed,
                        "寄售派发写前准备超时，未发出"
                    );
                    return None;
                }
            };
        // 配置 Cookie 没有持久凭据身份（`Unsupported`）时绑定为空；起写锁验要求 active 凭据，调用方
        // 见到空绑定即不起写。其余绑定失败同样按空处理，不在此猜测。
        let credential = credentials.session.internal_credential_binding(world).ok().map(
            |(credential_id, credential_version, runtime, _)| DeliverySendCredentialBinding {
                credential_id,
                credential_version,
                runtime_generation: runtime.generation,
            },
        );
        // 现金轮：同一独占会话里取新鲜资金证据并核对认证 UID；任何不满足都在写前失败关闭，丢弃即释放会话。
        let cash_evidence = match cash {
            Some(cash) => Some(self.consignment_cash_evidence(&credentials.session, &cash).await?),
            None => None,
        };
        Some(PreparedConsignmentDispatch {
            delivery_id: mail.delivery_id,
            fence: mail.fence,
            request,
            credentials,
            credential,
            cash_evidence,
        })
    }

    /// 以一次性许可写一次并读发后 Sent 证明；许可以值移入，结论只有“已证明”与“未知”。
    pub async fn execute_consignment_dispatch(
        &self,
        prepared: PreparedConsignmentDispatch,
        permit: PostPermit,
    ) -> ConsignmentDispatchOutcome {
        let PreparedConsignmentDispatch { delivery_id, fence, request, mut credentials, .. } =
            prepared;
        let world = request.world;
        // 许可只对准备时的同一投递与 fence 有效；不符时不写，但持久起写已提交，只能交人工核对。
        if permit.delivery_id() != delivery_id || permit.fence() != fence {
            tracing::warn!(
                world = world.as_str(),
                code = "permit_mismatch",
                "寄售派发许可与准备不符，不写入"
            );
            return ConsignmentDispatchOutcome::Unknown;
        }
        let digest = request_digest(&request);
        match self.send_with_credentials(&mut credentials, request).await {
            Ok(MoogleMailSendOutcome::Sent(receipt))
                if valid_decimal_id(&receipt.external_mail_id) =>
            {
                let evidence_digest = evidence_digest(world, &receipt.external_mail_id, &digest);
                ConsignmentDispatchOutcome::Proven(ConsignmentDispatchSent {
                    sent_mid: receipt.external_mail_id,
                    validator_version: CONSIGNMENT_DISPATCH_SENT_VALIDATOR_VERSION,
                    evidence_digest,
                })
            }
            Ok(_) => {
                tracing::warn!(
                    world = world.as_str(),
                    code = "send_unproven",
                    "寄售派发结果无法证明"
                );
                ConsignmentDispatchOutcome::Unknown
            }
            Err(error) => {
                tracing::warn!(
                    world = world.as_str(),
                    code = error.category.as_str(),
                    write_started = error.write_started(),
                    "寄售派发写入失败或结果无法证明"
                );
                ConsignmentDispatchOutcome::Unknown
            }
        }
    }
}

/// 实际请求的安全投影摘要：收件人、主题、正文摘要、Credits、COD 与排序后的附件。
fn request_digest(request: &MoogleMailSendRequest) -> [u8; 32] {
    sent_request_digest(&SentRequestParts {
        recipient: &request.message.recipient,
        subject: &request.message.subject,
        body_digest: canonical_mail_body_digest(&request.message.body),
        credits: request.credits,
        cod: request.cod,
        equipment_ids: &request.equipment_ids,
    })
}

/// 发送请求的安全投影；发信与复核由同一投影得出同一证据摘要。
struct SentRequestParts<'a> {
    recipient: &'a str,
    subject: &'a str,
    body_digest: [u8; 32],
    credits: i64,
    cod: i64,
    /// 已按文本升序排列。
    equipment_ids: &'a [String],
}

fn sent_request_digest(parts: &SentRequestParts<'_>) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(parts.recipient.as_bytes());
    hasher.update([0]);
    hasher.update(parts.subject.as_bytes());
    hasher.update([0]);
    hasher.update(parts.body_digest);
    hasher.update(parts.credits.to_be_bytes());
    hasher.update(parts.cod.to_be_bytes());
    for equipment_id in parts.equipment_ids {
        hasher.update([0]);
        hasher.update(equipment_id.as_bytes());
    }
    hasher.finalize().into()
}

/// 证据摘要：同一 Sent MID 与同一实际请求重放得到同一值（证据写口据此区分重放与冲突）。
fn evidence_digest(world: MoogleMailWorld, sent_mid: &str, request_digest: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"auction.consignment.dispatch.sent.v2\0");
    hasher.update(world.as_str().as_bytes());
    hasher.update([0]);
    hasher.update(sent_mid.as_bytes());
    hasher.update([0]);
    hasher.update(request_digest);
    hasher.finalize().into()
}
