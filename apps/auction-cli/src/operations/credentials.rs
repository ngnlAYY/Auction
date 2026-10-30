//! 外部 Cookie 凭据：安全列表与 stdin 导入，复用 Admin 的凭据服务、密封与审计。

use std::{io::Read, sync::Arc};

use anyhow::Context;
use auction_application::{
    AdminCredentialImportCommand, AdminCredentialImportSource, AdminCredentialKind,
    AdminCredentialListRequest, AdminCredentialPort, AdminCredentialSecret, AdminCredentialService,
    AdminCredentialStatus, AdminCredentialWorld, MAX_ADMIN_CREDENTIAL_SECRET_BYTES,
};
use auction_integrations::{CredentialAcquirer, CredentialSealer, DataKey};
use uuid::Uuid;

use crate::app::print_json;
use crate::cli::{CredentialCommand, CredentialKindArg, CredentialStatusArg, CredentialWorldArg};
use crate::deployment::ConfigDir;
use crate::support::text;

pub(crate) async fn run(config_dir: &ConfigDir, command: CredentialCommand) -> anyhow::Result<()> {
    let (config, store) = super::worker_store(config_dir).await?;
    let (key_id, key_material) = config
        .credential_data_key_id()
        .zip(config.credential_data_key())
        .context("敏感配置缺少 credential_data_key_id/credential_data_key")?;
    let data_key = Arc::new(
        DataKey::from_base64(key_id, key_material)
            .map_err(|_| anyhow::anyhow!("credential 数据密钥格式无效"))?,
    );
    let sealer = Arc::new(CredentialSealer::new(Arc::clone(&data_key)));
    let acquirer =
        Arc::new(CredentialAcquirer::production(data_key).context("初始化凭据获取客户端失败")?);
    let service = AdminCredentialService::new(Arc::clone(&store), sealer, acquirer);
    let result = execute(&service, command).await;
    store.postgres_pool().close().await;
    result
}

async fn execute(
    service: &dyn AdminCredentialPort,
    command: CredentialCommand,
) -> anyhow::Result<()> {
    match command {
        CredentialCommand::List { kind, world, status, limit, offset } => {
            let page = service
                .list(AdminCredentialListRequest {
                    kind: kind.map(credential_kind),
                    world: world.map(credential_world),
                    status: status.map(|status| match status {
                        CredentialStatusArg::Active => AdminCredentialStatus::Active,
                        CredentialStatusArg::Disabled => AdminCredentialStatus::Disabled,
                        CredentialStatusArg::Invalid => AdminCredentialStatus::Invalid,
                    }),
                    limit,
                    offset,
                })
                .await
                .context("读取凭据列表失败")?;
            print_json(&serde_json::json!({
                "total": page.total, "limit": page.limit, "offset": page.offset,
                "credentials": page.credentials,
            }))
        }
        CredentialCommand::Import { kind, world, label, player_name, actor } => {
            let actor = text::actor(&actor)?.to_owned();
            let secret = AdminCredentialSecret::new(read_cookie()?)
                .map_err(|_| anyhow::anyhow!("stdin 中的 Cookie 无效"))?;
            let credential = service
                .import(AdminCredentialImportCommand {
                    command_id: Uuid::new_v4(),
                    kind: credential_kind(kind),
                    world: credential_world(world),
                    label,
                    player_name,
                    source: AdminCredentialImportSource::Cookie(secret),
                    actor_id: actor,
                    request_id: None,
                    now: chrono::Utc::now(),
                })
                .await
                .context("导入凭据失败")?;
            print_json(&serde_json::json!({
                "credential": credential,
                "restart_required": "worker",
            }))
        }
    }
}

/// 只从 stdin 读取，去掉末尾换行；Cookie 不经命令行参数或环境变量。
fn read_cookie() -> anyhow::Result<String> {
    // 与 Admin 导入共用同一上限，超出即视为误输入。
    let limit = MAX_ADMIN_CREDENTIAL_SECRET_BYTES as u64;
    let mut input = String::new();
    std::io::stdin()
        .take(limit + 1)
        .read_to_string(&mut input)
        .context("从 stdin 读取 Cookie 失败")?;
    if input.len() as u64 > limit {
        anyhow::bail!("Cookie 超过 {limit} 字节上限");
    }
    let trimmed = input.trim_end_matches(['\r', '\n']).len();
    input.truncate(trimmed);
    if input.trim().is_empty() {
        anyhow::bail!("stdin 中没有 Cookie");
    }
    Ok(input)
}

const fn credential_kind(kind: CredentialKindArg) -> AdminCredentialKind {
    match kind {
        CredentialKindArg::Mail => AdminCredentialKind::MailCookie,
        CredentialKindArg::Forum => AdminCredentialKind::ForumCookie,
    }
}

const fn credential_world(world: CredentialWorldArg) -> AdminCredentialWorld {
    match world {
        CredentialWorldArg::Main => AdminCredentialWorld::Main,
        CredentialWorldArg::Isekai => AdminCredentialWorld::Isekai,
        CredentialWorldArg::Shared => AdminCredentialWorld::Shared,
    }
}
