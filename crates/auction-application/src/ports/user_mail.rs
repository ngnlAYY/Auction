//! Users owner 的 MoogleMail 注册与换绑端口。
//!
//! 该模块只定义跨层契约：登录 key 明文只在 Worker 的短生命周期值对象中存在，数据库只接收
//! 摘要和密文信封，Delivery outbox 只接收用户 ID 引用。具体的密钥生成、加解密和 SQL 由
//! integrations/storage owner 分别实现。

use std::fmt;

use base64::Engine as _;
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{ApplicationFuture, RequestHash, mail_processing::MailProcessingError};

const LOGIN_KEY_BYTES: usize = 32;
const LOGIN_KEY_TEXT_LENGTH: usize = 43;

/// Users owner 接受的 MoogleMail 命令闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailUserCommandKind {
    Register,
    Rebind,
}

impl MailUserCommandKind {
    /// 返回迁移表和通知使用的稳定命令类型。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Register => "register",
            Self::Rebind => "rebind",
        }
    }
}

/// Users owner 返回给邮件处理器的结果闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailUserResultCode {
    Registered,
    Recovered,
    Blacklisted,
    IdentityConflict,
    Rebound,
    Unchanged,
    InvalidKey,
}

impl MailUserResultCode {
    /// 返回数据库约束和通知模板使用的稳定结果码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Registered => "registered",
            Self::Recovered => "recovered",
            Self::Blacklisted => "blacklisted",
            Self::IdentityConflict => "identity_conflict",
            Self::Rebound => "rebound",
            Self::Unchanged => "unchanged",
            Self::InvalidKey => "invalid_key",
        }
    }
}

/// Users owner 命令结果；`user_id` 为空只允许 identity_conflict 或 invalid_key。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailUserResult {
    pub user_id: Option<Uuid>,
    pub code: MailUserResultCode,
}

impl MailUserResult {
    /// 校验结果码与用户标识的组合，避免损坏 owner 结果进入决定或通知。
    pub fn validate(self, kind: MailUserCommandKind) -> Result<Self, MailProcessingError> {
        let allows_missing_user = matches!(
            (kind, self.code),
            (MailUserCommandKind::Register, MailUserResultCode::IdentityConflict)
                | (MailUserCommandKind::Rebind, MailUserResultCode::InvalidKey)
        );
        if self.user_id.is_none() != allows_missing_user {
            return Err(MailProcessingError::InvalidUserResult);
        }
        let valid_code = match kind {
            MailUserCommandKind::Register => matches!(
                self.code,
                MailUserResultCode::Registered
                    | MailUserResultCode::Recovered
                    | MailUserResultCode::Blacklisted
                    | MailUserResultCode::IdentityConflict
            ),
            MailUserCommandKind::Rebind => matches!(
                self.code,
                MailUserResultCode::Rebound
                    | MailUserResultCode::Blacklisted
                    | MailUserResultCode::IdentityConflict
                    | MailUserResultCode::Unchanged
                    | MailUserResultCode::InvalidKey
            ),
        };
        if !valid_code {
            return Err(MailProcessingError::InvalidUserResult);
        }
        Ok(self)
    }
}

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

/// 注册或找回命令。
#[derive(Debug, Clone)]
pub struct MailUserRegisterCommand {
    pub command_id: auction_domain::CommandId,
    pub request_hash: RequestHash,
    pub username: String,
    pub game_player_id: String,
    pub now: DateTime<Utc>,
}

/// Users owner 原子注册写入；候选用户与密钥信封必须在同一事务中提交。
pub struct MailUserRegisterWrite {
    pub command: MailUserRegisterCommand,
    pub user_id: Uuid,
    pub public_key_hash: [u8; 32],
    pub envelope: MailLoginKeyEnvelope,
}

/// 换绑命令只接收旧 key 摘要，明文在进入 Users owner 前已经清零。
#[derive(Debug, Clone)]
pub struct MailUserRebindCommand {
    pub command_id: auction_domain::CommandId,
    pub request_hash: RequestHash,
    pub old_key_hash: [u8; 32],
    pub next_username: String,
    pub next_game_player_id: String,
    pub history_id: Uuid,
    pub now: DateTime<Utc>,
}

/// Users owner 的注册、换绑和幂等读取端口。
pub trait MailUserStore: Send + Sync {
    fn find_mail_command(
        &self,
        command_id: auction_domain::CommandId,
        kind: MailUserCommandKind,
        request_hash: RequestHash,
    ) -> ApplicationFuture<'_, Option<MailUserResult>>;

    fn register_or_recover(
        &self,
        write: MailUserRegisterWrite,
    ) -> ApplicationFuture<'_, MailUserResult>;

    fn rebind_game_identity(
        &self,
        command: MailUserRebindCommand,
    ) -> ApplicationFuture<'_, MailUserResult>;
}

/// 读取登录 key 密文信封的窄端口；解密能力不属于 storage owner。
pub trait MailLoginKeyEnvelopeReader: Send + Sync {
    fn load_login_key_envelope(&self, user_id: Uuid)
    -> ApplicationFuture<'_, MailLoginKeyEnvelope>;
}

/// Worker 专用登录 key 签发端口，只返回摘要和密文信封。
pub trait MailLoginKeyIssuer: Send + Sync {
    fn issue(&self, user_id: Uuid) -> ApplicationFuture<'_, MailLoginKeyIssue>;
}

/// Delivery transport 专用的短生命周期明文读取端口。
pub trait MailLoginKeyReader: Send + Sync {
    fn read(&self, user_id: Uuid) -> ApplicationFuture<'_, MailLoginKey>;
}

/// Users owner 用例端口。
pub trait MailUserPort: Send + Sync {
    fn register_or_recover(
        &self,
        command: MailUserRegisterCommand,
    ) -> ApplicationFuture<'_, MailUserResult>;

    fn rebind_game_identity(
        &self,
        command: MailUserRebindCommand,
    ) -> ApplicationFuture<'_, MailUserResult>;
}

fn valid_secret_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}
