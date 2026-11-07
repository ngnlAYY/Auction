//! 数据密钥环：当前写入密钥 + 保留解密的旧密钥。
//!
//! 规则：
//! - 新写入永远使用 `current`；信封只记录当前 key ID。
//! - 解密按信封自带的 key ID 在**同一域**内解析；未知 ID 失败关闭，不回退到当前 key，
//!   也不跨域尝试其它父密钥。
//! - 旧密钥只在通过保留期登记后才可能从配置移除；运行时密钥环只持有当前与已配置的旧密钥。

use std::{collections::HashSet, fmt, sync::Arc};

use auction_domain::DataKeyDomain;
use thiserror::Error;

use crate::cryptobox::{DataKey, EquipmentKeySealerError};

/// 密钥环装配或解析错误；不包含密钥材料或明文。
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum KeyringError {
    #[error("数据密钥标识重复")]
    DuplicateKeyId,
    #[error("当前数据密钥不能同时出现在旧密钥列表")]
    CurrentKeyDuplicated,
    #[error("数据密钥环为空")]
    Empty,
    #[error("等待解密的信封引用了未知数据密钥")]
    UnknownKeyId,
    #[error("数据密钥格式无效")]
    InvalidDataKey,
}

/// 单一域的数据密钥环；`current` 用于加密，`previous` 只用于解密。
pub struct Keyring {
    domain: DataKeyDomain,
    current: Arc<DataKey>,
    previous: Vec<Arc<DataKey>>,
}

impl Keyring {
    pub fn new(
        domain: DataKeyDomain,
        current: Arc<DataKey>,
        previous: Vec<Arc<DataKey>>,
    ) -> Result<Self, KeyringError> {
        // 当前密钥出现在旧密钥列表中先按“当前密钥重复”报告，便于运维定位配置错误。
        if previous.iter().any(|key| key.id() == current.id()) {
            return Err(KeyringError::CurrentKeyDuplicated);
        }
        let mut seen = HashSet::with_capacity(previous.len());
        for key in &previous {
            if !seen.insert(key.id().to_owned()) {
                return Err(KeyringError::DuplicateKeyId);
            }
        }
        Ok(Self { domain, current, previous })
    }

    /// 单密钥环；旧信封仍可解密，但一旦信封引用其它 key ID 就会失败关闭。
    pub fn single(domain: DataKeyDomain, current: Arc<DataKey>) -> Self {
        Self { domain, current, previous: Vec::new() }
    }

    /// 由标识与 Base64 材料装配；`current` 在前，其后为解密用旧密钥。
    pub fn from_base64<'a>(
        domain: DataKeyDomain,
        current: (&str, &str),
        previous: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<Self, KeyringError> {
        let current = Arc::new(DataKey::from_base64(current.0, current.1).map_err(map_key_error)?);
        let mut keys = Vec::new();
        for (id, encoded) in previous {
            keys.push(Arc::new(DataKey::from_base64(id, encoded).map_err(map_key_error)?));
        }
        Self::new(domain, current, keys)
    }

    pub const fn domain(&self) -> DataKeyDomain {
        self.domain
    }

    pub fn current(&self) -> &Arc<DataKey> {
        &self.current
    }

    pub fn current_id(&self) -> &str {
        self.current.id()
    }

    pub fn is_current(&self, key_id: &str) -> bool {
        self.current.id() == key_id
    }

    /// 按信封中的 key ID 解析解密密钥；未知 ID 失败关闭。
    pub fn resolve(&self, key_id: &str) -> Result<&Arc<DataKey>, KeyringError> {
        if self.current.id() == key_id {
            return Ok(&self.current);
        }
        self.previous.iter().find(|key| key.id() == key_id).ok_or(KeyringError::UnknownKeyId)
    }

    /// 信封是否仍需重加密：key ID 不是当前写入密钥即为待处理。
    /// 未知 key ID 在本判定中同样为待处理，真正的解密仍由 `resolve` 失败关闭。
    pub fn needs_rotation(&self, key_id: &str) -> bool {
        !self.is_current(key_id)
    }

    pub fn contains(&self, key_id: &str) -> bool {
        self.current.id() == key_id || self.previous.iter().any(|key| key.id() == key_id)
    }

    pub fn previous_ids(&self) -> impl Iterator<Item = &str> {
        self.previous.iter().map(|key| key.id())
    }

    pub fn len(&self) -> usize {
        self.previous.len() + 1
    }

    pub fn is_empty(&self) -> bool {
        false
    }
}

