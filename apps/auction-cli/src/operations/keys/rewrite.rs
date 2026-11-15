//! 单个数据密钥域的分页扫描、重新封装与 CAS 写回。
//!
//! 不变量：每条信封先用旧 key 解密、以当前 key 重新封装并用新信封再解密核对，`--apply` 时
//! 才以旧 key ID 作 CAS 条件写回。已提交的改写计数逐条累计在调用方持有的 [`DomainOutcome`]
//! 中，中途失败也不会丢失，调用方据此写部分回执与审计。

use std::{collections::BTreeMap, sync::Arc};

use anyhow::Context;
use auction_application::admin::{
    AdminCredentialEnvelope, AdminCredentialKind, AdminCredentialOpener, AdminCredentialSealer,
    AdminLoginKeyIssuer, AdminLoginKeyOpener,
};
use auction_application::mail::{MailLoginKey, MailLoginKeyEnvelope};
use auction_config::DataKeyDomain;
use auction_domain::{EquipmentId, EquipmentKey, EquipmentKeyEnvelope};
use auction_integrations::{
    CredentialSealer, EquipmentKeyOpener, EquipmentKeySealer, Keyring, MoogleMailLoginKeyIssuer,
    MoogleMailLoginKeyReader,
};
use auction_storage::DatabaseStore;
use serde::Serialize;
use uuid::Uuid;

/// 单个域的累计结果；`rewritten` 只计已提交的 CAS 写回。
#[derive(Debug, Default)]
pub(super) struct DomainOutcome {
    pub(super) scanned: i64,
    pub(super) rewritten: i64,
    pub(super) from_keys: BTreeMap<String, i64>,
}

/// 重加密中止的阶段；回执与审计只记录这个固定分类，完整原因只出现在 stderr。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum FailureStage {
    /// 读取信封页失败。
    Scan,
    /// 解密、重新封装或复核明文失败。
    Reseal,
    /// 写回信封失败；该条写入的结果可能未知。
    Write,
    /// CAS 未命中：信封在重加密期间被并发修改。
    Conflict,
}

impl FailureStage {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Scan => "scan",
            Self::Reseal => "reseal",
            Self::Write => "write",
            Self::Conflict => "conflict",
        }
    }
}

/// 带阶段分类的域失败；`error` 保留完整原因供 stderr 输出。
#[derive(Debug)]
pub(super) struct DomainFailure {
    pub(super) stage: FailureStage,
    pub(super) error: anyhow::Error,
}

/// 给错误标注中止阶段。
trait AtStage<T> {
    fn at(self, stage: FailureStage) -> Result<T, DomainFailure>;
}

impl<T> AtStage<T> for anyhow::Result<T> {
    fn at(self, stage: FailureStage) -> Result<T, DomainFailure> {
        self.map_err(|error| DomainFailure { stage, error })
    }
}

/// 扫描并重加密一个域；计数累计在 `outcome` 中，失败时其中的 `rewritten` 就是已提交的改写数。
pub(super) async fn reencrypt_domain(
    store: &Arc<DatabaseStore>,
    keyring: &Arc<Keyring>,
    domain: DataKeyDomain,
    page_size: i64,
    apply: bool,
    outcome: &mut DomainOutcome,
) -> Result<(), DomainFailure> {
    let pool = store.postgres_pool();
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
        .with_context(|| format!("扫描 {} 域信封失败", domain.as_str()))
        .at(FailureStage::Scan)?;
        let Some(last) = page.last().map(|row| row.entity_id) else {
            break;
        };
        for row in page {
            outcome.scanned += 1;
            *outcome.from_keys.entry(row.encryption_key_id.clone()).or_default() += 1;
            let rotated = reseal(store, keyring, domain, &row)
                .await
                .with_context(|| {
                    format!(
                        "{} 域实体 {} 重加密失败（旧 key {}）",
                        domain.as_str(),
                        row.entity_id,
                        row.encryption_key_id
                    )
                })
                .at(FailureStage::Reseal)?;
            if apply {
                let replaced = auction_storage::replace_key_envelope(
                    pool,
                    domain,
                    row.entity_id,
                    &row.encryption_key_id,
                    &rotated,
                )
                .await
                .with_context(|| format!("写入 {} 域重加密信封失败", domain.as_str()))
                .at(FailureStage::Write)?;
                if !replaced {
                    return Err(DomainFailure {
                        stage: FailureStage::Conflict,
                        error: anyhow::anyhow!(
                            "{} 域实体 {} 的信封在重加密期间被并发修改；请重新运行",
                            domain.as_str(),
                            row.entity_id
                        ),
                    });
                }
                outcome.rewritten += 1;
            }
        }
        after = Some(last);
    }
    if domain == DataKeyDomain::UserLogin {
        reencrypt_admin_command_responses(store, keyring, page_size, apply, outcome).await?;
    }
    Ok(())
}

