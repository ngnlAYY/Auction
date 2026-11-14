//! 数据密钥与装备 key 的加密信封适配器；应用层只得到密文信封，明文
//! key 用后由领域值对象清零。

use std::fmt;
use std::sync::Arc;

use auction_domain::{EquipmentId, EquipmentKey, EquipmentKeyEnvelope};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use ring::{aead, hmac, rand::SystemRandom};
use thiserror::Error;

use crate::keyring::{Keyring, KeyringError};
use crate::safe_diagnostics;
use envelope::{OpenError, SealError, SecretPurpose, open_secret, seal_secret};

pub(crate) mod envelope;

const DATA_KEY_LENGTH: usize = 32;
const DATA_KEY_FINGERPRINT_DOMAIN: &[u8] = b"auction/cryptobox/data-key/v1";
const AAD_PREFIX: &str = "auction/equipment-key/v1/";

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum EquipmentKeySealerError {
    #[error("装备数据密钥标识无效")]
    InvalidKeyId,
    #[error("装备数据密钥必须是 32 字节的 Base64 材料")]
    InvalidDataKey,
    #[error("装备 key 加密器尚未配置")]
    Unconfigured,
    #[error("装备 key 加密附加认证数据无效")]
    InvalidAssociatedData,
    #[error("装备 key 加密随机源不可用")]
    RandomSource,
    #[error("装备 key 加密失败")]
    Encryption,
}

/// 装备 key 解密器错误；错误信息不包含密钥、明文或信封内容。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum EquipmentKeyOpenerError {
    #[error("装备 key 解密关联的装备 ID 无效")]
    InvalidEquipmentId,
    #[error("装备 key 密文信封结构无效")]
    InvalidEnvelope,
    #[error("装备 key 数据密钥标识不匹配")]
    KeyMismatch,
    #[error("装备 key 解密认证失败")]
    Decryption,
    #[error("装备 key 指纹校验失败")]
    FingerprintMismatch,
    #[error("装备 key 明文不是有效的 UTF-8")]
    InvalidUtf8,
    #[error("装备 key 明文结构无效")]
    InvalidPlaintext,
}

/// 不透明的数据密钥；调试、显示和序列化永远不输出材料。
pub struct DataKey {
    id: String,
    fingerprint: String,
    material: [u8; DATA_KEY_LENGTH],
}

impl DataKey {
    /// 从敏感 TOML 中的标识和标准 Base64 材料建立 AES-256 数据密钥。
    pub fn from_base64(id: &str, encoded: &str) -> Result<Self, EquipmentKeySealerError> {
        if !auction_domain::valid_key_id(id) {
            return Err(EquipmentKeySealerError::InvalidKeyId);
        }
        // 固定输出容量，畸形或过长配置不能先分配与输入等大的秘密缓冲。
        let mut material = [0_u8; DATA_KEY_LENGTH];
        if !BASE64
            .decode_slice(encoded, &mut material)
            .is_ok_and(|length| length == DATA_KEY_LENGTH)
        {
            material.fill(0);
            return Err(EquipmentKeySealerError::InvalidDataKey);
        }
        let key = hmac::Key::new(hmac::HMAC_SHA256, &material);
        let fingerprint = auction_application::core::lower_hex(
            hmac::sign(&key, DATA_KEY_FINGERPRINT_DOMAIN).as_ref(),
        );
        Ok(Self { id: id.to_owned(), fingerprint, material })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// 返回用于密钥轮换诊断的非秘密指纹。
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// 以材料建立 AES-256-GCM 密钥；失败原因由调用方按用途记录。
    pub(crate) fn aead_key(&self) -> Result<aead::LessSafeKey, ring::error::Unspecified> {
        aead::UnboundKey::new(&aead::AES_256_GCM, &self.material).map(aead::LessSafeKey::new)
    }

    /// 在 integrations crate 内部向其他密钥用途适配器提供材料；不暴露到 crate 公共 API。
    pub(crate) fn material(&self) -> &[u8] {
        &self.material
    }
}

impl Drop for DataKey {
    fn drop(&mut self) {
        self.material.fill(0);
    }
}

impl fmt::Debug for DataKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

impl fmt::Display for DataKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// 将短生命周期装备 key 封装为数据库可持久化的 AES-GCM 信封。
pub struct EquipmentKeySealer {
    data_key: Arc<DataKey>,
    random: SystemRandom,
}

impl EquipmentKeySealer {
    /// 建立使用系统安全随机源的装备 key 加密器；新写入固定使用当前密钥。
    pub fn new(data_key: Arc<DataKey>) -> Self {
        Self { data_key, random: SystemRandom::new() }
    }