impl fmt::Debug for Keyring {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Keyring")
            .field("domain", &self.domain.as_str())
            .field("current_key_id", &self.current.id())
            .field("previous_key_ids", &self.previous.iter().map(|k| k.id()).collect::<Vec<_>>())
            .finish()
    }
}

fn map_key_error(error: EquipmentKeySealerError) -> KeyringError {
    match error {
        EquipmentKeySealerError::InvalidKeyId | EquipmentKeySealerError::InvalidDataKey => {
            KeyringError::InvalidDataKey
        }
        _ => KeyringError::InvalidDataKey,
    }
}

/// 由配置投影装配单一域的密钥环；未配置当前密钥时返回 `None`。
pub fn build_keyring<'a>(
    domain: DataKeyDomain,
    current: Option<(&str, &str)>,
    previous: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<Option<Arc<Keyring>>, KeyringError> {
    let Some(current) = current else {
        return Ok(None);
    };
    Keyring::from_base64(domain, current, previous).map(|ring| Some(Arc::new(ring)))
}

#[cfg(test)]
mod tests {
    use self::parent::*;
    use crate::keyring as parent;
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

    fn key(id: &str, byte: u8) -> Arc<DataKey> {
        Arc::new(DataKey::from_base64(id, &BASE64.encode([byte; 32])).expect("测试密钥有效"))
    }

    #[test]
    fn current_and_previous_keys_resolve_within_domain() {
        let ring = Keyring::new(
            DataKeyDomain::Equipment,
            key("equipment-2026-11", 1),
            vec![key("equipment-2025-11", 2)],
        )
        .expect("密钥环有效");
        assert_eq!(ring.current_id(), "equipment-2026-11");
        assert!(ring.resolve("equipment-2026-11").is_ok());
        assert_eq!(ring.resolve("equipment-2025-11").unwrap().id(), "equipment-2025-11");
        assert_eq!(
            ring.resolve("equipment-2024-11").expect_err("未知 key ID 必须失败"),
            KeyringError::UnknownKeyId
        );
        assert_eq!(ring.len(), 2);
        assert!(!ring.is_empty());
    }

    #[test]
    fn duplicate_key_ids_are_rejected() {
        let current = key("equipment-2026-11", 1);
        assert_eq!(
            Keyring::new(DataKeyDomain::Equipment, Arc::clone(&current), vec![current])
                .unwrap_err(),
            KeyringError::CurrentKeyDuplicated
        );
        assert_eq!(
            Keyring::new(
                DataKeyDomain::Equipment,
                key("equipment-2026-11", 1),
                vec![key("equipment-2025-11", 2), key("equipment-2025-11", 3)],
            )
            .unwrap_err(),
            KeyringError::DuplicateKeyId
        );
    }

    #[test]
    fn rotation_is_idempotent_and_wrong_key_fails_closed() {
        let old = Keyring::new(DataKeyDomain::Credential, key("credential-2025-11", 1), Vec::new())
            .unwrap();
        let next = Keyring::new(
            DataKeyDomain::Credential,
            key("credential-2026-11", 2),
            vec![key("credential-2025-11", 1)],
        )
        .unwrap();
        // 第一次扫描：旧信封待处理；重写后 key ID 变为当前，第二次扫描为空。
        let mut rows =
            vec![("credential-2025-11".to_owned(), 1_u8), ("credential-2026-11".to_owned(), 2_u8)];
        let first = rows.iter().filter(|(id, _)| next.needs_rotation(id)).count();
        assert_eq!(first, 1);
        for (id, _) in &mut rows {
            *id = next.current_id().to_owned();
        }
        assert_eq!(rows.iter().filter(|(id, _)| next.needs_rotation(id)).count(), 0);
        // 旧 key 在下一代密钥环中仍可解密；只含新 key 的环必须失败关闭。
        assert!(next.resolve("credential-2025-11").is_ok());
        assert_eq!(
            Keyring::single(DataKeyDomain::Credential, next.current().clone())
                .resolve("credential-2025-11")
                .expect_err("旧 key 不在单密钥环中"),
            KeyringError::UnknownKeyId
        );
        assert!(old.needs_rotation("credential-2026-11"));
    }

    #[test]
    fn debug_never_prints_material() {
        let ring = Keyring::new(
            DataKeyDomain::Credential,
            key("credential-2026-11", 9),
            vec![key("credential-2025-11", 8)],
        )
        .unwrap();
        let text = format!("{ring:?}");
        assert!(text.contains("credential-2026-11"));
        assert!(!text.contains(&BASE64.encode([9_u8; 32])));
        assert!(!text.contains(&BASE64.encode([8_u8; 32])));
    }
}
