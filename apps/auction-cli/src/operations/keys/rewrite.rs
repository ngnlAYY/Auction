//! 单个数据密钥域的分页扫描、重新封装与 CAS 写回。
//!
//! 不变量：每条信封先用旧 key 解密、以当前 key 重新封装并用新信封再解密核对，`--apply` 时
//! 才以原信封为 CAS 条件写回。已提交的改写计数逐条累计在调用方持有的 [`DomainOutcome`]
//! 中，中途失败也不会丢失，调用方据此写部分回执与审计。

use std::{collections::BTreeMap, sync::Arc};

use anyhow::Context;
use auction_application::admin::{
    AdminCredentialEnvelope, AdminCredentialKind, AdminCredentialOpener, AdminCredentialSealer,
    AdminLoginKeyIssuer, AdminLoginKeyOpener,
};
use auction_application::mail::{MailLoginKey, MailLoginKeyEnvelope};
use auction_config::DataKeyDomain;
use auction_integrations::{
    ConsignmentSecretEnvelope, ConsignmentSecretOpener, ConsignmentSecretSealer, CredentialSealer,
    EquipmentKeyOpener, Keyring, MailEvidencePurpose, MoogleMailLoginKeyIssuer,
    MoogleMailLoginKeyReader,
};
use auction_storage::{DatabaseStore, KeyRotationError, StoredKeyEnvelope};
use serde::Serialize;
use uuid::Uuid;

use super::equipment;

/// 单个域的累计结果；`rewritten` 只计已提交的 CAS 写回。
#[derive(Debug, Default)]
pub(super) struct DomainOutcome {
    pub(super) scanned: i64,
    pub(super) rewritten: i64,
    pub(super) sources: BTreeMap<&'static str, SourceOutcome>,
}

/// 单一来源表的计数，计入演练和部分执行的回执。
#[derive(Debug, Default, Serialize)]
pub(super) struct SourceOutcome {
    pub(super) scanned: i64,
    pub(super) rewritten: i64,
}

impl DomainOutcome {
    /// 初始化该域全部来源，空表也写零计数。
    pub(super) fn new(domain: DataKeyDomain) -> Self {
        let mut outcome = Self::default();
        match domain {
            DataKeyDomain::Equipment => {
                for source in StoredKeyEnvelope::EQUIPMENT_SOURCES {
                    outcome.sources.entry(source.as_str()).or_default();
                }
            }
            DataKeyDomain::Credential => {
                for source in StoredKeyEnvelope::CREDENTIAL_SOURCES {
                    outcome.sources.entry(source.as_str()).or_default();
                }
            }
            DataKeyDomain::UserLogin | DataKeyDomain::MailDiagnostics => {}
        }
        outcome
    }

    /// 记录一条已进入处理的候选信封。
    pub(super) fn scanned(&mut self, source: &'static str) {
        self.scanned += 1;
        self.sources.entry(source).or_default().scanned += 1;
    }

    /// 只记录本进程已确认提交的写回。
    pub(super) fn rewritten(&mut self, source: &'static str) {
        self.rewritten += 1;
        self.sources.entry(source).or_default().rewritten += 1;
    }
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
    let mut after = None;
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
        let Some(last) = page.last().map(StoredKeyEnvelope::cursor) else {
            break;
        };
        for row in page {
            let source = row.source().as_str();
            outcome.scanned(source);
            let rotated = reseal(store, keyring, domain, &row)
                .await
                .with_context(|| format!("{source} 信封重加密失败"))
                .at(FailureStage::Reseal)?;
            apply_if_requested(apply, async || {
                let result = auction_storage::replace_key_envelope(pool, &row, &rotated).await;
                let replaced = match result {
                    Err(KeyRotationError::Conflict) => {
                        result.context(format!("{source} 信封冲突")).at(FailureStage::Conflict)?
                    }
                    _ => result
                        .with_context(|| format!("写入 {source} 重加密信封失败"))
                        .at(FailureStage::Write)?,
                };
                if replaced.was_replaced() {
                    outcome.rewritten(source);
                } else if let Some(current) = replaced.current_envelope() {
                    let opener = EquipmentKeyOpener::new(Arc::clone(keyring));
                    equipment::verify(&opener, current)
                        .with_context(|| format!("{source} 并发换钥后的开封复核失败"))
                        .at(FailureStage::Reseal)?;
                }
                Ok(())
            })
            .await?;
        }
        after = Some(last);
    }
    if apply && domain == DataKeyDomain::Equipment {
        let stored = auction_storage::stored_key_ids(pool, domain)
            .await
            .context("收口装备域旧钥引用失败")
            .at(FailureStage::Scan)?;
        equipment::require_zero_old_references(&stored, keyring.current_id())
            .at(FailureStage::Conflict)?;
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
            let source = row.source().as_str();
            outcome.scanned(source);
            let rotated = reseal(store, keyring, DataKeyDomain::UserLogin, &row)
                .await
                .context("Admin 命令响应信封重加密失败")
                .at(FailureStage::Reseal)?;
            apply_if_requested(apply, async || {
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
                            "Admin 命令响应信封在重加密期间被并发修改，请重新运行"
                        ),
                    });
                }
                outcome.rewritten(source);
                Ok(())
            })
            .await?;
        }
        after = Some(last);
    }
    Ok(())
}

