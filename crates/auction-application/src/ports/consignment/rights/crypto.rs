//! 挑战码的密码端口。
//!
//! 挑战码只以 MAC 入库，投递用密文由 AAD 绑定 challenge、权利、冻结身份与权利版本；
//! 密钥与算法属于 `auction-integrations`。端口类型不实现序列化，`Debug` 不输出密文或密钥标识。
//! 未注入实现时存储只能失败关闭，不能退化为明文保存、内存随机密钥或跳过 MAC。

use std::fmt;

use auction_domain::consignment::rights::ChallengeCode;
use uuid::Uuid;

/// 挑战 MAC 与密文共同绑定的 AAD。
///
/// 虚拟目标没有权利，`right_id` 为 `None`；换任一字段都不得通过 MAC 或解封。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChallengeAad {
    /// 挑战 UUID。
    pub challenge_id: Uuid,
    /// 权利 UUID；虚拟或未匹配目标为空。
    pub right_id: Option<Uuid>,
    /// 冻结身份摘要。
    pub identity_digest: [u8; 32],
    /// 权利版本。
    pub right_version: u64,
}

/// 已封装的挑战码密文，供专用 outbox 在传输边界解封。
#[derive(Clone, PartialEq, Eq)]
pub struct SealedChallengeSecret {
    ciphertext: Vec<u8>,
    nonce: [u8; 12],
    encryption_key_id: String,
    fingerprint: String,
}

impl SealedChallengeSecret {
    /// 从 AEAD 输出构造；形状错误失败关闭。
    pub fn new(
        ciphertext: Vec<u8>,
        nonce: [u8; 12],
        encryption_key_id: String,
        fingerprint: String,
    ) -> Result<Self, RightCryptoError> {
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
            return Err(RightCryptoError);
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

    /// 借用明文指纹；只用于认证，不进入日志或回执。
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}

impl fmt::Debug for SealedChallengeSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SealedChallengeSecret")
            .field("ciphertext", &"[REDACTED]")
            .field("nonce", &"[REDACTED]")
            .field("encryption_key_id", &"[REDACTED]")
            .field("fingerprint", &"[REDACTED]")
            .finish()
    }
}

/// 挑战码封装、解封或 MAC 失败的安全投影。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("挑战码密码能力不可用")]
pub struct RightCryptoError;

/// 挑战码密码适配器。
pub trait RightChallengeCrypto: Send + Sync {
    /// 用当前数据密钥封装码，供专用 outbox 投递。
    fn seal(
        &self,
        code: &ChallengeCode,
        aad: ChallengeAad,
    ) -> Result<SealedChallengeSecret, RightCryptoError>;

    /// 用原 AAD 解封；失败不得回显密文或底层错误链。
    fn open(
        &self,
        sealed: &SealedChallengeSecret,
        aad: ChallengeAad,
    ) -> Result<ChallengeCode, RightCryptoError>;

    /// 对码计算 HMAC-SHA256。调用方在常量时间内比较，不得用 `==`。
    fn mac(&self, aad: ChallengeAad, code: &ChallengeCode) -> Result<[u8; 32], RightCryptoError>;
}
