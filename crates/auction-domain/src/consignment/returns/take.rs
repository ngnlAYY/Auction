//! Take 起写：命令 ID 派生与“同一 intake / work 至多一次成功起写”的判定。

use uuid::Uuid;

use crate::CommandId;

/// v2 命令统一前缀（末尾一个 NUL 字节）；与应用层 `COMMAND_PREFIX_V2` 逐字节一致。
pub const COMMAND_PREFIX_V2: &[u8] = b"auction.consignment.command.v2\0";

const START_RETURN_TAKE_OPERATION: &str = "start_return_take";

/// 起写命令 ID：`UUIDv5(NAMESPACE_URL, 前缀 ‖ text("start_return_take") ‖ intake ‖ work ‖ attempt ‖ fence)`。
///
/// 过租或旧 fence 的确定拒绝只占该次尝试的 ID，不影响新尝试；字节冻结。
pub fn start_return_take_command_id(
    intake: Uuid,
    work: Uuid,
    attempt: Uuid,
    fence: u64,
) -> CommandId {
    let mut input = COMMAND_PREFIX_V2.to_vec();
    input.extend_from_slice(&(START_RETURN_TAKE_OPERATION.len() as u64).to_be_bytes());
    input.extend_from_slice(START_RETURN_TAKE_OPERATION.as_bytes());
    input.extend_from_slice(intake.as_bytes());
    input.extend_from_slice(work.as_bytes());
    input.extend_from_slice(attempt.as_bytes());
    input.extend_from_slice(&fence.to_be_bytes());
    CommandId::from_uuid(Uuid::new_v5(&Uuid::NAMESPACE_URL, &input))
}

/// 已提交的成功起写，复合绑定 intake、work 与胜出的 attempt / fence。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartedRef {
    /// 胜出的 attempt。
    pub attempt_id: Uuid,
    /// 胜出的 fence。
    pub fence: u64,
}

/// 起写请求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TakeStartRequest {
    /// 本次 attempt。
    pub attempt_id: Uuid,
    /// 本次 fence。
    pub fence: u64,
    /// 本次 claim token。
    pub claim_token: Uuid,
    /// 证据会话绑定摘要。
    pub session_binding_digest: [u8; 32],
    /// 写前冻结清单摘要。
    pub expected_manifest_digest: [u8; 32],
}

/// 锁内读到的当前领取事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaimFacts {
    /// 当前 fence。
    pub current_fence: u64,
    /// 当前 attempt。
    pub current_attempt_id: Uuid,
    /// 当前 claim token。
    pub current_claim_token: Uuid,
    /// 锁后数据库时刻下租约仍有效。
    pub lease_live: bool,
    /// 授权会话的绑定摘要。
    pub session_binding_digest: [u8; 32],
    /// 冻结的退件清单摘要。
    pub frozen_manifest_digest: [u8; 32],
}

/// 起写判定的锁内输入。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TakeStartFacts {
    /// 接管仍开放（未消费、未关闭）。
    pub intake_open: bool,
    /// 同一 intake 已有的成功起写。
    pub intake_start: Option<StartedRef>,
    /// 同一 work 已有的成功起写。
    pub work_start: Option<StartedRef>,
    /// 当前存在相反的资产动作。
    pub opposite_action_present: bool,
    /// 当前领取。
    pub claim: ClaimFacts,
    /// 请求。
    pub request: TakeStartRequest,
}

/// 确定拒绝的原因；只占该次尝试的命令 ID。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TakeStartRejection {
    /// intake 与 work 的起写事实不一致（矛盾事实，失败关闭）。
    StartedInconsistent,
    /// 接管已关闭。
    IntakeNotOpen,
    /// 存在相反动作。
    OppositeAction,
    /// fence、attempt 或 token 已不是当前领取。
    FenceMismatch,
    /// 租约已过。
    LeaseLost,
    /// 会话绑定不符。
    SessionMismatch,
    /// 清单与冻结值不符。
    ManifestMismatch,
}

/// 起写判定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TakeStartDecision {
    /// 本次是唯一胜出者：插入起写事实，且只有它可 POST。
    First,
    /// 已有成功起写：只核对原 attempt，不得再发 POST（新 fence、新命令 ID 皆同）。
    Already(StartedRef),
    /// 确定拒绝。
    Reject(TakeStartRejection),
}

/// 判定能否起写。
///
/// 保护不变量 4、5：同一 intake 及其唯一 work 各自最多一条成功起写；已有起写时任何新
/// 领取都得不到许可，且 intake 与 work 的起写必须是同一条。
pub fn decide_take_start(facts: &TakeStartFacts) -> TakeStartDecision {
    match (facts.intake_start, facts.work_start) {
        (Some(intake), Some(work)) if intake == work => return TakeStartDecision::Already(intake),
        (None, None) => {}
        _ => return TakeStartDecision::Reject(TakeStartRejection::StartedInconsistent),
    }
    if !facts.intake_open {
        return TakeStartDecision::Reject(TakeStartRejection::IntakeNotOpen);
    }
    if facts.opposite_action_present {
        return TakeStartDecision::Reject(TakeStartRejection::OppositeAction);
    }
    let (claim, request) = (&facts.claim, &facts.request);
    if claim.current_fence != request.fence
        || claim.current_attempt_id != request.attempt_id
        || claim.current_claim_token != request.claim_token
    {
        return TakeStartDecision::Reject(TakeStartRejection::FenceMismatch);
    }
    if !claim.lease_live {
        return TakeStartDecision::Reject(TakeStartRejection::LeaseLost);
    }
    if claim.session_binding_digest != request.session_binding_digest {
        return TakeStartDecision::Reject(TakeStartRejection::SessionMismatch);
    }
    if claim.frozen_manifest_digest != request.expected_manifest_digest {
        return TakeStartDecision::Reject(TakeStartRejection::ManifestMismatch);
    }
    TakeStartDecision::First
}
