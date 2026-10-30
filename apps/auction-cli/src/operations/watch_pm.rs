//! 装备超价提醒私信的人工对账：只访问持久端口，不加载论坛 Cookie，也不提供重发入口。

use anyhow::Context;
use auction_application::{LotWatchPmOperations, RequestLotWatchPmReconcile};

use crate::app::print_json;
use crate::cli::PmCommand;
use crate::deployment::ConfigDir;
use crate::support::text;

pub(crate) async fn run(config_dir: &ConfigDir, command: PmCommand) -> anyhow::Result<()> {
    let (_, store) = super::worker_store(config_dir).await?;
    let result = async {
        match command {
            PmCommand::List { status, limit, offset } => {
                let rows = LotWatchPmOperations::list(store.as_ref(), status, limit, offset)
                    .await
                    .context("读取装备 PM 列表失败")?;
                print_json(&serde_json::json!({ "items": rows }))
            }
            PmCommand::Reconcile {
                notification_id,
                command_id,
                expected_fencing_version,
                actor,
            } => {
                let command_id = text::command_id(command_id, "--command-id")?;
                let actor = text::actor(&actor)?.to_owned();
                // 结果是原命令的持久接受快照，不表示私信已经送达。
                let result = store
                    .request_reconcile(RequestLotWatchPmReconcile {
                        notification_id,
                        command_id: command_id.into(),
                        expected_fencing_version,
                        actor,
                    })
                    .await
                    .context("请求装备 PM 只读对账失败")?;
                print_json(&result)
            }
        }
    }
    .await;
    store.postgres_pool().close().await;
    result
}
