//! runtime 文档的部署格式校验；复用 Worker 使用的 cron 解析器，不建立第二套语法。

use auction_application::admin::{
    AdminRuntimeAlertsConfig, AdminRuntimeConfigError, AdminRuntimeDocument,
};
use auction_application::scheduling::RecurringSchedule;
use auction_time::Timestamp;

use crate::StandardCronSchedule;

/// 只保留会影响告警配置的覆盖项；比较不依赖可能已过时的部署基值。
#[derive(PartialEq, Eq)]
pub(super) struct RuntimeAlertOverrides {
    alerts: Option<AdminRuntimeAlertsConfig>,
    mail_worlds: Option<[bool; 2]>,
}

impl RuntimeAlertOverrides {
    pub(super) fn from_document(document: &AdminRuntimeDocument) -> Self {
        let alerts = document
            .operations
            .as_ref()
            .map(|operations| &operations.alerts)
            .filter(|alerts| alerts.managed)
            .map(|alerts| AdminRuntimeAlertsConfig {
                fulfillment_collection_overdue_age_seconds: Some(
                    alerts.collection_overdue_age_seconds(),
                ),
                ..alerts.clone()
            });
        let mail_worlds =
            document.worlds.as_ref().map(|worlds| [worlds.main.enabled, worlds.isekai.enabled]);
        Self { alerts, mail_worlds }
    }
}

/// 校验领域约束与 cron 的未来触发时刻，不读取文件或外部服务。
pub(crate) fn validate_runtime_document(
    document: &AdminRuntimeDocument,
    now: Timestamp,
) -> Result<(), AdminRuntimeConfigError> {
    document.validate()?;
    crate::settings::validate_runtime_startup(document)
        .map_err(|error| AdminRuntimeConfigError::InvalidDocument(error.to_string()))?;
    if let Some(mail) = document.operations.as_ref().and_then(|operations| operations.mail.as_ref())
    {
        let schedule = StandardCronSchedule::parse(&mail.schedule).map_err(|_| {
            AdminRuntimeConfigError::InvalidDocument(
                "operations.mail.schedule 必须是有效的标准 cron、描述符或 @every 调度".to_owned(),
            )
        })?;
        if schedule.next_after(now).is_none() {
            return Err(AdminRuntimeConfigError::InvalidDocument(
                "operations.mail.schedule 必须存在可计算的未来触发时刻".to_owned(),
            ));
        }
    }
    Ok(())
}
