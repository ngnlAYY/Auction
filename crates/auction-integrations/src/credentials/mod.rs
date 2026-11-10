//! 外部凭据获取、校验与密封适配器；mail_cookie 与 forum_cookie 使用不同 AAD 和指纹域，
//! 本模块不读配置或数据库，账号密码仅供单次论坛登录，持久化边界只接收不透明密文信封。

use auction_application::core::diagnostics::DiagnosticContext;

use crate::diagnostics as safe_diagnostics;

use std::{fmt, sync::Arc};

use auction_application::admin::{
    AdminCredentialEnvelope, AdminCredentialError, AdminCredentialKind, AdminCredentialOpener,
    AdminCredentialSealer,
};
use auction_application::core::ApplicationError;
use ring::{
    aead, hmac,
    rand::{SecureRandom, SystemRandom},
};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::DataKey;
use crate::keyring::Keyring;

mod acquire;
mod check;
pub use acquire::CredentialAcquirer;
pub use check::CredentialVerifier;

const NONCE_LENGTH: usize = 12;
const MAIL_AAD_PREFIX: &str = "auction/credential/mail/v1/";
const FORUM_AAD_PREFIX: &str = "auction/credential/forum/v1/";
const MAIL_FINGERPRINT_DOMAIN: &[u8] = b"auction/cryptobox/secret/v1/mooglemail-cookie\0";
const FORUM_FINGERPRINT_DOMAIN: &[u8] = b"auction/cryptobox/secret/v1/ehentai-cookie\0";

pub struct CredentialSealer {
    keys: Arc<Keyring>,
    random: SystemRandom,
}

impl CredentialSealer {
    /// 使用独立于装备和用户登录 key 的域内密钥环建立密封器；新写入使用当前密钥。
    pub fn new(keys: Arc<Keyring>) -> Self {
        Self { keys, random: SystemRandom::new() }
    }

    /// 当前写入密钥；供启动诊断与密钥环核对使用。
    pub fn key_id(&self) -> &str {
        self.keys.current_id()
    }

    fn seal_now(
        &self,
        kind: AdminCredentialKind,
        credential_id: Uuid,
        scope: &str,
        secret: &[u8],
    ) -> Result<AdminCredentialEnvelope, ApplicationError> {
        if credential_id.is_nil()
            || scope.is_empty()
            || scope.len() > 128
            || !scope.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'_' | b'-' | b'.')
            })
            || secret.is_empty()
            || secret.len() > 16 * 1024
            || secret.iter().any(|byte| matches!(byte, 0 | b'\r' | b'\n'))
        {
            return Err(AdminCredentialError::InvalidRequest.into());
        }
        let aad = credential_aad(kind, credential_id, scope);
        let mut nonce = [0_u8; NONCE_LENGTH];
        self.random.fill(&mut nonce).map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "credential_crypto",
                "generate_nonce",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            crypto_error()
        })?;
        let data_key = self.keys.current();
        let key = data_key.aead_key().map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "credential_crypto",
                "load_encryption_key",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            crypto_error()
        })?;
        // 预留认证标签，避免追加标签时扩容并复制整份加密缓冲。
        let mut ciphertext = Vec::with_capacity(secret.len() + aead::AES_256_GCM.tag_len());
        ciphertext.extend_from_slice(secret);
        if let Err(diagnostic_cause) = key.seal_in_place_append_tag(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(aad.as_bytes()),
            &mut ciphertext,
        ) {
            ciphertext.fill(0);
            safe_diagnostics::error(
                "credential_crypto",
                "encrypt_secret",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            return Err(crypto_error());
        }
        let fingerprint = fingerprint(data_key, kind, secret);
        AdminCredentialEnvelope::new(
            ciphertext,
            nonce.to_vec(),
            data_key.id().to_owned(),
            fingerprint,
        )
        .map_err(ApplicationError::from)
    }

    fn open_now(
        &self,
        kind: AdminCredentialKind,
        credential_id: Uuid,
        scope: &str,
        envelope: &AdminCredentialEnvelope,
    ) -> Result<Vec<u8>, ApplicationError> {
        if credential_id.is_nil()
            || scope.is_empty()
            || scope.len() > 128
            || !scope.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'_' | b'-' | b'.')
            })
        {
            return Err(crypto_error());
        }
        let aad = credential_aad(kind, credential_id, scope);
        let data_key =
            self.keys.resolve(envelope.encryption_key_id()).ok().ok_or_else(crypto_error)?;
        let key = data_key.aead_key().map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "credential_crypto",
                "load_decryption_key",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            crypto_error()
        })?;
        let nonce = aead::Nonce::try_assume_unique_for_key(envelope.nonce()).map_err(
            |diagnostic_cause| {
                safe_diagnostics::error(
                    "credential_crypto",
                    "validate_nonce",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                crypto_error()
            },
        )?;
        let mut secret = envelope.ciphertext().to_vec();
        let length = match key
            .open_in_place(nonce, aead::Aad::from(aad.as_bytes()), &mut secret)
            .map(|plaintext| plaintext.len())
        {
            Ok(length) => length,
            Err(diagnostic_cause) => {
                safe_diagnostics::error(
                    "credential_crypto",
                    "decrypt_secret",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );

                secret.fill(0);
                return Err(crypto_error());
            }
        };
        // 直接移交已认证缓冲，避免复制 Cookie；截短前清理认证标签所在的尾部。
        secret[length..].fill(0);
        secret.truncate(length);
        let actual_fingerprint = fingerprint(data_key, kind, &secret);
        if actual_fingerprint.as_bytes().ct_eq(envelope.fingerprint().as_bytes()).into() {
            Ok(secret)
        } else {
            secret.fill(0);
            Err(crypto_error())
        }
    }
}

