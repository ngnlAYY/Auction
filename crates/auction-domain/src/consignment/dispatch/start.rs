//! 派发起写：同一投递至多一次成功起写，且只有当前领取者可得首次许可。

use uuid::Uuid;

/// 锁内读到的投递当前领取事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DispatchClaimFacts {
    /// 当前 fence。
    pub current_fence: u64,
    /// 当前 claim token。
    pub current_claim_token: Uuid,
    /// 锁后数据库时刻下租约仍有效。
    pub lease_live: bool,
}

/// 起写判定的锁内输入。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteStartFacts {
    /// 该投递已有成功起写。
    pub already_started: bool,
    /// 当前领取。
    pub claim: DispatchClaimFacts,
    /// 请求的 fence。
    pub request_fence: u64,
    /// 请求的 claim token。
    pub request_claim_token: Uuid,
}

/// 确定拒绝的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteStartRejection {
    /// fence 或 token 已不是当前领取。
    FenceMismatch,
    /// 租约已过。
    LeaseLost,
}

/// 起写判定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteStartDecision {
    /// 本次是唯一胜出者：插入起写事实，且只有它可 POST。
    First,
    /// 已有成功起写：只核对原 attempt，不得再发 POST。
    Already,
    /// 确定拒绝。
    Reject(WriteStartRejection),
}

/// 判定派发能否起写。
///
/// 保护不变量 4、5：每个投递最多一条成功起写；已有起写时任何领取都只得到“已提交”，
/// 首次许可必须来自当前 fence、当前 claim token 与锁后时钟下仍有效的租约。
pub fn decide_write_start(facts: &WriteStartFacts) -> WriteStartDecision {
    if facts.already_started {
        return WriteStartDecision::Already;
    }
    let claim = &facts.claim;
    if claim.current_fence != facts.request_fence
        || claim.current_claim_token != facts.request_claim_token
    {
        return WriteStartDecision::Reject(WriteStartRejection::FenceMismatch);
    }
    if !claim.lease_live {
        return WriteStartDecision::Reject(WriteStartRejection::LeaseLost);
    }
    WriteStartDecision::First
}
