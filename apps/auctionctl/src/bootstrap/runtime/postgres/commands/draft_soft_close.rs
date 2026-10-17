//! 显式草稿修正装配；不运行迁移、不修改配置、不调用外部服务。

use uuid::Uuid;

#[cfg(feature = "postgres")]
pub(crate) async fn repair_draft_soft_close(
    auction_id: Uuid,
    execute: bool,
    expected_version: Option<i64>,
    command_id: Option<Uuid>,
    actor: Option<String>,
    confirm_write: Option<String>,
) -> anyhow::Result<()> {
    use anyhow::Context;
    use auction_application::draft_soft_close::{DraftSoftClosePort, RepairDraftSoftCloseCommand};
    use auction_config::{AppConfig, Role};
    use auction_storage::DatabaseStore;

    if auction_id.is_nil() {
        anyhow::bail!("auction-id 必须为非零 UUID");
    }
    let command = if execute {
        let command = RepairDraftSoftCloseCommand {
            auction_id,
            expected_version: expected_version.context("execute 必须提供 expected-version")?,
            command_id: command_id.context("execute 必须提供稳定 command-id")?,
            actor: actor.context("execute 必须提供 actor")?,
            confirmation: confirm_write.context("execute 必须提供 confirm-write")?,
        };
        command.validate()?;
        Some(command)
    } else {
        if expected_version.is_some()
            || command_id.is_some()
            || actor.is_some()
            || confirm_write.is_some()
        {
            anyhow::bail!("只读模式不接受写入参数；需要写入时显式提供 --execute");
        }
        None
    };
    let config = AppConfig::load(Role::Admin).context("加载草稿修正 Admin 配置失败")?;
    let store = DatabaseStore::connect(config.database_url().context("缺少数据库配置")?)
        .await
        .context("连接草稿修正数据库失败")?;
    if let Some(command) = command {
        let result = store.repair_draft_soft_close(command).await?;
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        let inspection = store.inspect_draft_soft_close(auction_id).await?;
        println!("{}", serde_json::to_string_pretty(&inspection)?);
    }
    Ok(())
}

#[cfg(not(feature = "postgres"))]
pub(crate) async fn repair_draft_soft_close(
    _auction_id: Uuid,
    _execute: bool,
    _expected_version: Option<i64>,
    _command_id: Option<Uuid>,
    _actor: Option<String>,
    _confirm_write: Option<String>,
) -> anyhow::Result<()> {
    anyhow::bail!("repair-draft-soft-close 需要 postgres feature")
}