    /// 用装备 ID 绑定附加认证数据，避免密文被移植到另一件装备。
    pub fn seal(
        &self,
        equipment_id: EquipmentId,
        key: &EquipmentKey,
    ) -> Result<EquipmentKeyEnvelope, EquipmentKeySealerError> {
        if equipment_id.is_nil() {
            return Err(EquipmentKeySealerError::InvalidAssociatedData);
        }
        let aad =
            equipment_aad(equipment_id).ok_or(EquipmentKeySealerError::InvalidAssociatedData)?;
        // 加密与指纹在同一次借用中完成，不为明文 key 另建副本。
        let sealed = key
            .with_bytes(|plaintext| {
                seal_secret(
                    &self.data_key,
                    SecretPurpose::EquipmentKey,
                    aad.as_bytes(),
                    plaintext,
                    &self.random,
                )
            })
            .map_err(safe_diagnostics::error_map("equipment_crypto", "encrypt_key_join", || {
                EquipmentKeySealerError::Encryption
            }))?
            .map_err(|error| match error {
                SealError::RandomSource => EquipmentKeySealerError::RandomSource,
                SealError::CipherKey => EquipmentKeySealerError::Unconfigured,
                SealError::Encryption => EquipmentKeySealerError::Encryption,
            })?;
        EquipmentKeyEnvelope::new(
            sealed.ciphertext,
            sealed.nonce.to_vec(),
            self.data_key.id.clone(),
            sealed.fingerprint,
        )
        .map_err(safe_diagnostics::error_map("equipment_crypto", "build_envelope", || {
            EquipmentKeySealerError::Encryption
        }))
    }

    /// 返回当前数据密钥的非秘密标识，供启动诊断使用。
    pub fn key_id(&self) -> &str {
        self.data_key.id()
    }
}

impl fmt::Debug for EquipmentKeySealer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EquipmentKeySealer")
            .field("key_id", &self.data_key.id)
            .field("key_fingerprint", &self.data_key.fingerprint)
            .finish()
    }
}

/// 使用域内密钥环打开装备 key 信封的解密适配器；信封 key ID 未知时失败关闭。
pub struct EquipmentKeyOpener {
    keys: Arc<Keyring>,
}

impl EquipmentKeyOpener {
    pub fn new(keys: Arc<Keyring>) -> Self {
        Self { keys }
    }

    /// 校验并解密绑定到指定装备 ID 的信封。
    /// 解密过程同时验证信封结构、数据密钥轮换标识、AES-GCM 认证标签和明文指纹。
    /// 无论成功或失败，解密缓冲区都会在返回前尽力清零。
    pub fn open(
        &self,
        equipment_id: EquipmentId,
        envelope: &EquipmentKeyEnvelope,
    ) -> Result<EquipmentKey, EquipmentKeyOpenerError> {
        if equipment_id.is_nil() {
            return Err(EquipmentKeyOpenerError::InvalidEquipmentId);
        }
        envelope.validate().map_err(safe_diagnostics::error_map(
            "equipment_crypto",
            "validate_envelope",
            || EquipmentKeyOpenerError::InvalidEnvelope,
        ))?;
        let data_key = self.keys.resolve(envelope.encryption_key_id()).map_err(|error| {
            if matches!(error, KeyringError::UnknownKeyId) {
                EquipmentKeyOpenerError::KeyMismatch
            } else {
                EquipmentKeyOpenerError::InvalidEnvelope
            }
        })?;
        let aad = equipment_aad(equipment_id).ok_or(EquipmentKeyOpenerError::InvalidEnvelope)?;
        let opened = open_secret(
            data_key,
            SecretPurpose::EquipmentKey,
            aad.as_bytes(),
            envelope.ciphertext(),
            envelope.nonce(),
            envelope.fingerprint(),
        )
        .map_err(|error| match error {
            OpenError::InvalidNonce => EquipmentKeyOpenerError::InvalidEnvelope,
            OpenError::CipherKey | OpenError::Authentication => EquipmentKeyOpenerError::Decryption,
            OpenError::FingerprintMismatch => EquipmentKeyOpenerError::FingerprintMismatch,
        })?;
        // `opened` 在返回时清零；值对象只复制校验后的 key。
        let text = std::str::from_utf8(opened.as_bytes()).map_err(safe_diagnostics::error_map(
            "equipment_crypto",
            "decode_key",
            || EquipmentKeyOpenerError::InvalidUtf8,
        ))?;
        EquipmentKey::new(text).map_err(safe_diagnostics::error_map(
            "equipment_crypto",
            "validate_key",
            || EquipmentKeyOpenerError::InvalidPlaintext,
        ))
    }
}

impl fmt::Debug for EquipmentKeyOpener {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("EquipmentKeyOpener").field("key_count", &self.keys.len()).finish()
    }
}

impl fmt::Display for EquipmentKeyOpener {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// 装备 key 的 AAD 绑定装备 ID；格式是已存信封的一部分。
fn equipment_aad(equipment_id: EquipmentId) -> Option<String> {
    let aad = format!("{AAD_PREFIX}{}/key", equipment_id.as_uuid());
    valid_associated_data(&aad).then_some(aad)
}

fn valid_associated_data(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'/'))
}

