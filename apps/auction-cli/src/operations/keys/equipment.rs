//! 装备域三种来源的重封、解密复核与退休前的全域检查。

use std::sync::Arc;

use anyhow::Context;
use auction_application::consignment::{
    ListingCredentialError, RegistrationReplayCrypto, WrappedRegistrationReplayKey,
};
use auction_config::DataKeyDomain;
use auction_domain::{EquipmentId, EquipmentKey, EquipmentKeyEnvelope};
use auction_integrations::{
    EquipmentKeyOpener, EquipmentKeyOpenerError, EquipmentKeySealer, Keyring,
};
use auction_storage::{DatabaseStore, RotatedKeyEnvelope, StoredKeyEnvelope};

/// 用当前钥重封现有信封，并验证新信封。
pub(super) fn reseal(
    keyring: &Arc<Keyring>,
    row: &StoredKeyEnvelope,
) -> anyhow::Result<RotatedKeyEnvelope> {
    let opener = EquipmentKeyOpener::new(Arc::clone(keyring));
    if let Some(replay_key_id) = row.replay_key_id() {
        // 不变量 1、7：重包并复核同一逻辑钥，绑定其 ID，账本无需重算。
        let replacement = opener.rewrap(replay_key_id, &wrapped(row)?)?;
        return Ok(RotatedKeyEnvelope {
            ciphertext: replacement.wrapped_key().to_vec(),
            nonce: replacement.nonce().to_vec(),
            encryption_key_id: replacement.encryption_key_id().to_owned(),
            fingerprint: String::new(),
        });
    }

    let equipment_id = equipment_aad(row)?;
    let sealer = EquipmentKeySealer::new(Arc::clone(keyring.current()));
    let key = open_equipment(&opener, equipment_id, &envelope(row)?)?;
    let resealed = sealer.seal(equipment_id, &key).context("装备凭证重新封装失败")?;
    let reopened = open_equipment(&opener, equipment_id, &resealed)?;
    let matches = key.with_bytes(|expected| reopened.with_bytes(|actual| expected == actual))??;
    anyhow::ensure!(matches, "装备凭证重封前后不一致");
    Ok(RotatedKeyEnvelope {
        ciphertext: resealed.ciphertext().to_vec(),
        nonce: resealed.nonce().to_vec(),
        encryption_key_id: resealed.encryption_key_id().to_owned(),
        fingerprint: resealed.fingerprint().to_owned(),
    })
}

/// 用给定密钥环认证信封与其在原表中的身份。
pub(super) fn verify(opener: &EquipmentKeyOpener, row: &StoredKeyEnvelope) -> anyhow::Result<()> {
    if let Some(replay_key_id) = row.replay_key_id() {
        opener.check_wrapped(replay_key_id, &wrapped(row)?)?;
    } else {
        open_equipment(opener, equipment_aad(row)?, &envelope(row)?)?;
    }
    Ok(())
}

fn equipment_aad(row: &StoredKeyEnvelope) -> anyhow::Result<EquipmentId> {
    row.equipment_aad().map(EquipmentId::from_uuid).context("装备凭证缺少认证身份")
}

fn open_equipment(
    opener: &EquipmentKeyOpener,
    equipment_id: EquipmentId,
    envelope: &EquipmentKeyEnvelope,
) -> Result<EquipmentKey, ListingCredentialError> {
    opener.open(equipment_id, envelope).map_err(credential_unavailable)
}

fn credential_unavailable(_error: EquipmentKeyOpenerError) -> ListingCredentialError {
    ListingCredentialError
}

fn envelope(row: &StoredKeyEnvelope) -> anyhow::Result<EquipmentKeyEnvelope> {
    EquipmentKeyEnvelope::new(
        row.ciphertext.clone(),
        row.nonce.clone(),
        row.encryption_key_id.clone(),
        row.fingerprint.clone(),
    )
    .context("装备凭证信封结构无效")
}

fn wrapped(row: &StoredKeyEnvelope) -> anyhow::Result<WrappedRegistrationReplayKey> {
    let nonce = row.nonce.as_slice().try_into().context("比较钥包装的随机数长度无效")?;
    WrappedRegistrationReplayKey::new(row.ciphertext.clone(), nonce, row.encryption_key_id.clone())
        .context("比较钥包装结构无效")
}

/// 任一旧引用均阻止退休，即使旧钥仍在密钥环中。
pub(super) fn require_zero_old_references(
    stored: &[(String, i64)],
    current_key_id: &str,
) -> anyhow::Result<()> {
    let remaining = stored
        .iter()
        .filter(|(key_id, _)| key_id != current_key_id)
        .map(|(_, count)| count)
        .sum::<i64>();
    anyhow::ensure!(remaining == 0, "装备域仍有 {remaining} 个信封引用旧钥，请先完成重加密");
    Ok(())
}

/// 在停机窗口内检查零旧引用，并用只含当前钥的密钥环复核全部来源。
pub(super) async fn verify_current(
    store: &DatabaseStore,
    keyring: &Keyring,
) -> anyhow::Result<serde_json::Value> {
    let pool = store.postgres_pool();
    let stored = auction_storage::stored_key_ids(pool, DataKeyDomain::Equipment).await?;
    require_zero_old_references(&stored, keyring.current_id())?;
    let current_only = Keyring::single(DataKeyDomain::Equipment, Arc::clone(keyring.current()));
    let opener = EquipmentKeyOpener::new(Arc::new(current_only));
    let mut counts = StoredKeyEnvelope::EQUIPMENT_SOURCES
        .into_iter()
        .map(|source| (source.as_str(), 0_u64))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut after = None;
    loop {
        let page = StoredKeyEnvelope::scan_equipment_for_verification(
            pool,
            after,
            auction_storage::KEY_ROTATION_PAGE_SIZE,
        )
        .await?;
        let Some(last) = page.last() else { break };
        after = Some(last.cursor());
        for row in page {
            let source = row.source().as_str();
            verify(&opener, &row).with_context(|| format!("{source} 当前钥开封复核失败"))?;
            *counts.entry(source).or_default() += 1;
        }
    }
    Ok(serde_json::json!({ "old_references": 0, "verified": counts }))
}
