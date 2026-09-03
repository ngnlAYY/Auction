//! MoogleMail 单凭据恢复与敏感值清理边界。

use super::super::*;

pub(super) async fn load_credentials(
    config: &AppConfig,
    mail_config: &auction_config::MoogleMailWorkerConfig,
    world_configs: &[auction_config::MoogleMailWorldConfig],
    store: Arc<DatabaseStore>,
) -> anyhow::Result<MoogleMailCredentialSet> {
    let credentials = if let Some((key_id, key_material)) =
        config.credential_data_key_id().zip(config.credential_data_key())
    {
        let data_key = Arc::new(
            DataKey::from_base64(key_id, key_material)
                .map_err(|error| anyhow::Error::msg(error.to_string()))?,
        );
        let opener = CredentialSealer::new(Arc::clone(&data_key));
        let reader: Arc<dyn AdminCredentialRuntimeReader> = store.clone();
        let mut records = Vec::new();
        let mut seen_record_ids = HashSet::new();
        for world_config in world_configs {
            let credential_world = AdminCredentialWorld::parse(&world_config.world_id)
                .context("MoogleMail world_id must be main or isekai")?;
            for record in reader
                .list_active(AdminCredentialKind::MailCookie, credential_world)
                .await
                .context("load active MoogleMail credentials")?
            {
                // shared scope 会在 main/isekai 两次查询中出现，只把同一凭据保留一次。
                if seen_record_ids.insert(record.id) {
                    records.push((record, credential_world));
                }
            }
        }
        anyhow::ensure!(records.len() <= 1, "MoogleMail 只允许一个 active mail_cookie");
        let ownership = if records.is_empty() {
            None
        } else {
            Some(Arc::new(
                store
                    .acquire_credential_runtime_ownership(AdminCredentialKind::MailCookie)
                    .await
                    .context("acquire active MoogleMail credential ownership")?,
            ) as Arc<dyn AdminCredentialRuntimeOwnership>)
        };
        let mut cookies = Vec::with_capacity(records.len());
        for (record, credential_world) in records {
            let secret = opener
                .open(record.kind, record.id, &record.scope, &record.envelope)
                .map_err(|error| anyhow::Error::msg(error.to_string()))?;
            let value = match String::from_utf8(secret) {
                Ok(value) => value,
                Err(error) => {
                    let mut bytes = error.into_bytes();
                    bytes.fill(0);
                    anyhow::bail!("MoogleMail 凭据不是有效 UTF-8");
                }
            };
            cookies.push((
                record.id,
                if record.scope
                    == AdminCredentialWorld::Shared.scope(AdminCredentialKind::MailCookie)
                {
                    None
                } else {
                    Some(match credential_world {
                        AdminCredentialWorld::Main => MoogleMailWorld::Main,
                        AdminCredentialWorld::Isekai => MoogleMailWorld::Isekai,
                        AdminCredentialWorld::Shared => {
                            anyhow::bail!("MoogleMail 凭据 world scope 无效")
                        }
                    })
                },
                MoogleMailCookie::new(value)
                    .map_err(|error| anyhow::Error::msg(error.to_string()))?,
            ));
        }
        let invalidator: Arc<dyn AdminCredentialRuntimeWriter> = store.clone();
        if let Some(ownership) = ownership {
            MoogleMailCredentialSet::from_owned_runtime_cookies_with_scopes(
                cookies,
                invalidator,
                ownership,
            )
        } else {
            MoogleMailCredentialSet::from_runtime_cookies_with_scopes(cookies, invalidator)
        }
    } else if let Some(path) = &mail_config.keyenv_file {
        let keyenv = load_mooglemail_keyenv(path).context("load MoogleMail keyenv")?;
        let ownership = Arc::new(
            store
                .acquire_credential_runtime_ownership(AdminCredentialKind::MailCookie)
                .await
                .context("acquire configured MoogleMail credential ownership")?,
        ) as Arc<dyn AdminCredentialRuntimeOwnership>;
        MoogleMailCredentialSet::from_owned_cookie(
            MoogleMailCookie::new(keyenv.bot_cookie())
                .map_err(|error| anyhow::Error::msg(error.to_string()))?,
            ownership,
        )
    } else {
        let cookie = config.mooglemail_cookie().context(
            "MoogleMail enabled but no keyenv_file, credential record, or sensitive mooglemail_cookie",
        )?;
        let ownership = Arc::new(
            store
                .acquire_credential_runtime_ownership(AdminCredentialKind::MailCookie)
                .await
                .context("acquire configured MoogleMail credential ownership")?,
        ) as Arc<dyn AdminCredentialRuntimeOwnership>;
        MoogleMailCredentialSet::from_owned_cookie(
            MoogleMailCookie::new(cookie).map_err(|error| anyhow::Error::msg(error.to_string()))?,
            ownership,
        )
    };
    credentials.map_err(|error| anyhow::Error::msg(error.to_string()))
}