/// 不变量 7、8：演练不进入实际写入边界，审计只由实际写入流程追加。
pub(super) async fn apply_if_requested(
    apply: bool,
    write: impl AsyncFnOnce() -> Result<(), DomainFailure>,
) -> Result<(), DomainFailure> {
    if apply {
        write().await?;
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
        DataKeyDomain::Equipment => equipment::reseal(keyring, row),
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
            let source = row.source().as_str();
            if matches!(source, "consignment_reservation_secrets" | "mail_evidence_envelopes") {
                let envelope = ConsignmentSecretEnvelope::new(
                    row.ciphertext.clone(),
                    row.nonce.clone(),
                    row.encryption_key_id.clone(),
                    row.fingerprint.clone(),
                )
                .map_err(|error| anyhow::anyhow!("寄售密文信封结构无效（{error:?}）"))?;
                return reseal_consignment_secret(
                    keyring,
                    ConsignmentSecretRow {
                        source,
                        entity_id: row.entity_id,
                        generation: row.generation,
                        seller_user_id: row.seller_user_id,
                        purpose: row.purpose.as_deref(),
                        aad_owner: row.aad_owner.as_deref(),
                        envelope,
                    },
                );
            }
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

/// 从 `StoredKeyEnvelope` 抽出的寄售密文行；信封与用途字段必须成对出现。
struct ConsignmentSecretRow<'a> {
    source: &'a str,
    entity_id: Uuid,
    generation: Option<i64>,
    seller_user_id: Option<Uuid>,
    purpose: Option<&'a str>,
    aad_owner: Option<&'a str>,
    envelope: ConsignmentSecretEnvelope,
}

/// 重封寄售预约码或受保护证据；AAD 身份只来自加密时冻结的列。
fn reseal_consignment_secret(
    keyring: &Arc<Keyring>,
    row: ConsignmentSecretRow<'_>,
) -> anyhow::Result<auction_storage::RotatedKeyEnvelope> {
    let opener = ConsignmentSecretOpener::new(Arc::clone(keyring));
    let sealer = ConsignmentSecretSealer::new(Arc::clone(keyring.current()));
    match row.source {
        "consignment_reservation_secrets" => {
            let generation = row
                .generation
                .context("预约密文缺少代")?
                .try_into()
                .context("预约密文代超出范围")?;
            let seller_user_id = row.seller_user_id.context("预约密文缺少卖家")?;
            let token = opener
                .open_reservation(row.entity_id, generation, seller_user_id, &row.envelope)
                .map_err(|error| anyhow::anyhow!("预约码解密失败（{error:?}）"))?;
            let resealed = sealer
                .seal_reservation(row.entity_id, generation, seller_user_id, &token)
                .map_err(|error| anyhow::anyhow!("预约码重新封装失败（{error:?}）"))?;
            let verified = opener
                .open_reservation(row.entity_id, generation, seller_user_id, &resealed)
                .map_err(|error| anyhow::anyhow!("新预约码信封验证失败（{error:?}）"))?;
            anyhow::ensure!(token.as_str() == verified.as_str(), "预约码重封前后不一致");
            Ok(auction_storage::RotatedKeyEnvelope {
                ciphertext: resealed.ciphertext().to_vec(),
                nonce: resealed.nonce().to_vec(),
                encryption_key_id: resealed.encryption_key_id().to_owned(),
                fingerprint: resealed.fingerprint().to_owned(),
            })
        }
        "mail_evidence_envelopes" => {
            let purpose = parse_evidence_purpose(row.purpose.context("受保护证据缺少用途")?)?;
            let aad_owner = row.aad_owner.context("受保护证据缺少 owner")?;
            let plaintext = opener
                .open_evidence(row.entity_id, purpose, aad_owner, &row.envelope)
                .map_err(|error| anyhow::anyhow!("受保护证据解密失败（{error:?}）"))?;
            let resealed = sealer
                .seal_evidence(row.entity_id, purpose, aad_owner, &plaintext)
                .map_err(|error| anyhow::anyhow!("受保护证据重新封装失败（{error:?}）"))?;
            let verified = opener
                .open_evidence(row.entity_id, purpose, aad_owner, &resealed)
                .map_err(|error| anyhow::anyhow!("新受保护证据信封验证失败（{error:?}）"))?;
            anyhow::ensure!(plaintext == verified, "受保护证据重封前后不一致");
            Ok(auction_storage::RotatedKeyEnvelope {
                ciphertext: resealed.ciphertext().to_vec(),
                nonce: resealed.nonce().to_vec(),
                encryption_key_id: resealed.encryption_key_id().to_owned(),
                fingerprint: resealed.fingerprint().to_owned(),
            })
        }
        _ => anyhow::bail!("非寄售密文来源不能走寄售重封"),
    }
}

fn parse_evidence_purpose(value: &str) -> anyhow::Result<MailEvidencePurpose> {
    match value {
        "manifest" => Ok(MailEvidencePurpose::Manifest),
        "equipment_credential" => Ok(MailEvidencePurpose::EquipmentCredential),
        "original_observation" => Ok(MailEvidencePurpose::OriginalObservation),
        "quarantine_evidence" => Ok(MailEvidencePurpose::QuarantineEvidence),
        _ => anyhow::bail!("受保护证据用途未知"),
    }
}

fn login_key_bytes(key: &MailLoginKey) -> Vec<u8> {
    key.with_encoded(|encoded| encoded.as_bytes().to_vec())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use auction_config::DataKeyDomain;
    use auction_integrations::{
        ConsignmentSecretEnvelope, ConsignmentSecretOpener, ConsignmentSecretSealer, DataKey,
        Keyring, MailEvidencePurpose,
    };
    use auction_time::Timestamp;
    use uuid::Uuid;

    use super::ConsignmentSecretRow;
    use super::DomainOutcome;
    use super::parse_evidence_purpose;
    use super::reseal_consignment_secret;

    const OLD_KEY_ID: &str = "fixture-consignment-old";
    const CURRENT_KEY_ID: &str = "fixture-consignment-current";
    const OLD_MATERIAL: &str = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";
    const CURRENT_MATERIAL: &str = "AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI=";

    fn ring() -> Arc<Keyring> {
        let old = Arc::new(DataKey::from_base64(OLD_KEY_ID, OLD_MATERIAL).expect("旧寄售钥"));
        let current =
            Arc::new(DataKey::from_base64(CURRENT_KEY_ID, CURRENT_MATERIAL).expect("当前寄售钥"));
        Arc::new(
            Keyring::new(DataKeyDomain::Credential, current, vec![old]).expect("凭据域寄售密钥环"),
        )
    }

    /// 凭据域回执固定列出三处来源，即使新增表为空也不能省略计数。
    #[test]
    fn credential_receipt_initializes_all_persisted_sources() {
        let outcome = DomainOutcome::new(DataKeyDomain::Credential);
        let sources: Vec<_> = outcome.sources.keys().copied().collect();
        assert_eq!(
            sources,
            vec!["consignment_reservation_secrets", "credential_keys", "mail_evidence_envelopes",]
        );
        assert!(
            outcome.sources.values().all(|source| { source.scanned == 0 && source.rewritten == 0 })
        );
    }

    /// 装备域仍只初始化既有三处来源，不因新寄售密文改变回执形状。
    #[test]
    fn equipment_receipt_keeps_three_sources() {
        let outcome = DomainOutcome::new(DataKeyDomain::Equipment);
        assert_eq!(
            outcome.sources.keys().copied().collect::<Vec<_>>(),
            vec![
                "consignment_registration_secrets",
                "consignment_replay_keys",
                "equipment_secrets",
            ]
        );
    }

    /// 受保护证据用途是闭集，未知值失败关闭。
    #[test]
    fn evidence_purpose_parser_is_closed() {
        assert_eq!(
            parse_evidence_purpose("manifest").expect("清单"),
            MailEvidencePurpose::Manifest
        );
        assert_eq!(
            parse_evidence_purpose("equipment_credential").expect("装备引用"),
            MailEvidencePurpose::EquipmentCredential
        );
        assert_eq!(
            parse_evidence_purpose("original_observation").expect("原观察"),
            MailEvidencePurpose::OriginalObservation
        );
        assert_eq!(
            parse_evidence_purpose("quarantine_evidence").expect("隔离证据"),
            MailEvidencePurpose::QuarantineEvidence
        );
        assert!(parse_evidence_purpose("unknown").is_err());
    }

    /// 两类寄售密文都按原 AAD 解开、以当前钥重封，并复核明文未变。
    #[test]
    fn consignment_secret_reencrypt_keeps_aad_and_plaintext() {
        let ring = ring();
        let old_key = Arc::new(DataKey::from_base64(OLD_KEY_ID, OLD_MATERIAL).expect("旧寄售钥"));
        let sealer = ConsignmentSecretSealer::new(old_key);
        let opener = ConsignmentSecretOpener::new(Arc::clone(&ring));
        let reservation_id = Uuid::from_u128(0x910);
        let seller_id = Uuid::from_u128(0x911);
        let token = auction_domain::consignment::format_token([0x91; 32]);
        let reservation_envelope =
            sealer.seal_reservation(reservation_id, 4, seller_id, &token).expect("封装备预约");
        let rotated = reseal_consignment_secret(
            &ring,
            ConsignmentSecretRow {
                source: "consignment_reservation_secrets",
                entity_id: reservation_id,
                generation: Some(4),
                seller_user_id: Some(seller_id),
                purpose: None,
                aad_owner: None,
                envelope: reservation_envelope,
            },
        )
        .expect("重封预约");
        assert_eq!(rotated.encryption_key_id, CURRENT_KEY_ID);
        let reopened = ConsignmentSecretEnvelope::new(
            rotated.ciphertext.clone(),
            rotated.nonce.clone(),
            rotated.encryption_key_id.clone(),
            rotated.fingerprint.clone(),
        )
        .expect("新预约信封");
        assert_eq!(
            opener
                .open_reservation(reservation_id, 4, seller_id, &reopened)
                .expect("打开新预约")
                .as_str(),
            token.as_str()
        );

        let evidence_id = Uuid::from_u128(0x912);
        let evidence_envelope = sealer
            .seal_evidence(
                evidence_id,
                MailEvidencePurpose::Manifest,
                "fixture-owner",
                b"fixture evidence",
            )
            .expect("封装备证据");
        let rotated = reseal_consignment_secret(
            &ring,
            ConsignmentSecretRow {
                source: "mail_evidence_envelopes",
                entity_id: evidence_id,
                generation: None,
                seller_user_id: None,
                purpose: Some("manifest"),
                aad_owner: Some("fixture-owner"),
                envelope: evidence_envelope,
            },
        )
        .expect("重封证据");
        assert_eq!(rotated.encryption_key_id, CURRENT_KEY_ID);
        let reopened = ConsignmentSecretEnvelope::new(
            rotated.ciphertext.clone(),
            rotated.nonce.clone(),
            rotated.encryption_key_id.clone(),
            rotated.fingerprint.clone(),
        )
        .expect("新证据信封");
        assert_eq!(
            opener
                .open_evidence(
                    evidence_id,
                    MailEvidencePurpose::Manifest,
                    "fixture-owner",
                    &reopened,
                )
                .expect("打开新证据"),
            b"fixture evidence"
        );
    }

    /// 凭据域回执与审计携带三处来源计数，但不携带任何 key 标识。
    #[test]
    fn credential_receipt_and_audit_have_no_key_ids() {
        let mut outcome = DomainOutcome::new(DataKeyDomain::Credential);
        outcome.scanned("credential_keys");
        outcome.rewritten("credential_keys");
        outcome.scanned("consignment_reservation_secrets");
        outcome.rewritten("consignment_reservation_secrets");
        outcome.scanned("mail_evidence_envelopes");
        let item = super::super::ProcessedDomain {
            domain: DataKeyDomain::Credential,
            outcome,
            failure_stage: None,
        };
        let receipt_id = Uuid::new_v4();
        let document = super::super::receipt_document(
            receipt_id,
            true,
            Timestamp::now(),
            std::slice::from_ref(&item),
            None,
        );
        let domain = &document["domains"]["credential"];
        let counts = &domain["sources"];
        assert_eq!(
            counts,
            &serde_json::json!({
                "consignment_reservation_secrets": {"scanned": 1, "rewritten": 1},
                "credential_keys": {"scanned": 1, "rewritten": 1},
                "mail_evidence_envelopes": {"scanned": 1, "rewritten": 0},
            })
        );
        let audit = super::super::audit_details(&item, "ops", receipt_id, "digest", true);
        assert_eq!(&audit["sources"], counts);
        for document in [&document, &audit] {
            let text = serde_json::to_string(document).expect("序列化回执或审计");
            assert!(!text.contains("encryption_key_id"));
            assert!(!text.contains(OLD_KEY_ID));
            assert!(!text.contains(CURRENT_KEY_ID));
        }
    }
}
