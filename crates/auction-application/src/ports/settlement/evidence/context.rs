//! 持独占会话的 opaque 上下文；提交取消及未知结果永久撤销当前 owner。

use std::time::Instant;

use chrono::Utc;

use super::{InternalEvidenceError, InternalEvidenceFuture, SettlementInternalEvidenceSnapshot};

/// 集成 owner 保留原 session/operation_guard，并提供不可恢复的同步撤销。
/// 只有真实支持撤销和持久 generation 的适配器才能构造内部上下文。
pub trait InternalEvidenceLease: Send + Sync {
    /// 同步 sticky 检查不替代数据库探测，但阻断已知撤销后的新提交。
    fn is_current(&self) -> bool;
    fn check_current(&self, deadline: Instant) -> InternalEvidenceFuture<'_, ()>;
    fn revoke(&self);
}

/// 不可克隆、不可反序列化的准备句柄；所有数据库 future 必须在此句柄作用域内 await。
pub struct PreparedSettlementInternalEvidence {
    snapshot: SettlementInternalEvidenceSnapshot,
    lease: Box<dyn InternalEvidenceLease>,
    deadline: Instant,
    used: bool,
}

impl PreparedSettlementInternalEvidence {
    /// 仅可信集成边界调用；不会从管理员声明或反序列化输入生成证明。
    pub fn new(
        snapshot: SettlementInternalEvidenceSnapshot,
        lease: Box<dyn InternalEvidenceLease>,
        deadline: Instant,
    ) -> Result<Self, InternalEvidenceError> {
        if !lease.is_current() {
            return Err(InternalEvidenceError::Revoked);
        }
        if !snapshot.valid_at(Utc::now()) || deadline <= Instant::now() {
            return Err(InternalEvidenceError::Expired);
        }
        Ok(Self { snapshot, lease, deadline, used: false })
    }

    /// 借用事实；不能从 prepared 中取出可跨会话重用的证明。
    pub fn snapshot(&self) -> &SettlementInternalEvidenceSnapshot {
        &self.snapshot
    }

    /// 原准备操作的单调绝对截止点；交接和事务重试不得延长此预算。
    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    /// 提交前在原会话上重新检查 owner，并保持绝对截止点。
    pub fn check_current(&self) -> InternalEvidenceFuture<'_, ()> {
        Box::pin(async move {
            if self.used {
                return Err(InternalEvidenceError::Revoked);
            }
            if self.deadline <= Instant::now() || !self.snapshot.valid_at(Utc::now()) {
                return Err(InternalEvidenceError::Expired);
            }
            self.lease.check_current(self.deadline).await
        })
    }

    /// 开始可能提交的数据库作用域。必须先 await check_current，再进入统一锁序。
    /// 不得 spawn/detach 数据库 future；取消、错误未知时直接销毁返回 guard。
    pub fn begin_commit(
        &mut self,
    ) -> Result<InternalEvidenceCommitGuard<'_>, InternalEvidenceError> {
        if self.used || !self.lease.is_current() {
            return Err(InternalEvidenceError::Revoked);
        }
        if self.deadline <= Instant::now() || !self.snapshot.valid_at(Utc::now()) {
            return Err(InternalEvidenceError::Expired);
        }
        self.used = true;
        Ok(InternalEvidenceCommitGuard { prepared: self, confirmed: false })
    }
}

/// 未确定结束就 Drop 的事务作用域会使该进程 owner sticky 失效，释放 guard 也不能再外写。
/// 序列化重试可在同一作用域内进行；每次仍须检查证据/claim 实时时效。
pub struct InternalEvidenceCommitGuard<'a> {
    prepared: &'a mut PreparedSettlementInternalEvidence,
    confirmed: bool,
}

impl InternalEvidenceCommitGuard<'_> {
    /// 数据库 future 应借用该快照，禁止脱离 guard 的作用域运行。
    pub fn snapshot(&self) -> &SettlementInternalEvidenceSnapshot {
        &self.prepared.snapshot
    }

    /// 提交必须继续使用准备阶段的单调绝对截止点，不能从当前时间重新分配预算。
    pub fn deadline(&self) -> Instant {
        self.prepared.deadline
    }

    /// 只有已获得数据库确定提交结果才能调用；结果未知须保留稳定命令并查询原结果。
    pub fn confirm_committed(mut self) {
        self.confirmed = true;
    }

    /// 只有已确定未提交/已完成回滚的业务拒绝才能调用，不能把连接错误当作回滚证据。
    pub fn confirm_rolled_back(mut self) {
        self.confirmed = true;
    }
}

impl Drop for InternalEvidenceCommitGuard<'_> {
    fn drop(&mut self) {
        if !self.confirmed {
            self.prepared.lease.revoke();
        }
    }
}
