//! Admin 装备录入的共享字段边界和错误分类。

use super::*;

/// Admin 手工装备录入策略。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AdminIntakePolicy {
    require_tradeable: bool,
}

impl AdminIntakePolicy {
    /// 建立进程启动时冻结的手工录入策略。
    pub const fn new(require_tradeable: bool) -> Self {
        Self { require_tradeable }
    }

    /// 是否拒绝来源明确标记为不可交易的装备。
    pub const fn requires_tradeable(self) -> bool {
        self.require_tradeable
    }
}

/// Admin 装备录入应用错误。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum AdminIntakeError {
    #[error("Admin 装备录入命令无效")]
    InvalidCommand,
    #[error("Admin 装备来源格式或协议不受支持")]
    InvalidSource,
    #[error("Admin 装备来源暂时不可用")]
    SourceUnavailable,
    #[error("Admin 装备来源能力未配置")]
    SourceUnconfigured,
    #[error("装备 key 加密能力未配置或失败")]
    KeyUnavailable,
    #[error("Admin 装备录入命令与已有请求冲突")]
    CommandConflict,
    #[error("Admin 装备录入结果损坏")]
    StoredResultCorrupt,
}

impl From<EquipmentKeyEnvelopeError> for AdminIntakeError {
    fn from(_: EquipmentKeyEnvelopeError) -> Self {
        Self::KeyUnavailable
    }
}

pub fn aggregate_id(world_id: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_URL, format!("auction/admin-intake/{world_id}").as_bytes())
}

pub(crate) fn decimal_identity(value: &str) -> bool {
    !value.is_empty() && value.len() <= 64 && value.bytes().all(|byte| byte.is_ascii_digit())
}

pub(crate) fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

pub(crate) fn valid_optional_text(value: &str, maximum: usize) -> bool {
    value.is_empty() || valid_text(value, maximum)
}
