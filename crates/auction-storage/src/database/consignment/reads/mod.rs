//! 寄售读端口与持久补扫游标。

mod availability;
mod catalog;
mod commands;
mod deposits;
mod maintenance;
mod private;
mod projection;

#[cfg(feature = "test-db")]
mod test_access;

use auction_application::consignment::{ConsignmentError, ListingPageWindow};
use auction_time::Timestamp;
use sqlx::{Postgres, QueryBuilder};

use crate::database::DatabaseStore;
use crate::database::backend::set_read_only_limits;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::pool::TrackedTransaction;

fn unavailable<T>(_: T) -> ConsignmentError {
    ConsignmentError::Unavailable
}

fn storage_error(error: sqlx::Error) -> ConsignmentError {
    WriteError::from(error).into_application()
}

async fn snapshot(
    store: &DatabaseStore,
    require_admission: bool,
) -> Result<(TrackedTransaction<'static, Postgres>, Timestamp), ConsignmentError> {
    let mut tx = store.pool().begin_read_snapshot().await.map_err(storage_error)?;
    set_read_only_limits(&mut tx).await.map_err(|(_, source)| storage_error(source))?;
    // 第一条读语句固定 MVCC 快照及同页、总数共用的截止比较时刻。
    let sql = if require_admission {
        // 单例应由迁移创建。公开端口将依赖缺失和合法的空目录区分开。
        "SELECT clock_timestamp() FROM public.consignment_lookup_control WHERE id = 1"
    } else {
        "SELECT clock_timestamp()"
    };
    let as_of = sqlx::query_scalar(sql).fetch_one(&mut *tx).await.map_err(storage_error)?;
    Ok((tx, as_of))
}

fn page_order(query: &mut QueryBuilder<Postgres>, window: ListingPageWindow) {
    query
        .push(" ORDER BY l.updated_at DESC, l.id DESC LIMIT ")
        .push_bind(i64::from(window.page_size()))
        .push(" OFFSET ")
        .push_bind(i64::from(window.offset()));
}
