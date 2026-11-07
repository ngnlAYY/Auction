//! 按 HV 认证 UID 读取论坛目标资料；读取者账号不构成目标身份证明。

use std::time::Instant;

use chrono::{DateTime, Utc};

use crate::ports::settlement::InternalEvidenceFuture;

/// 固定资料结构得到的完整显示名与精确目标 UID，不含任何论坛凭据。
#[derive(Debug)]
pub struct ForumTargetProfileEvidence {
    pub target_uid: String,
    pub player_name: String,
    pub observed_at: DateTime<Utc>,
    pub protocol_version: &'static str,
    pub digest: [u8; 32],
}

/// 只接受 UID 和绝对预算，不接受 raw URL 或预期名称，不创建 PM/发帖任务。
pub trait ForumTargetProfileReader: Send + Sync {
    fn read_target_profile(
        &self,
        target_uid: String,
        deadline: Instant,
    ) -> InternalEvidenceFuture<'_, ForumTargetProfileEvidence>;
}
