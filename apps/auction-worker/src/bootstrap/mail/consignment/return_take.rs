//! 退件 Take 的外写传输适配：起写前重读当前领取并在同一认证会话里重读原信清单，复用期 4 的处置
//! 传输；拿到一次性许可后写一次 Take 并带回精确证明。
//!
//! 保护不变量 4、5、6：
//! - 领取已失效（租约过期、fence / token / attempt 不是当前值）、领取形状不合法、处置传输写前
//!   不可用，或重读到的清单摘要不等于冻结清单时，`prepare` 返回 `None`：不起写、零 Take。
//! - `execute` 只有“已证明”与“未知”；未知交人工核对，绝不再 Take。本适配器从不返回错误。
//! - 不按 RTS 主题比对：Take 工作的持久主题是固定占位，只认工作、投递、attempt、fence 与冻结清单。

use std::sync::Arc;
use std::time::{Duration, Instant};

use auction_application::consignment::{ConsignmentFuture, PostPermit};
use auction_application::mail::{
    ConsignmentDispositionRequest, ConsignmentDispositionTransport, PreparedDisposition,
    ReturnTakeClaim, ReturnTakeLease, ReturnTakeWorkStore,
};
use auction_jobs::consignment::{ClaimedTake, PreparedTake, ReturnTakeTransport, TakeExecution};

use super::transport::MoogleMailConsignmentTransport;

/// 复用期 4 处置传输的退件 Take 传输。
pub struct MoogleMailReturnTakeTransport {
    /// 当前领取重读端口。
    pub store: Arc<dyn ReturnTakeWorkStore>,
    /// 与期 4 处置维护同一组 gateway 的处置传输。
    pub disposition: Arc<MoogleMailConsignmentTransport>,
    /// 写前准备与执行的外部截止（与领取租约同长）。
    pub deadline: Duration,
}

impl ReturnTakeTransport for MoogleMailReturnTakeTransport {
    fn prepare<'a>(&'a self, take: &'a ClaimedTake) -> ConsignmentFuture<'a, Option<PreparedTake>> {
        Box::pin(async move {
            let lease = ReturnTakeLease {
                intake_id: take.intake_id,
                attempt_id: take.attempt_id,
                fence: take.fence,
                claim_token: take.claim_token,
            };
            let Some(claim) = self.store.load_current_return_take(lease).await? else {
                return Ok(None);
            };
            if !matches_take(&claim, take) {
                tracing::warn!(
                    code = "return_take_claim_mismatch",
                    "退件 Take 当前领取与工作不符，不起写"
                );
                return Ok(None);
            }
            let Some(deadline) = Instant::now().checked_add(self.deadline) else {
                return Ok(None);
            };
            let expected = claim.work.manifest.manifest_digest;
            let request = ConsignmentDispositionRequest {
                action: claim.work.action,
                final_decision_id: claim.work.final_decision_id,
                work_id: claim.work.work_id,
                delivery_id: claim.work.delivery_id,
                attempt_id: claim.work.attempt_id,
                fence: claim.work.fence,
                manifest: claim.work.manifest,
            };
            let prepared = match self.disposition.prepare(request, deadline).await? {
                Ok(prepared) => prepared,
                Err(not_sent) => {
                    tracing::warn!(reason = ?not_sent.reason, "退件 Take 写前准备未成，不起写");
                    return Ok(None);
                }
            };
            // 同一会话重读到的清单必须就是冻结清单，否则原信已变，不 Take（交恢复路径核对）。
            if prepared.observed_manifest_digest() != expected {
                tracing::warn!(code = "manifest_changed", "退件原信清单已变，不起写");
                return Ok(None);
            }
            Ok(Some(PreparedTake {
                session_binding_digest: prepared.session_binding_digest(),
                expected_manifest_digest: expected,
                handle: Box::new(prepared),
            }))
        })
    }

    fn execute(
        &self,
        prepared: PreparedTake,
        permit: PostPermit,
    ) -> ConsignmentFuture<'_, TakeExecution> {
        Box::pin(async move {
            // 只有本适配器构造的句柄能回到这里；类型不符时许可随之丢弃，不 Take、按未知交人工核对。
            let Ok(prepared) = prepared.handle.downcast::<Box<dyn PreparedDisposition>>() else {
                tracing::warn!(code = "return_take_handle_invalid", "退件 Take 句柄不符，不执行");
                return Ok(TakeExecution::Unknown);
            };
            Ok(self.disposition.execute_take(*prepared, permit).await)
        })
    }
}

/// 重读的当前领取必须仍是 executor 手里的那一项 Take 工作。
fn matches_take(claim: &ReturnTakeClaim, take: &ClaimedTake) -> bool {
    claim.validate()
        && claim.intake_id == take.intake_id
        && claim.work.work_id == take.work_id
        && claim.work.delivery_id == take.delivery_id
        && claim.work.attempt_id == take.attempt_id
        && claim.work.fence == take.fence
        && claim.work.claim_token == take.claim_token
}
