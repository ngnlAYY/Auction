//! Admin 管理的非敏感运行参数；接管开关与实际功能开关分离，旧部署默认继续沿用角色配置。

mod alerts;
mod economics;
mod forum;
mod schedule;
mod worker;

use serde::{Deserialize, Deserializer, Serialize};

use super::errors::AdminRuntimeConfigError;

pub use alerts::AdminRuntimeAlertsConfig;
pub use economics::AdminRuntimeEconomicsConfig;
pub use forum::AdminRuntimeForumConfig;
pub use schedule::AdminRuntimeScheduleConfig;
pub use worker::{
    AdminRuntimeDeliveryConfig, AdminRuntimeFulfillmentConfig, AdminRuntimeRecoveryConfig,
    AdminRuntimeWorkerConfig,
};

/// 五组可独立接管的参数；保存只更新候选文件，进程重启后才消费。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeOperationsConfig {
    /// 后续新拍卖的经济模板，不修改已冻结快照。
    pub economics: AdminRuntimeEconomicsConfig,
    /// 后续轮次的周排期，不改写持久化游标或既有拍卖时间。
    pub schedule: AdminRuntimeScheduleConfig,
    /// 论坛同步开关、请求预算和轮询边界。
    pub forum: AdminRuntimeForumConfig,
    /// 运维告警阈值、收件目标和批次边界。
    pub alerts: AdminRuntimeAlertsConfig,
    /// 各 Worker 循环的吞吐、租约和重试边界。
    pub worker: AdminRuntimeWorkerConfig,
}

impl AdminRuntimeOperationsConfig {
    /// 使用纯文档约束校验所有候选，保证读取、保存和故障恢复采用同一规则。
    pub fn validate(&self) -> Result<(), AdminRuntimeConfigError> {
        self.economics.validate()?;
        self.schedule.validate()?;
        self.forum.validate()?;
        self.alerts.validate()?;
        self.worker.validate()
    }
}

/// 旧文档可以省略整个区块；显式 null 不能被解释为撤销管理页配置。
pub(super) fn deserialize_optional<'de, D>(
    deserializer: D,
) -> Result<Option<AdminRuntimeOperationsConfig>, D::Error>
where
    D: Deserializer<'de>,
{
    AdminRuntimeOperationsConfig::deserialize(deserializer).map(Some)
}

fn require(valid: bool, field: &str) -> Result<(), AdminRuntimeConfigError> {
    if valid { Ok(()) } else { Err(invalid(field)) }
}

fn invalid(field: &str) -> AdminRuntimeConfigError {
    AdminRuntimeConfigError::InvalidDocument(format!("operations.{field} 超出允许范围或存在冲突"))
}
