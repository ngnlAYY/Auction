//! 装备来源的纯领域值对象。
//! 这里只定义装备 key 和持久化密文信封的形状，不负责网络读取、加密或数据库写入。
//! 明文 key 只能在可信适配器的同步回调中短暂使用；对外格式化和序列化一律返回脱敏值。

use std::fmt;

use serde::ser::Serializer;
use thiserror::Error;

use crate::text::valid_text;

pub mod taxonomy;

const EQUIPMENT_KEY_LENGTH: usize = 10;
const ENVELOPE_NONCE_LENGTH: usize = 12;
const MIN_CIPHERTEXT_LENGTH: usize = 16;

/// 装备 key 的值对象错误。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum EquipmentKeyError {
    #[error("装备 key 必须是 10 位十六进制字符串")]
    InvalidFormat,
}

/// 只在可信调用链中短暂存在的装备 key。
/// 该类型不实现 `Clone`，并在销毁时尽力清零内部字节。任何日志、调试输出和 JSON
/// 序列化都只能得到 `[REDACTED]`。
pub struct EquipmentKey {
    bytes: Option<[u8; EQUIPMENT_KEY_LENGTH]>,
}

impl EquipmentKey {
    /// 校验并复制小写规范化的十六进制 key。
    pub fn new(raw: &str) -> Result<Self, EquipmentKeyError> {
        if raw.len() != EQUIPMENT_KEY_LENGTH || !raw.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(EquipmentKeyError::InvalidFormat);
        }
        let mut bytes = [0; EQUIPMENT_KEY_LENGTH];
        bytes.copy_from_slice(raw.as_bytes());
        bytes.make_ascii_lowercase();
        Ok(Self { bytes: Some(bytes) })
    }

    /// 在不转移所有权的同步回调中短暂暴露 key 字节。
    pub fn with_bytes<T>(&self, callback: impl FnOnce(&[u8]) -> T) -> Result<T, EquipmentKeyError> {
        let bytes = self.bytes.as_ref().ok_or(EquipmentKeyError::InvalidFormat)?;
        Ok(callback(bytes))
    }

    /// 尽力清零并丢弃内部 key。
    pub fn clear(&mut self) {
        if let Some(bytes) = &mut self.bytes {
            bytes.fill(0);
        }
        self.bytes = None;
    }

    /// 为同一条有界、幂等命令建立一次短生命周期副本。
    /// 该入口只供调用方在明确的进程内重试边界中使用；类型仍不实现 `Clone`，避免装备 key
    /// 被容器、DTO 或日志辅助结构无意复制。原值和副本销毁时都会独立清零各自缓冲区。
    pub fn duplicate_for_retry(&self) -> Self {
        Self { bytes: self.bytes }
    }
}

impl Drop for EquipmentKey {
    fn drop(&mut self) {
        self.clear();
    }
}

impl fmt::Debug for EquipmentKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

impl fmt::Display for EquipmentKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

impl serde::Serialize for EquipmentKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str("[REDACTED]")
    }
}

/// 装备 key 的持久化密文信封。
/// 信封只由加密适配器构造；数据库 owner 只校验结构并保存字段，不接触明文 key。
#[derive(Clone, PartialEq, Eq)]
pub struct EquipmentKeyEnvelope {
    ciphertext: Vec<u8>,
    nonce: Vec<u8>,
    encryption_key_id: String,
    fingerprint: String,
}

impl EquipmentKeyEnvelope {
    /// 从已经完成加密的字段建立信封，接收调用方缓冲区的所有权。
    pub fn new(
        ciphertext: Vec<u8>,
        nonce: Vec<u8>,
        encryption_key_id: String,
        fingerprint: String,
    ) -> Result<Self, EquipmentKeyEnvelopeError> {
        let envelope = Self { ciphertext, nonce, encryption_key_id, fingerprint };
        envelope.validate()?;
        Ok(envelope)
    }

    /// 校验信封字段的长度、字符集和非空约束。
    pub fn validate(&self) -> Result<(), EquipmentKeyEnvelopeError> {
        if self.ciphertext.len() < MIN_CIPHERTEXT_LENGTH
            || self.nonce.len() != ENVELOPE_NONCE_LENGTH
            || !valid_key_id(&self.encryption_key_id)
            || !valid_text(&self.fingerprint, 256)
        {
            return Err(EquipmentKeyEnvelopeError::InvalidShape);
        }
        Ok(())
    }

    /// 借用密文供持久化适配器使用；需要原地解密的调用方自行建立可变副本。
    pub fn ciphertext(&self) -> &[u8] {
        &self.ciphertext
    }

    /// 借用 nonce，避免只读校验和持久化绑定重复分配缓冲区。
    pub fn nonce(&self) -> &[u8] {
        &self.nonce
    }

    /// 返回非秘密数据密钥标识。
    pub fn encryption_key_id(&self) -> &str {
        &self.encryption_key_id
    }

    /// 返回用于重复识别的非秘密指纹。
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}

impl fmt::Debug for EquipmentKeyEnvelope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

impl fmt::Display for EquipmentKeyEnvelope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

impl serde::Serialize for EquipmentKeyEnvelope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str("[REDACTED]")
    }
}

/// 装备密文信封结构错误。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum EquipmentKeyEnvelopeError {
    #[error("装备 key 密文信封结构无效")]
    InvalidShape,
}

/// 数据加密 key 标识：1 至 64 字节，字母数字开头，其后可含 `.`、`_`、`-`；装备与通用信封共用。
pub fn valid_key_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphanumeric() || (index > 0 && matches!(byte, b'.' | b'_' | b'-'))
        })
}
