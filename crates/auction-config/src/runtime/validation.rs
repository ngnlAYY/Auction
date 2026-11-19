//! runtime 文档的部署格式校验；复用 Worker 使用的 cron 解析器，不建立第二套语法。

use auction_application::admin::{
    AdminRuntimeAlertsConfig, AdminRuntimeConfigError, AdminRuntimeDocument,
};
use auction_application::scheduling::RecurringSchedule;
use auction_time::Timestamp;

use crate::StandardCronSchedule;
use crate::settings::sender_allowlist_has_case_duplicates;

/// 只比较受 Worker 部署基值约束的 runtime 覆盖；未接管的字段不参与比较。
#[derive(PartialEq, Eq)]
pub(super) struct RuntimeWorkerOverrides<'a> {
    alerts: Option<AdminRuntimeAlertsConfig>,
    mail_worlds: Option<[bool; 2]>,
    mail_schedule: Option<&'a str>,
    auction_worlds: Option<&'a [String]>,
}

impl<'a> RuntimeWorkerOverrides<'a> {
    pub(super) fn from_document(document: &'a AdminRuntimeDocument) -> Self {
        let operations = document.operations.as_ref();
        let alerts = operations
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
        let mail_schedule = operations
            .and_then(|operations| operations.mail.as_ref())
            .filter(|mail| mail.managed)
            .map(|mail| mail.schedule.as_str());
        // 未接管时回退部署排期；接管但关闭时不使用排期世界。
        let auction_worlds = operations
            .map(|operations| &operations.schedule)
            .filter(|schedule| schedule.managed)
            .map(|schedule| if schedule.enabled { schedule.worlds.as_slice() } else { &[] });
        Self { alerts, mail_worlds, mail_schedule, auction_worlds }
    }
}

/// 仅限制新候选，旧文件和已登记的命令仍保留原拼写及历史指纹。
pub(super) fn validate_sender_allowlists(
    document: &AdminRuntimeDocument,
) -> Result<(), AdminRuntimeConfigError> {
    if let Some(worlds) = &document.worlds {
        for (name, world) in [("main", &worlds.main), ("isekai", &worlds.isekai)] {
            if sender_allowlist_has_case_duplicates(&world.sender_allowlist) {
                return Err(AdminRuntimeConfigError::InvalidDocument(format!(
                    "worlds.{name}.sender_allowlist 含有 ASCII 大小写重复的发件人"
                )));
            }
        }
    }
    Ok(())
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
