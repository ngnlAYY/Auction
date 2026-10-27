//! 草稿软截止显式修正的 PostgreSQL 事务 owner；不参与自动迁移或调度。

use auction_application::draft_soft_close::{
    DraftSoftCloseError, DraftSoftCloseFuture, DraftSoftCloseInspection, DraftSoftClosePort,
    DraftSoftCloseRepairResult, RepairDraftSoftCloseCommand,
};
use uuid::Uuid;

use super::{DatabaseStore, PostgresStore};

mod read;
mod write;

impl DraftSoftClosePort for DatabaseStore {
    fn inspect_draft_soft_close(
        &self,
        auction_id: Uuid,
    ) -> DraftSoftCloseFuture<'_, DraftSoftCloseInspection> {
        Box::pin(async move {
            if auction_id.is_nil() {
                return Err(DraftSoftCloseError::InvalidRequest);
            }
            match self {
                Self::Postgres(store) => inspect(store, auction_id).await,
            }
        })
    }

    fn repair_draft_soft_close(
        &self,
        command: RepairDraftSoftCloseCommand,
    ) -> DraftSoftCloseFuture<'_, DraftSoftCloseRepairResult> {
        Box::pin(async move {
            command.validate()?;
            match self {
                Self::Postgres(store) => {
                    // 只重试 PostgreSQL 明确回滚的串行化冲突/死锁；提交结果未知由原命令重放解决。
                    for attempt in 0..3 {
                        match write::execute(store, &command).await {
                            Ok(result) => return Ok(result),
                            Err(Failure::Sql(error)) if retryable(&error) => {
                                if attempt == 2 {
                                    return Err(DraftSoftCloseError::RetryExhausted);
                                }
                            }
                            Err(error) => return Err(error.into_application()),
                        }
                    }
                    Err(DraftSoftCloseError::RetryExhausted)
                }
            }
        })
    }
}

async fn inspect(
    store: &PostgresStore,
    auction_id: Uuid,
) -> Result<DraftSoftCloseInspection, DraftSoftCloseError> {
    let result: Result<_, Failure> = async {
        // 默认检查由数据库强制只读，无行锁、账本、审计或隐含写入。
        let mut transaction =
            store.pool().begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE, READ ONLY").await?;
        let snapshot = read::snapshot(&mut transaction, auction_id).await?;
        let inspection = snapshot.inspect();
        transaction.commit().await?;
        Ok(inspection)
    }
    .await;
    result.map_err(Failure::into_application)
}

#[derive(Debug)]
enum Failure {
    Sql(sqlx::Error),
    Application(DraftSoftCloseError),
}

impl From<sqlx::Error> for Failure {
    fn from(error: sqlx::Error) -> Self {
        Self::Sql(error)
    }
}

impl From<DraftSoftCloseError> for Failure {
    fn from(error: DraftSoftCloseError) -> Self {
        Self::Application(error)
    }
}

impl Failure {
    fn into_application(self) -> DraftSoftCloseError {
        match self {
            Self::Application(error) => error,
            Self::Sql(_) => DraftSoftCloseError::Storage,
        }
    }
}

fn retryable(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .is_some_and(|error| matches!(error.code().as_deref(), Some("40001" | "40P01")))
}