impl AdminCredentialSealer for CredentialSealer {
    fn seal(
        &self,
        kind: AdminCredentialKind,
        credential_id: Uuid,
        scope: &str,
        secret: &[u8],
    ) -> Result<AdminCredentialEnvelope, ApplicationError> {
        self.seal_now(kind, credential_id, scope, secret)
    }
}

impl AdminCredentialOpener for CredentialSealer {
    fn open(
        &self,
        kind: AdminCredentialKind,
        credential_id: Uuid,
        scope: &str,
        envelope: &AdminCredentialEnvelope,
    ) -> Result<Vec<u8>, ApplicationError> {
        self.open_now(kind, credential_id, scope, envelope)
    }
}

impl fmt::Debug for CredentialSealer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialSealer")
            .field("key_id", &self.keys.current_id())
            .field("key_count", &self.keys.len())
            .finish()
    }
}

fn credential_aad(kind: AdminCredentialKind, credential_id: Uuid, scope: &str) -> String {
    let prefix = match kind {
        AdminCredentialKind::MailCookie => MAIL_AAD_PREFIX,
        AdminCredentialKind::ForumCookie => FORUM_AAD_PREFIX,
    };
    format!("{prefix}{scope}/{credential_id}")
}

fn fingerprint(data_key: &DataKey, kind: AdminCredentialKind, secret: &[u8]) -> String {
    let domain = match kind {
        AdminCredentialKind::MailCookie => MAIL_FINGERPRINT_DOMAIN,
        AdminCredentialKind::ForumCookie => FORUM_FINGERPRINT_DOMAIN,
    };
    let key = hmac::Key::new(hmac::HMAC_SHA256, data_key.material());
    let mut context = hmac::Context::with_key(&key);
    context.update(domain);
    context.update(secret);
    auction_application::core::lower_hex(context.sign().as_ref())
}

