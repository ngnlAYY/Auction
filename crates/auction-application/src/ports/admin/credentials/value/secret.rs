//! Admin 凭据明文值与密文信封。

use std::fmt;

use super::{
    error::AdminCredentialError,
    limits::{MAX_ADMIN_CREDENTIAL_SECRET_BYTES, valid_text},
};

/// 由加密适配器产生的数据库密文信封。
pub struct AdminCredentialEnvelope {
    ciphertext: Vec<u8>,
    nonce: Vec<u8>,
    encryption_key_id: String,
    fingerprint: String,
}

impl AdminCredentialEnvelope {
    /// 建立并校验 AES-GCM 信封；密文和 nonce 不允许进入 JSON/日志。
    pub fn new(
        ciphertext: Vec<u8>,
        nonce: Vec<u8>,
        encryption_key_id: String,
        fingerprint: String,
    ) -> Result<Self, AdminCredentialError> {
        if ciphertext.len() < 16
            || nonce.len() != 12
            || !valid_text(&encryption_key_id, 256)
            || !valid_text(&fingerprint, 256)
        {
            return Err(AdminCredentialError::StoredResultCorrupt);
        }
        Ok(Self { ciphertext, nonce, encryption_key_id, fingerprint })
    }

    /// 返回密文切片供 storage 写入。
    pub fn ciphertext(&self) -> &[u8] {
        &self.ciphertext
    }

    pub fn nonce(&self) -> &[u8] {
        &self.nonce
    }

    /// 返回数据密钥轮换标识。
    pub fn encryption_key_id(&self) -> &str {
        &self.encryption_key_id
    }

    /// 返回密文指纹。
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}

impl fmt::Debug for AdminCredentialEnvelope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdminCredentialEnvelope")
            .field("ciphertext", &"[REDACTED]")
            .field("nonce", &"[REDACTED]")
            .field("encryption_key_id", &self.encryption_key_id)
            .field("fingerprint", &"[REDACTED]")
            .finish()
    }
}

impl Drop for AdminCredentialEnvelope {
    fn drop(&mut self) {
        self.ciphertext.fill(0);
        self.nonce.fill(0);
    }
}

/// 凭据明文值对象；Drop 时清零内存，禁止 Debug/序列化泄漏。
pub struct AdminCredentialSecret(Vec<u8>);

impl AdminCredentialSecret {
    /// 从 HTTP 已解析的 UTF-8 字符串建立明文值对象。
    pub fn new(value: String) -> Result<Self, AdminCredentialError> {
        if value.is_empty()
            || value.len() > MAX_ADMIN_CREDENTIAL_SECRET_BYTES
            || value
                .chars()
                .any(|character| character == '\0' || character == '\r' || character == '\n')
        {
            return Err(AdminCredentialError::InvalidRequest);
        }
        Ok(Self(value.into_bytes()))
    }

    /// 返回明文只读字节，调用方不得复制到持久化或日志结构。
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for AdminCredentialSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

impl Drop for AdminCredentialSecret {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}
