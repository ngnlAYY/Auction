//! Worker 冷配置摘要及 Admin 读取所需的窄端口。

use serde::{Deserialize, Serialize};

use crate::ports::core::{ApplicationFuture, RequestHash, lower_hex};

/// Worker 心跳详情中独立于动态运行能力的启动摘要键。
pub const WORKER_COLD_CONFIG_HEARTBEAT_KEY: &str = "worker_cold_config";

/// 已装配的非敏感冷配置摘要；不同版本不可比较。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerColdConfigDigest {
    version: u32,
    sha256: String,
}

impl WorkerColdConfigDigest {
    /// 由配置 owner 提供无 map 的规范投影，按紧凑 JSON 的字段顺序计算 SHA-256。
    pub fn from_projection(
        version: u32,
        projection: &impl Serialize,
    ) -> Result<Self, serde_json::Error> {
        let payload = serde_json::to_vec(projection)?;
        let digest = RequestHash::from_canonical_bytes(&payload);
        Ok(Self { version, sha256: lower_hex(&digest.as_bytes()) })
    }

    /// 只比较受支持版本的规范摘要；其他值不能作为不一致的证据。
    pub(crate) fn comparable_to(&self, target: &Self) -> bool {
        self.version == target.version
            && self.sha256.len() == 64
            && self
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }
}

/// 仅为 Admin 读取持久化的 Worker 启动摘要。
pub trait WorkerColdConfigReader: Send + Sync {
    /// 每个未过期、未进入 stopping 的 Worker 对应一项。
    /// starting、老版本或无效详情保留为 None。
    fn live_worker_cold_configs(
        &self,
    ) -> ApplicationFuture<'_, Vec<Option<WorkerColdConfigDigest>>>;
}

/// 当前文件对应的 Worker 冷配置状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerRestartState {
    RestartRequired,
    Consistent,
    Unknown,
}