#[cfg(test)]
pub(crate) mod known_vectors;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use auction_domain::{DataKeyDomain, EquipmentId, EquipmentKey, EquipmentKeyEnvelope};
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

    use super::known_vectors::EQUIPMENT_KEY;
    use super::{EquipmentKeyOpener, EquipmentKeySealer};
    use crate::keyring::Keyring;

    fn data_key(id: &str, byte: u8) -> Arc<crate::DataKey> {
        Arc::new(crate::DataKey::from_base64(id, &BASE64.encode([byte; 32])).expect("测试密钥有效"))
    }

    /// 收敛前生成的装备 key 信封必须仍可打开；新写入的指纹、密钥标识、nonce 长度与 AAD 原文不变。
    #[test]
    fn known_vector_opens_and_reseals_with_frozen_format() {
        let vector = &EQUIPMENT_KEY;
        let equipment_id = EquipmentId::from_uuid(vector.subject());
        let keyring = Arc::new(Keyring::single(DataKeyDomain::Equipment, vector.data_key()));
        let envelope = EquipmentKeyEnvelope::new(
            vector.ciphertext(),
            vector.nonce(),
            vector.key_id.to_owned(),
            vector.fingerprint.to_owned(),
        )
        .expect("冻结信封结构有效");
        let opener = EquipmentKeyOpener::new(Arc::clone(&keyring));
        let opened = opener.open(equipment_id, &envelope).expect("冻结信封可打开");
        assert_eq!(opened.with_bytes(<[u8]>::to_vec).unwrap(), vector.plaintext);

        let resealed = EquipmentKeySealer::new(Arc::clone(keyring.current()))
            .seal(equipment_id, &opened)
            .expect("重新密封成功");
        assert_eq!(resealed.encryption_key_id(), vector.key_id);
        assert_eq!(resealed.fingerprint(), vector.fingerprint);
        assert_eq!(resealed.nonce().len(), vector.nonce().len());
        assert_eq!(
            vector.open_raw(resealed.ciphertext(), resealed.nonce()).as_deref(),
            Some(vector.plaintext)
        );

        // AAD 绑定装备 ID：移植到另一件装备必须失败关闭。
        let other = EquipmentId::from_uuid(uuid::Uuid::new_v4());
        assert!(opener.open(other, &envelope).is_err());
    }

    #[test]
    fn previous_equipment_key_decrypts_and_rotation_uses_current() {
        let equipment_id = EquipmentId::from_uuid(uuid::Uuid::new_v4());
        let old_only = Keyring::single(DataKeyDomain::Equipment, data_key("equipment-2025-11", 1));
        let old_sealer = EquipmentKeySealer::new(Arc::clone(old_only.current()));
        let key = EquipmentKey::new("0123456789").expect("装备 key 有效");
        let envelope = old_sealer.seal(equipment_id, &key).expect("旧 key 密封成功");
        assert_eq!(envelope.encryption_key_id(), "equipment-2025-11");

        let rotated = Arc::new(
            Keyring::new(
                DataKeyDomain::Equipment,
                data_key("equipment-2026-11", 2),
                vec![data_key("equipment-2025-11", 1)],
            )
            .expect("轮换密钥环有效"),
        );
        let opener = EquipmentKeyOpener::new(Arc::clone(&rotated));
        let opened = opener.open(equipment_id, &envelope).expect("旧 key 仍可解密");
        assert_eq!(opened.with_bytes(<[u8]>::to_vec).unwrap(), b"0123456789");

        let sealer = EquipmentKeySealer::new(Arc::clone(rotated.current()));
        let resealed = sealer.seal(equipment_id, &opened).expect("重新密封成功");
        assert_eq!(resealed.encryption_key_id(), "equipment-2026-11");
        // 旧 key 单独无法打开新信封。
        assert!(EquipmentKeyOpener::new(Arc::new(old_only)).open(equipment_id, &resealed).is_err());
    }

    #[test]
    fn unknown_key_id_fails_closed() {
        let equipment_id = EquipmentId::from_uuid(uuid::Uuid::new_v4());
        let keyring =
            Arc::new(Keyring::single(DataKeyDomain::Equipment, data_key("equipment-2026-11", 2)));
        let sealer = EquipmentKeySealer::new(Arc::clone(keyring.current()));
        let envelope = sealer
            .seal(equipment_id, &EquipmentKey::new("abcdef0123").expect("有效"))
            .expect("密封成功");
        let foreign = auction_domain::EquipmentKeyEnvelope::new(
            envelope.ciphertext().to_vec(),
            envelope.nonce().to_vec(),
            "user_login-2026-11".to_owned(),
            envelope.fingerprint().to_owned(),
        )
        .expect("结构有效");
        assert!(EquipmentKeyOpener::new(keyring).open(equipment_id, &foreign).is_err());
    }
}
