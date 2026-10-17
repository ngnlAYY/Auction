//! 装备 PM 运维：只访问持久端口，不初始化网关或加载论坛 Cookie。
#[cfg(feature = "postgres")]
use super::super::*;
#[cfg(feature = "postgres")]
use auction_application::{LotWatchPmOperations, RequestLotWatchPmReconcile};
use uuid::Uuid;

pub(crate) async fn lot_watch_pm_list(
    status: Option<String>,
    limit: i64,
    offset: i64,
) -> anyhow::Result<()> {
    #[cfg(feature = "postgres")]
    {
        let config = AppConfig::load(Role::Worker).context("load worker configuration")?;
        let store =
            DatabaseStore::connect(config.database_url().context("database_url is required")?)
                .await
                .context("connect PM operations store")?;
        let rows = LotWatchPmOperations::list(&store, status, limit, offset)
            .await
            .context("read PM delivery list")?;
        println!("{}", serde_json::to_string_pretty(&rows)?);
        Ok(())
    }
    #[cfg(not(feature = "postgres"))]
    {
        let _ = (status, limit, offset);
        anyhow::bail!("lot-watch-pm 需要 PostgreSQL 完整档；请使用 --features postgres")
    }
}

pub(crate) async fn lot_watch_pm_reconcile(
    notification_id: Uuid,
    command_id: Uuid,
    expected_fencing_version: i64,
    actor: String,
) -> anyhow::Result<()> {
    #[cfg(feature = "postgres")]
    {
        let config = AppConfig::load(Role::Worker).context("load worker configuration")?;
        let store =
            DatabaseStore::connect(config.database_url().context("database_url is required")?)
                .await
                .context("connect PM operations store")?;
        let result = store
            .request_reconcile(RequestLotWatchPmReconcile {
                notification_id,
                command_id: command_id.into(),
                expected_fencing_version,
                actor,
            })
            .await
            .context("request read-only PM reconciliation")?;
        // 这是原命令的持久接受结果，不是已发送确认。
        println!("{}", serde_json::to_string_pretty(&result)?);
        Ok(())
    }
    #[cfg(not(feature = "postgres"))]
    {
        let _ = (notification_id, command_id, expected_fencing_version, actor);
        anyhow::bail!("lot-watch-pm 需要 PostgreSQL 完整档；请使用 --features postgres")
    }
}
