//! runtime 文档的部署格式校验；复用 Worker 使用的 cron 解析器，不建立第二套语法。

use auction_application::{AdminRuntimeConfigError, AdminRuntimeDocument, RecurringSchedule};
use chrono::{DateTime, Utc};

use crate::StandardCronSchedule;

/// 校验领域约束与 cron 的未来触发时刻，不读取文件或外部服务。
pub(crate) fn validate_runtime_document(
    document: &AdminRuntimeDocument,
    now: DateTime<Utc>,
) -> Result<(), AdminRuntimeConfigError> {
    document.validate()?;
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
