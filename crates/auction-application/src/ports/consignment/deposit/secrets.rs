//! 预约令牌的密码端口。
//!
//! 存储只生成规范令牌与摘要，并通过本端口封装、解封；密钥与 AEAD 实现属于
//! `auction-integrations`。端口类型不实现序列化，`Debug` 不输出密文、明文或密钥标识。

use std::fmt;

use auction_domain::consignment::deposit::TokenText;
use uuid::Uuid;

/// 预约密文绑定的稳定 AAD。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReservationSecretAad {
    /// 预约 UUID。
    pub reservation_id: Uuid,
    /// 预约代。
    pub generation: u64,
    /// 卖家 UUID。
    pub seller_user_id: Uuid,
}

/// 已封装的预约令牌密文。
#[derive(Clone, PartialEq, Eq)]
pub struct SealedReservationSecret {
    ciphertext: Vec<u8>,
    nonce: [u8; 12],
    encryption_key_id: String,
    fingerprint: String,
}

impl SealedReservationSecret {
    /// 从 AEAD 输出构造；形状错误失败关闭。
    pub fn new(
        ciphertext: Vec<u8>,
        nonce: [u8; 12],
        encryption_key_id: String,
        fingerprint: String,
    ) -> Result<Self, ReservationSecretCryptoError> {
        if ciphertext.is_empty()
            || encryption_key_id.is_empty()
            || encryption_key_id.len() > 64
            || !encryption_key_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
            || fingerprint.len() != 64
            || !fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(ReservationSecretCryptoError);
        }
        Ok(Self { ciphertext, nonce, encryption_key_id, fingerprint })
    }

    /// 借用密文。
    pub fn ciphertext(&self) -> &[u8] {
        &self.ciphertext
    }

    /// 返回 nonce。
    pub const fn nonce(&self) -> &[u8; 12] {
        &self.nonce
    }

    /// 借用密钥标识。
    pub fn encryption_key_id(&self) -> &str {
        &self.encryption_key_id
    }

    /// 借用明文指纹；只用于信封认证，不进入日志或回执。
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}

impl fmt::Debug for SealedReservationSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SealedReservationSecret")
            .field("ciphertext", &"[REDACTED]")
            .field("nonce", &"[REDACTED]")
            .field("encryption_key_id", &"[REDACTED]")
            .field("fingerprint", &"[REDACTED]")
            .finish()
    }
}

/// 预约令牌封装或解封失败的安全投影。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("预约令牌密文不可用")]
pub struct ReservationSecretCryptoError;

/// 预约令牌 AEAD 适配器。
///
/// 实现方必须用 AAD 绑定预约、代与卖家；未注入实现时存储只能失败关闭，不能退化为
/// 明文保存或进程内随机密钥。
pub trait ReservationSecretCrypto: Send + Sync {
    /// 用当前数据密钥封装规范令牌文本。
    fn seal(
        &self,
        token: &TokenText,
        aad: ReservationSecretAad,
    ) -> Result<SealedReservationSecret, ReservationSecretCryptoError>;

    /// 用原 AAD 解封；失败不得回显密文或底层错误链。
    fn open(
        &self,
        sealed: &SealedReservationSecret,
        aad: ReservationSecretAad,
    ) -> Result<TokenText, ReservationSecretCryptoError>;
}
