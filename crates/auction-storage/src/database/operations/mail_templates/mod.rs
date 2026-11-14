//! 出站通知模板修订事实的 PostgreSQL 查询与 Read Committed 写入事务；不改变业务表。

mod queries;
mod transaction;

use auction_application::mail_templates::{
    MailTemplateCommand, MailTemplateEvent, MailTemplateFuture, MailTemplateStore,
};

use crate::database::DatabaseStore;

impl MailTemplateStore for DatabaseStore {
    fn latest_mail_templates(&self) -> MailTemplateFuture<'_, Vec<MailTemplateEvent>> {
        let store = self;
        Box::pin(queries::latest(store))
    }

    fn write_mail_template(
        &self,
        command: MailTemplateCommand,
    ) -> MailTemplateFuture<'_, MailTemplateEvent> {
        let store = self;
        Box::pin(async move {
            crate::database::backend::retry_conflicts(
                store.pool(),
                auction_observability::DatabaseTransactionOwner::Admin,
                crate::database::backend::SERIALIZABLE_ATTEMPTS,
                |error| {
                    matches!(error, transaction::WriteError::Sql(error)
                    if crate::database::backend::observed_serialization_conflict(error))
                },
                || transaction::write(store, &command),
            )
            .await
            .map_err(transaction::WriteError::public)
        })
    }
}
