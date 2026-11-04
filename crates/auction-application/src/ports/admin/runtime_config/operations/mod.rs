//! Admin 管理的非敏感运行参数；接管开关与实际功能开关分离，旧部署默认继续沿用角色配置。

mod alerts;
mod economics;
mod forum;
mod mail;
mod schedule;
mod worker;

use serde::{Deserialize, Deserializer, Serialize};

use super::errors::AdminRuntimeConfigError;

pub use alerts::AdminRuntimeAlertsConfig;
pub use economics::AdminRuntimeEconomicsConfig;
pub use forum::AdminRuntimeForumConfig;
pub use mail::{AdminRuntimeMailConfig, AdminRuntimeMailDeliveryBudget};
pub use schedule::AdminRuntimeScheduleConfig;
pub use worker::{
    AdminRuntimeDeliveryConfig, AdminRuntimeFulfillmentConfig, AdminRuntimeRecoveryConfig,
    AdminRuntimeWorkerConfig,
};

/// 可独立接管的参数；保存只更新候选文件。Worker 的节奏/预算字段由周期热读取在下一个
/// 循环边界使用，其余字段在进程重启后才消费。
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
    /// 邮件调度与处理预算；省略时不改变升级前文件和命令账本的序列化指纹。
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "mail::deserialize_optional"
    )]
    pub mail: Option<AdminRuntimeMailConfig>,
}

impl AdminRuntimeOperationsConfig {
    /// 使用纯文档约束校验所有候选，保证读取、保存和故障恢复采用同一规则。
    pub fn validate(&self) -> Result<(), AdminRuntimeConfigError> {
        self.economics.validate()?;
        self.schedule.validate()?;
        self.forum.validate()?;
        self.alerts.validate()?;
        self.worker.validate()?;
        if let Some(mail) = &self.mail {
            mail.validate()?;
        }
        Ok(())
    }

    /// 只校验新候选的有效预算，不改变既有命令结果或读取时的文档合法性。
    /// 未接管值来自真实 Worker 启动快照；缺失上下文时不能把编辑页候选当作部署基值。
    pub fn validate_mail_delivery_budget(
        &self,
        base: Option<AdminRuntimeMailDeliveryBudget>,
    ) -> Result<(), AdminRuntimeConfigError> {
        let mail = self.mail.as_ref().filter(|mail| mail.managed);
        let budget = if let Some(mut budget) = base {
            if let Some(mail) = mail {
                budget.mail_fetch_timeout_seconds = mail.fetch_timeout_seconds;
            }
            if self.worker.managed {
                budget.delivery_request_timeout_seconds =
                    self.worker.delivery.request_timeout_seconds;
            }
            budget
        } else {
            match (mail, self.worker.managed) {
                (None, false) => return Ok(()),
                (Some(mail), true) => AdminRuntimeMailDeliveryBudget {
                    // 两组完整接管可证明启用时也安全，无需猜测部署侧 enabled。
                    mail_enabled: true,
                    mail_fetch_timeout_seconds: mail.fetch_timeout_seconds,
                    delivery_request_timeout_seconds: self.worker.delivery.request_timeout_seconds,
                },
                _ => {
                    return Err(AdminRuntimeConfigError::InvalidDocument(
                        "单独接管 operations.mail 或 operations.worker 前，须在 Admin 配置 worker_config_file 并重启，以校验真实 Worker 部署预算".to_owned(),
                    ));
                }
            }
        };
        if !budget.is_valid() {
            return Err(AdminRuntimeConfigError::InvalidDocument(
                "MoogleMail 启用时 operations.worker.delivery.request_timeout_seconds 必须大于有效 mail.fetch_timeout_seconds 的两倍".to_owned(),
            ));
        }
        Ok(())
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
