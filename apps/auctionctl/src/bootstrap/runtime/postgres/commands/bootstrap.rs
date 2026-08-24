//! PostgreSQL 完整档的 MoogleMail 凭据引导命令。

#[cfg(feature = "postgres")]
use super::super::*;

#[cfg(feature = "postgres")]
pub(crate) async fn bootstrap_mail_credentials(world: &str, label: &str) -> anyhow::Result<()> {
    let config = AppConfig::load(Role::Worker).context("load worker configuration")?;
    if !config.worker.mooglemail.enabled {
        anyhow::bail!("Worker 的 MoogleMail 功能未启用");
    }
    let credential_world = match world {
        "main" => AdminCredentialWorld::Main,
        "isekai" => AdminCredentialWorld::Isekai,
        _ => anyhow::bail!("bootstrap mail world 必须是 main 或 isekai"),
    };
    if !config
        .worker
        .mooglemail
        .configured_worlds()
        .iter()
        .any(|configured| configured.world_id == world)
    {
        anyhow::bail!("bootstrap mail world 必须已在 Worker 配置中启用");
    }

    let database_url =
        config.database_url().context("database_url is required for credential bootstrap")?;
    if !matches!(DatabaseBackend::from_url(database_url)?, DatabaseBackend::Postgres) {
        anyhow::bail!("bootstrap-mail-credentials 只支持 PostgreSQL 完整档");
    }
    let data_key_id = config
        .credential_data_key_id()
        .context("credential_data_key_id is required for credential bootstrap")?;
    let data_key_material = config
        .credential_data_key()
        .context("credential_data_key is required for credential bootstrap")?;
    let data_key = DataKey::from_base64(data_key_id, data_key_material)
        .map_err(|_| anyhow::anyhow!("credential data key is invalid"))?;
    let cookie = read_bootstrap_cookie().context("read bootstrap mail Cookie from stdin")?;
    let secret = AdminCredentialSecret::new(cookie)
        .map_err(|_| anyhow::anyhow!("bootstrap mail Cookie is invalid"))?;
    let store = Arc::new(
        DatabaseStore::connect(database_url)
            .await
            .context("connect database for credential bootstrap")?,
    );
    let sealer = Arc::new(CredentialSealer::new(Arc::new(data_key)));
    let service = AdminCredentialService::new(store, sealer);
    let credential = service
        .import(AdminCredentialImportCommand {
            command_id: Uuid::new_v4(),
            kind: AdminCredentialKind::MailCookie,
            world: credential_world,
            label: label.to_owned(),
            secret,
            actor_id: "auctionctl/bootstrap-mail-credentials".to_owned(),
            request_id: None,
            now: Utc::now(),
        })
        .await
        .context("register bootstrap mail credential")?;
    println!(
        "credential_id={} kind={} world={} label={} status={}",
        credential.id,
        credential.kind.as_str(),
        credential.world.as_str(),
        credential.label,
        credential.status.as_str(),
    );
    Ok(())
}

#[cfg(not(feature = "postgres"))]
pub(crate) async fn bootstrap_mail_credentials(_world: &str, _label: &str) -> anyhow::Result<()> {
    anyhow::bail!("bootstrap-mail-credentials 需要 PostgreSQL 完整档；请使用 --features postgres")
}
