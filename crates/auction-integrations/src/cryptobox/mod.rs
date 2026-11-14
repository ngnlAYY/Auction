//! 数据密钥与装备 key 的加密信封适配器；应用层只得到密文信封，明文
//! key 用后由领域值对象清零。

use std::sync::Arc;

use auction_application::core::diagnostics::DiagnosticContext;

use crate::safe_diagnostics;

use std::fmt;

use auction_domain::{EquipmentId, EquipmentKey, EquipmentKeyEnvelope};

use crate::keyring::{Keyring, KeyringError};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use ring::{
    aead, hmac,
    rand::{SecureRandom, SystemRandom},
};
use subtle::ConstantTimeEq;
use thiserror::Error;

const DATA_KEY_LENGTH: usize = 32;
const NONCE_LENGTH: usize = 12;
const DATA_KEY_FINGERPRINT_DOMAIN: &[u8] = b"auction/cryptobox/data-key/v1";
const SECRET_FINGERPRINT_DOMAIN: &[u8] = b"auction/cryptobox/secret/v1/equipment-key\0";
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

    pub(crate) fn aead_key(&self) -> Result<aead::LessSafeKey, EquipmentKeySealerError> {
        let unbound = aead::UnboundKey::new(&aead::AES_256_GCM, &self.material).map_err(
            |diagnostic_cause| {
                safe_diagnostics::error(
                    "equipment_crypto",
                    "prepare_cipher_key",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                EquipmentKeySealerError::Unconfigured
            },
        )?;
        Ok(aead::LessSafeKey::new(unbound))
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
        let aad = format!("{AAD_PREFIX}{}/key", equipment_id.as_uuid());
        if !valid_associated_data(&aad) {
            return Err(EquipmentKeySealerError::InvalidAssociatedData);
        }

        let mut nonce = [0_u8; NONCE_LENGTH];
        self.random.fill(&mut nonce).map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "equipment_crypto",
                "generate_nonce",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            EquipmentKeySealerError::RandomSource
        })?;
        let aead_key = self.data_key.aead_key()?;
        let ciphertext = key
            .with_bytes(|plaintext| {
                // 预留认证标签，避免追加标签时扩容并复制整份加密缓冲。
                let mut buffer = Vec::with_capacity(plaintext.len() + aead::AES_256_GCM.tag_len());
                buffer.extend_from_slice(plaintext);
                if let Err(diagnostic_cause) = aead_key.seal_in_place_append_tag(
                    aead::Nonce::assume_unique_for_key(nonce),
                    aead::Aad::from(aad.as_bytes()),
                    &mut buffer,
                ) {
                    buffer.fill(0);
                    safe_diagnostics::error(
                        "equipment_crypto",
                        "encrypt_key",
                        safe_diagnostics::project(&diagnostic_cause),
                        DiagnosticContext::default(),
                    );
                    return Err(EquipmentKeySealerError::Encryption);
                }
                Ok(buffer)
            })
            .map_err(|diagnostic_cause| {
                safe_diagnostics::error(
                    "equipment_crypto",
                    "encrypt_key_join",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                EquipmentKeySealerError::Encryption
            })??;

        // 与解密共用分块 HMAC，避免为指纹另建含明文 key 的拼接缓冲。
        let fingerprint = key
            .with_bytes(|plaintext| fingerprint(&self.data_key, plaintext))
            .map_err(|diagnostic_cause| {
                safe_diagnostics::error(
                    "equipment_crypto",
                    "fingerprint_key",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                EquipmentKeySealerError::Encryption
            })?;

        let envelope = EquipmentKeyEnvelope::new(
            ciphertext,
            nonce.to_vec(),
            self.data_key.id.clone(),
            fingerprint,
        )
        .map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "equipment_crypto",
                "build_envelope",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            EquipmentKeySealerError::Encryption
        })?;
        Ok(envelope)
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
        envelope.validate().map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "equipment_crypto",
                "validate_envelope",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            EquipmentKeyOpenerError::InvalidEnvelope
        })?;
        let data_key = self.keys.resolve(envelope.encryption_key_id()).map_err(|error| {
            if matches!(error, KeyringError::UnknownKeyId) {
                EquipmentKeyOpenerError::KeyMismatch
            } else {
                EquipmentKeyOpenerError::InvalidEnvelope
            }
        })?;

        let aad = format!("{AAD_PREFIX}{}/key", equipment_id.as_uuid());
        if !valid_associated_data(&aad) {
            return Err(EquipmentKeyOpenerError::InvalidEnvelope);
        }

        let nonce_array: [u8; NONCE_LENGTH] =
            envelope.nonce().try_into().map_err(|_| EquipmentKeyOpenerError::InvalidEnvelope)?;
        let mut buffer = envelope.ciphertext().to_vec();
        let result = (|| {
            let aead_key = data_key.aead_key().map_err(|diagnostic_cause| {
                safe_diagnostics::error(
                    "equipment_crypto",
                    "prepare_decryption_key",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                EquipmentKeyOpenerError::Decryption
            })?;
            let plaintext = aead_key
                .open_in_place(
                    aead::Nonce::assume_unique_for_key(nonce_array),
                    aead::Aad::from(aad.as_bytes()),
                    &mut buffer,
                )
                .map_err(|diagnostic_cause| {
                    safe_diagnostics::error(
                        "equipment_crypto",
                        "decrypt_key",
                        safe_diagnostics::project(&diagnostic_cause),
                        DiagnosticContext::default(),
                    );
                    EquipmentKeyOpenerError::Decryption
                })?;

            let actual_fingerprint = fingerprint(data_key, plaintext);
            if !bool::from(actual_fingerprint.as_bytes().ct_eq(envelope.fingerprint().as_bytes())) {
                return Err(EquipmentKeyOpenerError::FingerprintMismatch);
            }

            let plaintext = std::str::from_utf8(plaintext).map_err(|diagnostic_cause| {
                safe_diagnostics::error(
                    "equipment_crypto",
                    "decode_key",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                EquipmentKeyOpenerError::InvalidUtf8
            })?;
            EquipmentKey::new(plaintext).map_err(|diagnostic_cause| {
                safe_diagnostics::error(
                    "equipment_crypto",
                    "validate_key",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                EquipmentKeyOpenerError::InvalidPlaintext
            })
        })();
        buffer.fill(0);
        result
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

fn valid_associated_data(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'/'))
}

fn fingerprint(data_key: &DataKey, plaintext: &[u8]) -> String {
    let hmac_key = hmac::Key::new(hmac::HMAC_SHA256, data_key.material());
    let mut context = hmac::Context::with_key(&hmac_key);
    context.update(SECRET_FINGERPRINT_DOMAIN);
    context.update(plaintext);
    auction_application::core::lower_hex(context.sign().as_ref())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use auction_domain::{DataKeyDomain, EquipmentId, EquipmentKey};
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

    use super::{EquipmentKeyOpener, EquipmentKeySealer};
    use crate::keyring::Keyring;

    fn data_key(id: &str, byte: u8) -> Arc<crate::DataKey> {
        Arc::new(crate::DataKey::from_base64(id, &BASE64.encode([byte; 32])).expect("测试密钥有效"))
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
