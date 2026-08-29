//! 论坛未知写入的只读清单和人工重新写入授权命令。

#[cfg(feature = "postgres")]
use super::super::*;
use uuid::Uuid;

#[cfg(feature = "postgres")]
pub(crate) async fn forum_write_recovery_list(limit: u16) -> anyhow::Result<()> {
    let config = AppConfig::load(Role::Worker).context("load worker configuration")?;
    let database_url = config.database_url().context("database_url is required")?;
    let store = DatabaseStore::connect(database_url)
        .await
        .context("connect database for forum write recovery list")?;
    let items = store.list_write_recovery(limit).await.context("read forum write recovery list")?;
    println!("{}", serde_json::to_string_pretty(&items)?);
    Ok(())
}

#[cfg(feature = "postgres")]
pub(crate) async fn authorize_forum_write_retry(
    auction_id: Uuid,
    command_id: Uuid,
    actor: &str,
    confirm_write: &str,
) -> anyhow::Result<()> {
    if auction_id.is_nil() || command_id.is_nil() {
        anyhow::bail!("auction_id 和 command_id 必须是非零 UUID");
    }
    if actor.is_empty()
        || actor != actor.trim()
        || actor.len() > 256
        || actor.chars().any(char::is_control)
    {
        anyhow::bail!("actor 必须是 1 至 256 字节的规范非控制字符文本");
    }
    let expected_confirmation = format!("AUTHORIZE FORUM WRITE {auction_id}");
    if confirm_write != expected_confirmation {
        anyhow::bail!("confirm-write 不匹配；必须逐字确认目标拍卖 UUID");
    }

    #[derive(Serialize)]
    struct HashInput<'a> {
        auction_id: Uuid,
        actor_id: &'a str,
    }

    let request_hash = RequestHash::from_request(
        "authorize_forum_write_retry",
        auction_id,
        &HashInput { auction_id, actor_id: actor },
    )
    .context("build forum write authorization request digest")?;
    let config = AppConfig::load(Role::Worker).context("load worker configuration")?;
    let database_url = config.database_url().context("database_url is required")?;
    let store = DatabaseStore::connect(database_url)
        .await
        .context("connect database for forum write authorization")?;
    let result = store
        .authorize_write_retry(ForumWriteRetryAuthorizationCommand {
            command_id,
            auction_id,
            actor_id: actor.to_owned(),
            request_hash,
        })
        .await
        .context("authorize forum write retry")?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

#[cfg(not(feature = "postgres"))]
pub(crate) async fn forum_write_recovery_list(_limit: u16) -> anyhow::Result<()> {
    anyhow::bail!("forum-write-recovery-list 需要 PostgreSQL 完整档；请使用 --features postgres")
}

#[cfg(not(feature = "postgres"))]
pub(crate) async fn authorize_forum_write_retry(
    _auction_id: Uuid,
    _command_id: Uuid,
    _actor: &str,
    _confirm_write: &str,
) -> anyhow::Result<()> {
    anyhow::bail!("authorize-forum-write-retry 需要 PostgreSQL 完整档；请使用 --features postgres")
}