/// Admin 命令响应信封与登录 key 共用 `UserLogin` 域；它们同样必须在旧 key 退役前重加密，
/// 否则 `AdminUserService::replay` 会在命令重放时无法解密原结果。
async fn reencrypt_admin_command_responses(
    store: &Arc<DatabaseStore>,
    keyring: &Arc<Keyring>,
    page_size: i64,
    apply: bool,
    outcome: &mut DomainOutcome,
) -> Result<(), DomainFailure> {
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
        .context("扫描 user_login 域 Admin 命令响应信封失败")
        .at(FailureStage::Scan)?;
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
                })
                .at(FailureStage::Reseal)?;
            if apply {
                let replaced = auction_storage::replace_admin_command_key_envelope(
                    pool,
                    row.entity_id,
                    &row.encryption_key_id,
                    &rotated,
                )
                .await
                .context("写入 user_login 域 Admin 命令响应信封失败")
                .at(FailureStage::Write)?;
                if !replaced {
                    return Err(DomainFailure {
                        stage: FailureStage::Conflict,
                        error: anyhow::anyhow!(
                            "user_login 域 Admin 命令 {} 的信封在重加密期间被并发修改；请重新运行",
                            row.entity_id
                        ),
                    });
                }
                outcome.rewritten += 1;
            }
        }
        after = Some(last);
    }
    Ok(())
}

/// 用旧信封解密、用当前 key 重新封装，并用新信封重新解密核对。
async fn reseal(
    store: &Arc<DatabaseStore>,
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
            .ok()
            .context("装备信封结构无效")?;
            let key = opener.open(equipment_id, &envelope).ok().context("装备 key 解密失败")?;
            let resealed = sealer.seal(equipment_id, &key).ok().context("装备 key 重新加密失败")?;
            let reopened =
                opener.open(equipment_id, &resealed).ok().context("新装备信封验证失败")?;
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
            .ok()
            .context("登录 key 信封结构无效")?;
            let key = AdminLoginKeyOpener::open(&reader, user_id, envelope)
                .await
                .ok()
                .context("登录 key 解密失败")?;
            let issue = AdminLoginKeyIssuer::seal(&issuer, user_id, key)
                .await
                .ok()
                .context("登录 key 重新加密失败")?;
            let verified = AdminLoginKeyOpener::open(&reader, user_id, issue.envelope.clone())
                .await
                .ok()
                .context("新登录信封验证失败")?;
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
            .ok()
            .context("凭据信封结构无效")?;
            let secret =
                AdminCredentialOpener::open(&*sealer, kind, row.entity_id, scope, &envelope)
                    .ok()
                    .context("凭据解密失败")?;
            let resealed =
                AdminCredentialSealer::seal(&*sealer, kind, row.entity_id, scope, &secret)
                    .ok()
                    .context("凭据重新加密失败")?;
            let verified =
                AdminCredentialOpener::open(&*sealer, kind, row.entity_id, scope, &resealed)
                    .ok()
                    .context("新凭据信封验证失败")?;
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
    key.with_bytes(<[u8]>::to_vec).ok().context("装备 key 已清理")
}

fn login_key_bytes(key: &MailLoginKey) -> Vec<u8> {
    key.with_encoded(|encoded| encoded.as_bytes().to_vec())
}