fn crypto_error() -> ApplicationError {
    ApplicationError::AdminCredential(AdminCredentialError::CryptoUnavailable)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use auction_application::admin::{
        AdminCredentialKind, AdminCredentialOpener, AdminCredentialSealer,
    };
    use auction_domain::DataKeyDomain;
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
    use uuid::Uuid;

    use super::CredentialSealer;
    use crate::keyring::Keyring;

    fn data_key(id: &str, byte: u8) -> Arc<crate::DataKey> {
        Arc::new(crate::DataKey::from_base64(id, &BASE64.encode([byte; 32])).expect("测试密钥有效"))
    }

    /// 旧 key 写入的信封在同时持有当前与旧 key 的密钥环中可解密；只含新 key 时失败关闭。
    #[test]
    fn previous_key_decrypts_and_missing_key_fails_closed() {
        let credential_id = Uuid::new_v4();
        let scope = "mooglemail:main";
        let old =
            Keyring::new(DataKeyDomain::Credential, data_key("credential-2025-11", 1), Vec::new())
                .expect("旧密钥环有效");
        let old_sealer = CredentialSealer::new(Arc::new(old));
        let envelope = AdminCredentialSealer::seal(
            &old_sealer,
            AdminCredentialKind::MailCookie,
            credential_id,
            scope,
            b"ipb_member_id=1; ipb_pass_hash=deadbeef",
        )
        .expect("旧密钥密封成功");
        assert_eq!(envelope.encryption_key_id(), "credential-2025-11");

        let current = CredentialSealer::new(Arc::new(
            Keyring::new(
                DataKeyDomain::Credential,
                data_key("credential-2026-11", 2),
                vec![data_key("credential-2025-11", 1)],
            )
            .expect("轮换密钥环有效"),
        ));
        let secret = AdminCredentialOpener::open(
            &current,
            AdminCredentialKind::MailCookie,
            credential_id,
            scope,
            &envelope,
        )
        .expect("旧 key 仍可解密");
        assert_eq!(secret, b"ipb_member_id=1; ipb_pass_hash=deadbeef");

        // 用当前 key 重新密封后，旧 key 单独无法解密（失败关闭）。
        let resealed = AdminCredentialSealer::seal(
            &current,
            AdminCredentialKind::MailCookie,
            credential_id,
            scope,
            &secret,
        )
        .expect("重新密封成功");
        assert_eq!(resealed.encryption_key_id(), "credential-2026-11");
        assert_ne!(resealed.fingerprint(), envelope.fingerprint());
        assert!(
            AdminCredentialOpener::open(
                &old_sealer,
                AdminCredentialKind::MailCookie,
                credential_id,
                scope,
                &resealed,
            )
            .is_err()
        );
    }

    /// 未知 key ID 不能回退到当前 key，也不能跨域尝试。
    #[test]
    fn unknown_key_id_is_rejected() {
        let credential_id = Uuid::new_v4();
        let scope = "mooglemail:main";
        let keyring = Arc::new(
            Keyring::new(DataKeyDomain::Credential, data_key("credential-2026-11", 2), Vec::new())
                .expect("密钥环有效"),
        );
        let sealer = CredentialSealer::new(Arc::clone(&keyring));
        let envelope = AdminCredentialSealer::seal(
            &sealer,
            AdminCredentialKind::ForumCookie,
            credential_id,
            scope,
            b"ipb_member_id=2",
        )
        .expect("密封成功");
        let foreign = auction_application::admin::AdminCredentialEnvelope::new(
            envelope.ciphertext().to_vec(),
            envelope.nonce().to_vec(),
            "equipment-2026-11".to_owned(),
            envelope.fingerprint().to_owned(),
        )
        .expect("结构有效");
        assert!(
            AdminCredentialOpener::open(
                &sealer,
                AdminCredentialKind::ForumCookie,
                credential_id,
                scope,
                &foreign,
            )
            .is_err()
        );
    }

    /// 同一密钥重复密封产生新 nonce，指纹只由 key 与明文决定（重加密幂等性依据）。
    #[test]
    fn reseal_is_deterministic_in_fingerprint_only() {
        let credential_id = Uuid::new_v4();
        let scope = "ehentai:shared";
        let sealer = CredentialSealer::new(Arc::new(
            Keyring::new(DataKeyDomain::Credential, data_key("credential-2026-11", 3), Vec::new())
                .expect("密钥环有效"),
        ));
        let first = AdminCredentialSealer::seal(
            &sealer,
            AdminCredentialKind::ForumCookie,
            credential_id,
            scope,
            b"ipb_member_id=3",
        )
        .expect("密封成功");
        let second = AdminCredentialSealer::seal(
            &sealer,
            AdminCredentialKind::ForumCookie,
            credential_id,
            scope,
            b"ipb_member_id=3",
        )
        .expect("密封成功");
        assert_ne!(first.nonce(), second.nonce());
        assert_eq!(first.fingerprint(), second.fingerprint());
    }
}
