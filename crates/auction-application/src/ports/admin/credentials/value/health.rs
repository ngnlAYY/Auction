//! Admin dashboard 的凭据安全聚合值对象。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::model::{AdminCredentialKind, AdminCredentialWorld};

/// Dashboard 使用的凭据按 kind/world 聚合行，不包含密文、明文或指纹。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminCredentialHealthScope {
    pub kind: AdminCredentialKind,
    pub world: AdminCredentialWorld,
    pub active_count: i64,
    pub disabled_count: i64,
    pub invalid_count: i64,
    /// 该范围内最近一次凭据记录变化时间；没有记录时为空。
    pub updated_at: Option<DateTime<Utc>>,
}

impl AdminCredentialHealthScope {
    /// 校验计数和时间不会以负数、溢出或无来源状态进入 Admin dashboard。
    pub fn validate(&self) -> bool {
        let total = self
            .active_count
            .checked_add(self.disabled_count)
            .and_then(|count| count.checked_add(self.invalid_count));
        let time_shape_valid = match total {
            Some(0) => self.updated_at.is_none(),
            Some(value) if value > 0 => self.updated_at.is_some(),
            _ => false,
        };
        self.active_count >= 0
            && self.disabled_count >= 0
            && self.invalid_count >= 0
            && self.updated_at.is_none_or(|value| value.timestamp_nanos_opt().is_some())
            && time_shape_valid
    }
}

/// Dashboard 的凭据安全聚合快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminCredentialHealthSnapshot {
    pub observed_at: DateTime<Utc>,
    /// 凭据管理命令账本中最近一次变更时间，用于判断 Worker 是否需要重启。
    pub last_admin_change_at: Option<DateTime<Utc>>,
    pub scopes: Vec<AdminCredentialHealthScope>,
}

impl AdminCredentialHealthSnapshot {
    /// 校验稳定排序、范围唯一性和有界计数。
    pub fn validate(&self) -> bool {
        self.observed_at.timestamp_nanos_opt().is_some()
            && self.last_admin_change_at.is_none_or(|value| value.timestamp_nanos_opt().is_some())
            && self.scopes.len() <= 16
            && self.scopes.iter().enumerate().all(|(index, scope)| {
                scope.validate()
                    && self.scopes[..index].iter().all(|previous| {
                        previous.kind != scope.kind || previous.world != scope.world
                    })
            })
    }
}
