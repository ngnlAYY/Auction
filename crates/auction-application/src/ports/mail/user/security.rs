//! Users/MoogleMail 登录 key 的短生命周期安全容器。

use std::fmt;

use base64::Engine as _;
use sha2::{Digest, Sha256};

use super::super::mail_processing::MailProcessingError;

const LOGIN_KEY_BYTES: usize = 32;
const LOGIN_KEY_TEXT_LENGTH: usize = 43;

/// 仅在 Worker 内存中短暂存在的 Base64URL 登录 key。
pub struct MailLoginKey {
    encoded: Vec<u8>,
}

impl MailLoginKey {
    /// 解析 256-bit entropy 的 canonical Base64URL（无 padding）登录 key。
    pub fn from_encoded(encoded: &str) -> Result<Self, MailProcessingError> {
        if encoded.len() != LOGIN_KEY_TEXT_LENGTH || !encoded.is_ascii() {
            return Err(MailProcessingError::InvalidLoginKey);
        }
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| MailProcessingError::InvalidLoginKey)?;
        let canonical = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&decoded);
        let valid = decoded.len() == LOGIN_KEY_BYTES && canonical == encoded;
        let mut decoded = decoded;
        decoded.fill(0);
        if !valid {
            return Err(MailProcessingError::InvalidLoginKey);
        }
        Ok(Self { encoded: encoded.as_bytes().to_vec() })
    }

    /// 在受控回调中读取编码值；回调不得保留或记录明文。
    pub fn with_encoded<T>(&self, callback: impl FnOnce(&str) -> T) -> T {
        // `from_encoded` 只接受 ASCII canonical Base64URL，因此这里不会失败；不使用
        // `unwrap` 是为了避免公共请求路径形成隐含 panic，异常数据由构造器提前拒绝。
        let value = std::str::from_utf8(&self.encoded).unwrap_or_default();
        callback(value)
    }

    /// 返回登录 key 的固定 SHA-256 摘要；数据库和认证只比较该摘要。
    pub fn public_key_hash(&self) -> [u8; 32] {
        let digest = Sha256::digest(&self.encoded);
        let mut result = [0_u8; 32];
        result.copy_from_slice(&digest);
        result
    }

    /// 尽力清除 key 的内存副本。
    pub fn clear(&mut self) {
        self.encoded.fill(0);
        self.encoded.clear();
    }
}

impl Drop for MailLoginKey {
    fn drop(&mut self) {
        self.clear();
    }
}

impl fmt::Debug for MailLoginKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

impl fmt::Display for MailLoginKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// 用户登录 key 的数据库密文信封。
pub struct MailLoginKeyEnvelope {
    ciphertext: Vec<u8>,
    nonce: Vec<u8>,
    encryption_key_id: String,
    fingerprint: String,
}

impl MailLoginKeyEnvelope {
    /// 从数据库或加密适配器建立并校验信封。
    pub fn new(
        ciphertext: Vec<u8>,
        nonce: Vec<u8>,
        encryption_key_id: String,
        fingerprint: String,
    ) -> Result<Self, MailProcessingError> {
        if ciphertext.len() < 16
            || nonce.len() != 12
            || !valid_secret_text(&encryption_key_id, 64)
            || !valid_secret_text(&fingerprint, 256)
        {
            return Err(MailProcessingError::InvalidLoginKeyEnvelope);
        }
        Ok(Self { ciphertext, nonce, encryption_key_id, fingerprint })
    }

    /// 返回密文；调用方不得记录或复制到浏览器/日志。
    pub fn ciphertext(&self) -> &[u8] {
        &self.ciphertext
    }

    /// 返回 GCM nonce。
    pub fn nonce(&self) -> &[u8] {
        &self.nonce
    }

    /// 返回非秘密的数据密钥轮换标识。
    pub fn encryption_key_id(&self) -> &str {
        &self.encryption_key_id
    }

    /// 返回用于完整性核对的非明文指纹。
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}

impl Clone for MailLoginKeyEnvelope {
    fn clone(&self) -> Self {
        Self {
            ciphertext: self.ciphertext.clone(),
            nonce: self.nonce.clone(),
            encryption_key_id: self.encryption_key_id.clone(),
            fingerprint: self.fingerprint.clone(),
        }
    }
}

impl Drop for MailLoginKeyEnvelope {
    fn drop(&mut self) {
        self.ciphertext.fill(0);
        self.nonce.fill(0);
        self.encryption_key_id.clear();
        self.fingerprint.clear();
    }
}

impl fmt::Debug for MailLoginKeyEnvelope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// 注册事务所需的密钥摘要和密文材料，不含登录 key 明文。
pub struct MailLoginKeyIssue {
    pub public_key_hash: [u8; 32],
    pub envelope: MailLoginKeyEnvelope,
}

impl fmt::Debug for MailLoginKeyIssue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

fn valid_secret_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}
