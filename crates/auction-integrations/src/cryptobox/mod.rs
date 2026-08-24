//! 装备 key 的加密信封适配器。
//!
//! 该模块是唯一接触数据密钥材料的当前实现。应用层只得到密文信封，存储层只保存信封
//! 字段；明文装备 key 在同步回调中使用，随后由领域值对象清零。

use std::fmt;

use auction_domain::{EquipmentId, EquipmentKey, EquipmentKeyEnvelope};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use ring::{
    aead, hmac,
    rand::{SecureRandom, SystemRandom},
};
use thiserror::Error;

const DATA_KEY_LENGTH: usize = 32;
const NONCE_LENGTH: usize = 12;
const DATA_KEY_FINGERPRINT_DOMAIN: &[u8] = b"auction/cryptobox/data-key/v1";
const SECRET_FINGERPRINT_DOMAIN: &[u8] = b"auction/cryptobox/secret/v1/equipment-key\0";
const AAD_PREFIX: &str = "auction/equipment-key/v1/";

/// 装备 key 加密器错误。
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

/// 不透明的数据密钥；调试、显示和序列化永远不输出材料。
pub struct DataKey {
    id: String,
    fingerprint: String,
    material: Vec<u8>,
}

impl DataKey {
    /// 从敏感 TOML 中的标识和标准 Base64 材料建立 AES-256 数据密钥。
    pub fn from_base64(id: &str, encoded: &str) -> Result<Self, EquipmentKeySealerError> {
        if !valid_key_id(id) {
            return Err(EquipmentKeySealerError::InvalidKeyId);
        }
        let material =
            BASE64.decode(encoded).map_err(|_| EquipmentKeySealerError::InvalidDataKey)?;
        if material.len() != DATA_KEY_LENGTH {
            return Err(EquipmentKeySealerError::InvalidDataKey);
        }
        let key = hmac::Key::new(hmac::HMAC_SHA256, &material);
        let fingerprint = hex_encode(hmac::sign(&key, DATA_KEY_FINGERPRINT_DOMAIN).as_ref());
        Ok(Self { id: id.to_owned(), fingerprint, material })
    }

    /// 返回非秘密的数据密钥标识。
    pub fn id(&self) -> &str {
        &self.id
    }

    /// 返回用于密钥轮换诊断的非秘密指纹。
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub(crate) fn aead_key(&self) -> Result<aead::LessSafeKey, EquipmentKeySealerError> {
        let unbound = aead::UnboundKey::new(&aead::AES_256_GCM, &self.material)
            .map_err(|_| EquipmentKeySealerError::Unconfigured)?;
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
        self.material.clear();
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
    data_key: DataKey,
    random: SystemRandom,
}

impl EquipmentKeySealer {
    /// 建立使用系统安全随机源的装备 key 加密器。
    pub fn new(data_key: DataKey) -> Self {
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
        self.random.fill(&mut nonce).map_err(|_| EquipmentKeySealerError::RandomSource)?;
        let aead_key = self.data_key.aead_key()?;
        let mut ciphertext = key
            .with_bytes(|plaintext| {
                let mut buffer = plaintext.to_vec();
                aead_key
                    .seal_in_place_append_tag(
                        aead::Nonce::assume_unique_for_key(nonce),
                        aead::Aad::from(aad.as_bytes()),
                        &mut buffer,
                    )
                    .map(|()| buffer)
                    .map_err(|_| EquipmentKeySealerError::Encryption)
            })
            .map_err(|_| EquipmentKeySealerError::Encryption)??;

        let fingerprint = {
            let hmac_key = hmac::Key::new(hmac::HMAC_SHA256, &self.data_key.material);
            let mut input = Vec::with_capacity(SECRET_FINGERPRINT_DOMAIN.len() + 10);
            input.extend_from_slice(SECRET_FINGERPRINT_DOMAIN);
            key.with_bytes(|plaintext| input.extend_from_slice(plaintext))
                .map_err(|_| EquipmentKeySealerError::Encryption)?;
            hex_encode(hmac::sign(&hmac_key, &input).as_ref())
        };

        let envelope = EquipmentKeyEnvelope::new(
            std::mem::take(&mut ciphertext),
            nonce.to_vec(),
            self.data_key.id.clone(),
            fingerprint,
        )
        .map_err(|_| EquipmentKeySealerError::Encryption)?;
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

fn valid_key_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphanumeric() || (index > 0 && matches!(byte, b'.' | b'_' | b'-'))
        })
}

fn valid_associated_data(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'/'))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}
