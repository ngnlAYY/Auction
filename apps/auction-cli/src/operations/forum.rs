//! 论坛主题未知写入的恢复：只读清单，以及精确标题对账未命中后的单次重试授权。

use anyhow::Context;
use auction_application::{
    ForumSyncRecoveryPort, ForumWriteRetryAuthorizationCommand, RequestHash,
};
use serde::Serialize;
use uuid::Uuid;

use crate::app::print_json;
use crate::cli::ForumCommand;
use crate::deployment::ConfigDir;
use crate::support::text;

pub(crate) async fn run(config_dir: &ConfigDir, command: ForumCommand) -> anyhow::Result<()> {
    let (_, store) =
        super::worker_store(config_dir, auction_storage::DatabaseConnectionPurpose::CliOperations)
            .await?;
    let result = async {
        match command {
            ForumCommand::Recovery { limit } => {
                let items =
                    store.list_write_recovery(limit).await.context("读取论坛恢复清单失败")?;
                print_json(&serde_json::json!({ "items": items }))
            }
            ForumCommand::AuthorizeRetry { auction_id, command_id, actor, confirm } => {
                let auction_id = text::command_id(auction_id, "--auction-id")?;
                let command_id = text::command_id(command_id, "--command-id")?;
                let actor = text::actor(&actor)?;
                text::confirmation(&confirm, &format!("AUTHORIZE FORUM WRITE {auction_id}"))?;
                #[derive(Serialize)]
                struct HashInput<'a> {
                    auction_id: Uuid,
                    actor_id: &'a str,
                }
                let request_hash = RequestHash::from_request(
                    "authorize_forum_write_retry",
                    auction_id,
                    &HashInput { auction_id, actor_id: actor },
                )?;
                let result = store
                    .authorize_write_retry(ForumWriteRetryAuthorizationCommand {
                        command_id,
                        auction_id,
                        actor_id: actor.to_owned(),
                        request_hash,
                    })
                    .await
                    .context("授权论坛重试失败")?;
                print_json(&result)
            }
        }
    }
    .await;
    store.postgres_pool().close().await;
    result
}
