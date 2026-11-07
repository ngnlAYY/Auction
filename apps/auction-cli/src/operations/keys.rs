//! 数据密钥环的只读核对与离线重加密。
//!
//! 重加密只改变信封的 key ID、密文、nonce 与指纹，不改变业务事实；每次写入都以旧 key ID
//! 作 CAS 条件，并在返回前用新信封重新解密核对明文。密钥材料、明文和指纹都不会进入日志、
//! 回执或审计。

use std::{collections::BTreeMap, sync::Arc};

use anyhow::Context;
use auction_application::admin::{
    AdminCredentialEnvelope, AdminCredentialKind, AdminCredentialOpener, AdminCredentialSealer,
    AdminLoginKeyIssuer, AdminLoginKeyOpener,
};
use auction_application::mail::{MailLoginKey, MailLoginKeyEnvelope};
use auction_config::{AppConfig, DataKeyDomain};
use auction_domain::{EquipmentId, EquipmentKey, EquipmentKeyEnvelope};
use auction_integrations::{
    CredentialSealer, EquipmentKeyOpener, EquipmentKeySealer, Keyring, MoogleMailLoginKeyIssuer,
    MoogleMailLoginKeyReader, build_keyring,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::app::print_json;
use crate::cli::{KeysCommand, KeysReencryptArgs};
use crate::deployment::ConfigDir;
use crate::support::files;

const RECEIPT_FORMAT: &str = "auction-key-reencrypt-receipt-v1";

/// 数据库信封所在域；诊断归档是一次写入文件，不在重加密范围。
const DATABASE_DOMAINS: [DataKeyDomain; 3] =
    [DataKeyDomain::Equipment, DataKeyDomain::UserLogin, DataKeyDomain::Credential];

pub(crate) async fn run(config_dir: &ConfigDir, command: KeysCommand) -> anyhow::Result<()> {
    match command {
        KeysCommand::Status { strict } => status(config_dir, strict).await,
        KeysCommand::Reencrypt(args) => reencrypt(config_dir, args).await,
    }
}

async fn status(config_dir: &ConfigDir, strict: bool) -> anyhow::Result<()> {
    let (config, store) =
        super::worker_store(config_dir, auction_storage::DatabaseConnectionPurpose::CliOperations)
            .await?;
    let result = async {
        let pool = store.postgres_pool();
        let mut domains = serde_json::Map::new();
        let mut failures = Vec::new();
        for domain in DATABASE_DOMAINS {
            let keyring = keyring(&config, domain)?;
            let stored = auction_storage::stored_key_ids(pool, domain)
                .await
                .with_context(|| format!("读取 {} 域信封 key ID 失败", domain.as_str()))?;
            let mut referenced = serde_json::Map::new();
            for (key_id, envelopes) in &stored {
                let resolvable = keyring.as_ref().is_some_and(|ring| ring.contains(key_id));
                if !resolvable {
                    failures.push(format!(
                        "{} 域仍有 {envelopes} 个信封引用未配置的 key {key_id}",
                        domain.as_str()
                    ));
                }
                referenced.insert(
                    key_id.clone(),
                    json!({ "envelopes": envelopes, "resolvable": resolvable }),
                );
            }
            let now = chrono::Utc::now();
            let previous = config
                .data_key_previous(domain)
                .iter()
                .map(|key| {
                    let still_referenced = stored.iter().any(|(id, _)| id == key.id());
                    if now >= key.retain_until() && still_referenced {
                        failures.push(format!(
                            "{} 域旧 key {} 已过 12 个月保留期但仍有信封引用；请先重加密",
                            domain.as_str(),
                            key.id()
                        ));
                    }
                    json!({
                        "key_id": key.id(),
                        "retired_at": key.retired_at().to_rfc3339(),
                        "retain_until": key.retain_until().to_rfc3339(),
                        "still_referenced": still_referenced,
                        "removable": !still_referenced && now >= key.retain_until(),
                    })
                })
                .collect::<Vec<_>>();
            domains.insert(
                domain.as_str().to_owned(),
                json!({
                    "configured": keyring.is_some(),
                    "current_key_id": keyring.as_ref().map(|ring| ring.current_id()),
                    "previous_keys": previous,
                    "database_envelopes": referenced,
                }),
            );
        }
        // 诊断归档使用独立域，但延用 credential/equipment 的历史文件；状态只报告配置。
        let diagnostics = keyring(&config, DataKeyDomain::MailDiagnostics)?;
        domains.insert(
            DataKeyDomain::MailDiagnostics.as_str().to_owned(),
            json!({
                "configured": diagnostics.is_some(),
                "current_key_id": diagnostics.as_ref().map(|ring| ring.current_id()),
                "previous_keys": config
                    .data_key_previous(DataKeyDomain::MailDiagnostics)
                    .iter()
                    .map(|key| json!({
                        "key_id": key.id(),
                        "retired_at": key.retired_at().to_rfc3339(),
                        "retain_until": key.retain_until().to_rfc3339(),
                    }))
                    .collect::<Vec<_>>(),
                "note": "诊断归档为一次写入文件，不重写；旧域 key 须保留到对应归档过期",
            }),
        );
        print_json(&json!({
            "strict": strict,
            "retention_months": auction_config::DATA_KEY_RETENTION_MONTHS,
            "domains": domains,
            "failures": failures,
        }))?;
        anyhow::ensure!(!strict || failures.is_empty(), "密钥环严格核对失败");
        Ok(())
    }
    .await;
    store.postgres_pool().close().await;
    result
}

async fn reencrypt(config_dir: &ConfigDir, args: KeysReencryptArgs) -> anyhow::Result<()> {
    let domains: Vec<DataKeyDomain> = match args.domain {
        Some(arg) => vec![arg.domain()],
        None => DATABASE_DOMAINS.to_vec(),
    };
    anyhow::ensure!(
        !domains.contains(&DataKeyDomain::MailDiagnostics),
        "mail_diagnostics 归档是一次写入的诊断证据，不重写文件；请重加密数据库信封并保留旧域 key 至归档过期"
    );
    if args.apply {
        anyhow::ensure!(args.receipt.is_some(), "--apply 必须同时提供 --receipt 回执路径");
        let actor = args.actor.as_deref().context("--apply 必须提供 --actor 审计操作者")?;
        anyhow::ensure!(!actor.trim().is_empty() && actor.len() <= 128, "--actor 无效");
    }
    let (config, store) =
        super::worker_store(config_dir, auction_storage::DatabaseConnectionPurpose::CliOperations)
            .await?;
    let result = async {
        let pool = store.postgres_pool();
        let started_at = chrono::Utc::now();
        let mut domains_report = serde_json::Map::new();
        let mut all_from: BTreeMap<String, i64> = BTreeMap::new();
        let mut applied_keys = Vec::new();
        for domain in domains {
            let keyring = keyring(&config, domain)?
                .with_context(|| format!("{} 域缺少当前数据密钥，无法重加密", domain.as_str()))?;
            let outcome =
                reencrypt_domain(&store, &keyring, domain, args.page_size, args.apply).await?;
            for (key_id, count) in &outcome.from_keys {
                *all_from.entry(key_id.clone()).or_default() += count;
            }
            domains_report.insert(
                domain.as_str().to_owned(),
                json!({
                    "current_key_id": keyring.current_id(),
                    "scanned": outcome.scanned,
                    "rewritten": outcome.rewritten,
                    "from_key_ids": outcome.from_keys,
                }),
            );
            applied_keys.push((domain, keyring.current_id().to_owned(), outcome));
        }
        let receipt_id = Uuid::new_v4();
        let receipt = json!({
            "format": RECEIPT_FORMAT,
            "receipt_id": receipt_id,
            "applied": args.apply,
            "started_at": started_at.to_rfc3339(),
            "finished_at": chrono::Utc::now().to_rfc3339(),
            "from_key_ids": all_from,
            "domains": domains_report,
            "operator": { "os_uid": files::effective_uid() },
        });
        let encoded = serde_json::to_vec_pretty(&receipt).context("序列化重加密回执失败")?;
        if let Some(path) = &args.receipt {
            files::write_private(path, &encoded)?;
        }
        if args.apply {
            let actor =
                args.actor.as_deref().ok_or_else(|| anyhow::anyhow!("apply 必须先校验 --actor"))?;
            let receipt_sha256 = auction_application::core::lower_hex(&Sha256::digest(&encoded));
            let identity = super::operator_identity::capture()?;
            for (domain, to_key_id, outcome) in applied_keys {
                let metadata = auction_application::core::cli_audit_metadata(
                    Some(&identity),
                    json!({
                        "to_key_id": to_key_id,
                        "from_key_ids": outcome.from_keys,
                        "scanned": outcome.scanned,
                        "rewritten": outcome.rewritten,
                        "receipt_id": receipt_id,
                        "receipt_sha256": receipt_sha256,
                        "actor": actor,
                    }),
                );
                auction_storage::append_reencrypt_audit(
                    pool,
                    domain,
                    "auction-cli:keys-reencrypt",
                    metadata,
                )
                .await
                .with_context(|| format!("追加 {} 域重加密审计失败", domain.as_str()))?;
            }
        }
        print_json(&receipt)
    }
    .await;
    store.postgres_pool().close().await;
    result
}

struct DomainOutcome {
    scanned: i64,
    rewritten: i64,
    from_keys: BTreeMap<String, i64>,
}

async fn reencrypt_domain(
    store: &Arc<auction_storage::DatabaseStore>,
    keyring: &Arc<Keyring>,
    domain: DataKeyDomain,
    page_size: i64,
    apply: bool,
) -> anyhow::Result<DomainOutcome> {
    let pool = store.postgres_pool();
    let mut outcome = DomainOutcome { scanned: 0, rewritten: 0, from_keys: BTreeMap::new() };
    let mut after: Option<Uuid> = None;
    loop {
        let page = auction_storage::scan_key_envelopes(
            pool,
            domain,
            keyring.current_id(),
            after,
            page_size,
        )
        .await
        .with_context(|| format!("扫描 {} 域信封失败", domain.as_str()))?;
        let Some(last) = page.last().map(|row| row.entity_id) else {
            break;
        };
        for row in page {
            outcome.scanned += 1;
            *outcome.from_keys.entry(row.encryption_key_id.clone()).or_default() += 1;
            let rotated = reseal(store, keyring, domain, &row).await.with_context(|| {
                format!(
                    "{} 域实体 {} 重加密失败（旧 key {}）",
                    domain.as_str(),
                    row.entity_id,
                    row.encryption_key_id
                )
            })?;
            if apply {
                let replaced = auction_storage::replace_key_envelope(
                    pool,
                    domain,
                    row.entity_id,
                    &row.encryption_key_id,
                    &rotated,
                )
                .await
                .with_context(|| format!("写入 {} 域重加密信封失败", domain.as_str()))?;
                anyhow::ensure!(
                    replaced,
                    "{} 域实体 {} 的信封在重加密期间被并发修改；请重新运行",
                    domain.as_str(),
                    row.entity_id
                );
                outcome.rewritten += 1;
            }
        }
        after = Some(last);
    }
    if domain == DataKeyDomain::UserLogin {
        reencrypt_admin_command_responses(store, keyring, page_size, apply, &mut outcome).await?;
    }
    Ok(outcome)
}

/// Admin 命令响应信封与登录 key 共用 `UserLogin` 域；它们同样必须在旧 key 退役前重加密，
/// 否则 `AdminUserService::replay` 会在命令重放时无法解密原结果。
async fn reencrypt_admin_command_responses(
    store: &Arc<auction_storage::DatabaseStore>,
    keyring: &Arc<Keyring>,
    page_size: i64,
    apply: bool,
    outcome: &mut DomainOutcome,
) -> anyhow::Result<()> {
    let pool = store.postgres_pool();
    let mut after: Option<Uuid> = None;
    loop {
        let page = auction_storage::scan_admin_command_key_envelopes(
            pool,
            keyring.current_id(),
            after,
            page_size,
        )
        .await
        .with_context(|| "扫描 user_login 域 Admin 命令响应信封失败")?;
        let Some(last) = page.last().map(|row| row.entity_id) else {
            break;
        };
        for row in page {
            outcome.scanned += 1;
            *outcome.from_keys.entry(row.encryption_key_id.clone()).or_default() += 1;
            let rotated = reseal(store, keyring, DataKeyDomain::UserLogin, &row)
                .await
                .with_context(|| {
                    format!(
                        "user_login 域 Admin 命令 {} 重加密失败（旧 key {}）",
                        row.entity_id, row.encryption_key_id
                    )
                })?;
            if apply {
                let replaced = auction_storage::replace_admin_command_key_envelope(
                    pool,
                    row.entity_id,
                    &row.encryption_key_id,
                    &rotated,
                )
                .await
                .with_context(|| "写入 user_login 域 Admin 命令响应信封失败")?;
                anyhow::ensure!(
                    replaced,
                    "user_login 域 Admin 命令 {} 的信封在重加密期间被并发修改；请重新运行",
                    row.entity_id
                );
                outcome.rewritten += 1;
            }
        }
        after = Some(last);
    }
    Ok(())
}

/// 用旧信封解密、用当前 key 重新封装，并用新信封重新解密核对。
async fn reseal(
    store: &Arc<auction_storage::DatabaseStore>,
    keyring: &Arc<Keyring>,
    domain: DataKeyDomain,
    row: &auction_storage::StoredKeyEnvelope,
) -> anyhow::Result<auction_storage::RotatedKeyEnvelope> {
    match domain {
        DataKeyDomain::Equipment => {
            let opener = EquipmentKeyOpener::new(Arc::clone(keyring));
            let sealer = EquipmentKeySealer::new(Arc::clone(keyring.current()));
            let equipment_id = EquipmentId::from_uuid(row.entity_id);
            let envelope = EquipmentKeyEnvelope::new(
                row.ciphertext.clone(),
                row.nonce.clone(),
                row.encryption_key_id.clone(),
                row.fingerprint.clone(),
            )
            .map_err(|error| {
                let _ = error;
                anyhow::anyhow!("装备信封结构无效")
            })?;
            let key = opener.open(equipment_id, &envelope).map_err(|error| {
                let _ = error;
                anyhow::anyhow!("装备 key 解密失败")
            })?;
            let resealed = sealer.seal(equipment_id, &key).map_err(|error| {
                let _ = error;
                anyhow::anyhow!("装备 key 重新加密失败")
            })?;
            let reopened = opener.open(equipment_id, &resealed).map_err(|error| {
                let _ = error;
                anyhow::anyhow!("新装备信封验证失败")
            })?;
            anyhow::ensure!(
                equipment_bytes(&key)? == equipment_bytes(&reopened)?,
                "装备 key 重加密后明文不一致"
            );
            Ok(auction_storage::RotatedKeyEnvelope {
                ciphertext: resealed.ciphertext().to_vec(),
                nonce: resealed.nonce().to_vec(),
                encryption_key_id: resealed.encryption_key_id().to_owned(),
                fingerprint: resealed.fingerprint().to_owned(),
            })
        }
        DataKeyDomain::UserLogin => {
            let reader = MoogleMailLoginKeyReader::new(Arc::clone(store), Arc::clone(keyring));
            let issuer = MoogleMailLoginKeyIssuer::new(Arc::clone(keyring.current()));
            let user_id = row.owner_user_id.unwrap_or(row.entity_id);
            let envelope = MailLoginKeyEnvelope::new(
                row.ciphertext.clone(),
                row.nonce.clone(),
                row.encryption_key_id.clone(),
                row.fingerprint.clone(),
            )
            .map_err(|error| {
                let _ = error;
                anyhow::anyhow!("登录 key 信封结构无效")
            })?;
            let key =
                AdminLoginKeyOpener::open(&reader, user_id, envelope).await.map_err(|error| {
                    let _ = error;
                    anyhow::anyhow!("登录 key 解密失败")
                })?;
            let issue =
                AdminLoginKeyIssuer::seal(&issuer, user_id, key).await.map_err(|error| {
                    let _ = error;
                    anyhow::anyhow!("登录 key 重新加密失败")
                })?;
            let verified = AdminLoginKeyOpener::open(&reader, user_id, issue.envelope.clone())
                .await
                .map_err(|error| {
                    let _ = error;
                    anyhow::anyhow!("新登录信封验证失败")
                })?;
            anyhow::ensure!(
                login_key_bytes(&issue.key) == login_key_bytes(&verified),
                "登录 key 重加密后明文不一致"
            );
            Ok(auction_storage::RotatedKeyEnvelope {
                ciphertext: issue.envelope.ciphertext().to_vec(),
                nonce: issue.envelope.nonce().to_vec(),
                encryption_key_id: issue.envelope.encryption_key_id().to_owned(),
                fingerprint: issue.envelope.fingerprint().to_owned(),
            })
        }
        DataKeyDomain::Credential => {
            let sealer = Arc::new(CredentialSealer::new(Arc::clone(keyring)));
            let kind = row
                .credential_kind
                .as_deref()
                .and_then(AdminCredentialKind::parse)
                .context("凭据类型未知")?;
            let scope = row.scope.as_deref().context("凭据 scope 缺失")?;
            let envelope = AdminCredentialEnvelope::new(
                row.ciphertext.clone(),
                row.nonce.clone(),
                row.encryption_key_id.clone(),
                row.fingerprint.clone(),
            )
            .map_err(|error| {
                let _ = error;
                anyhow::anyhow!("凭据信封结构无效")
            })?;
            let secret =
                AdminCredentialOpener::open(&*sealer, kind, row.entity_id, scope, &envelope)
                    .map_err(|error| {
                        let _ = error;
                        anyhow::anyhow!("凭据解密失败")
                    })?;
            let resealed =
                AdminCredentialSealer::seal(&*sealer, kind, row.entity_id, scope, &secret)
                    .map_err(|error| {
                        let _ = error;
                        anyhow::anyhow!("凭据重新加密失败")
                    })?;
            let verified =
                AdminCredentialOpener::open(&*sealer, kind, row.entity_id, scope, &resealed)
                    .map_err(|error| {
                        let _ = error;
                        anyhow::anyhow!("新凭据信封验证失败")
                    })?;
            anyhow::ensure!(secret == verified, "凭据重加密后明文不一致");
            Ok(auction_storage::RotatedKeyEnvelope {
                ciphertext: resealed.ciphertext().to_vec(),
                nonce: resealed.nonce().to_vec(),
                encryption_key_id: resealed.encryption_key_id().to_owned(),
                fingerprint: resealed.fingerprint().to_owned(),
            })
        }
        DataKeyDomain::MailDiagnostics => anyhow::bail!("mail_diagnostics 不支持数据库重加密"),
    }
}

fn equipment_bytes(key: &EquipmentKey) -> anyhow::Result<Vec<u8>> {
    key.with_bytes(<[u8]>::to_vec).map_err(|error| {
        let _ = error;
        anyhow::anyhow!("装备 key 已清理")
    })
}

fn login_key_bytes(key: &MailLoginKey) -> Vec<u8> {
    key.with_encoded(|encoded| encoded.as_bytes().to_vec())
}

fn keyring(config: &AppConfig, domain: DataKeyDomain) -> anyhow::Result<Option<Arc<Keyring>>> {
    build_keyring(
        domain,
        config.data_key_current(domain),
        config.data_key_previous(domain).iter().map(|key| (key.id(), key.material())),
    )
    .map_err(|error| anyhow::anyhow!("{} 域密钥环无效：{error}", domain.as_str()))
}
