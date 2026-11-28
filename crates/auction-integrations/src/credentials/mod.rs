//! 外部凭据获取、校验与密封适配器；mail_cookie 与 forum_cookie 使用不同 AAD 和指纹域，
//! 本模块不读配置或数据库，账号密码仅供单次论坛登录，持久化边界只接收不透明密文信封。

use std::{fmt, sync::Arc};

use auction_application::admin::{
    AdminCredentialEnvelope, AdminCredentialError, AdminCredentialKind, AdminCredentialOpener,
    AdminCredentialSealer,
};
use auction_application::core::ApplicationError;
use auction_domain::DataKeyDomain;
use ring::rand::SystemRandom;
use uuid::Uuid;

use crate::cryptobox::envelope::{
    OpenError, OpenedSecret, SealError, SecretPurpose, open_secret, seal_secret,
};
use crate::keyring::Keyring;

mod acquire;
mod check;
mod mail_check;
pub use acquire::CredentialAcquirer;
pub use check::CredentialVerifier;
pub use mail_check::WorkerMailCredentialVerifier;

const MAIL_AAD_PREFIX: &str = "auction/credential/mail/v1/";
const FORUM_AAD_PREFIX: &str = "auction/credential/forum/v1/";

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
        // Cookie 用途必须使用独立凭据域密钥环；兄弟适配器同样在入口拒绝别的域。
        if self.keys.domain() != DataKeyDomain::Credential {
            return Err(AdminCredentialError::InvalidRequest.into());
        }
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
        let data_key = self.keys.current();
        let sealed = seal_secret(data_key, purpose(kind), aad.as_bytes(), secret, &self.random)
            .map_err(sealing_error)?;
        AdminCredentialEnvelope::new(
            sealed.ciphertext,
            sealed.nonce.to_vec(),
            data_key.id().to_owned(),
            sealed.fingerprint,
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
        if self.keys.domain() != DataKeyDomain::Credential {
            return Err(crypto_error());
        }
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
        // 直接移交已认证缓冲，避免复制 Cookie；认证或指纹失败时原语已尽力清零缓冲。
        open_secret(
            data_key,
            purpose(kind),
            aad.as_bytes(),
            envelope.ciphertext(),
            envelope.nonce(),
            envelope.fingerprint(),
        )
        .map(OpenedSecret::into_bytes)
        .map_err(opening_error)
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

/// 两类 Cookie 的指纹域互不相同，同一明文换用途不能通过指纹校验。
const fn purpose(kind: AdminCredentialKind) -> SecretPurpose {
    match kind {
        AdminCredentialKind::MailCookie => SecretPurpose::MoogleMailCookie,
        AdminCredentialKind::ForumCookie => SecretPurpose::EhentaiCookie,
    }
}

/// 原语已按用途记录具体原因；对外统一为“加密不可用”，不区分随机源、密钥或加密失败。
fn sealing_error(failure: SealError) -> ApplicationError {
    match failure {
        SealError::RandomSource | SealError::CipherKey | SealError::Encryption => crypto_error(),
    }
}

/// 认证与指纹失败同样失败关闭，不向调用方区分篡改与密钥错误。
fn opening_error(failure: OpenError) -> ApplicationError {
    match failure {
        OpenError::InvalidNonce
        | OpenError::CipherKey
        | OpenError::Authentication
        | OpenError::FingerprintMismatch => crypto_error(),
    }
}

fn crypto_error() -> ApplicationError {
    ApplicationError::AdminCredential(AdminCredentialError::CryptoUnavailable)
}

/// 在线检查期间解密出的 Cookie 明文；取消或错误退出也清空缓冲，不实现 Debug/Serialize。
struct DecryptedCookie(Vec<u8>);

impl Drop for DecryptedCookie {
    fn drop(&mut self) {
        // 普通 fill(0) 可能在释放前被优化掉；与邮件客户端的敏感缓冲共用同一 volatile 清零。
        crate::clients::mooglemail::wipe_bytes(&mut self.0);
    }
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
    use crate::cryptobox::known_vectors::{EHENTAI_COOKIE, KnownVector, MOOGLEMAIL_COOKIE};
    use crate::keyring::Keyring;

    fn data_key(id: &str, byte: u8) -> Arc<crate::DataKey> {
        Arc::new(crate::DataKey::from_base64(id, &BASE64.encode([byte; 32])).expect("测试密钥有效"))
    }

    /// 收敛前生成的两类 Cookie 信封必须仍可打开；新写入的指纹、密钥标识、nonce 长度与 AAD 原文不变。
    #[test]
    fn known_vectors_open_and_reseal_with_frozen_format() {
        let cases: [(AdminCredentialKind, &str, &KnownVector); 2] = [
            (AdminCredentialKind::MailCookie, "mooglemail:main", &MOOGLEMAIL_COOKIE),
            (AdminCredentialKind::ForumCookie, "ehentai:shared", &EHENTAI_COOKIE),
        ];
        for (kind, scope, vector) in cases {
            let sealer = CredentialSealer::new(Arc::new(Keyring::single(
                DataKeyDomain::Credential,
                vector.data_key(),
            )));
            let envelope = auction_application::admin::AdminCredentialEnvelope::new(
                vector.ciphertext(),
                vector.nonce(),
                vector.key_id.to_owned(),
                vector.fingerprint.to_owned(),
            )
            .expect("冻结信封结构有效");
            let secret =
                AdminCredentialOpener::open(&sealer, kind, vector.subject(), scope, &envelope)
                    .expect("冻结信封可打开");
            assert_eq!(secret, vector.plaintext);

            let resealed =
                AdminCredentialSealer::seal(&sealer, kind, vector.subject(), scope, &secret)
                    .expect("重新密封成功");
            assert_eq!(resealed.encryption_key_id(), vector.key_id);
            assert_eq!(resealed.fingerprint(), vector.fingerprint);
            assert_eq!(resealed.nonce().len(), vector.nonce().len());
            assert_eq!(
                vector.open_raw(resealed.ciphertext(), resealed.nonce()).as_deref(),
                Some(vector.plaintext)
            );

            // AAD 绑定作用域与凭据 ID：换作用域必须失败关闭。
            assert!(
                AdminCredentialOpener::open(
                    &sealer,
                    kind,
                    vector.subject(),
                    "other:scope",
                    &envelope
                )
                .is_err()
            );
        }
        // 两类 Cookie 的 AAD 前缀不同：邮件 Cookie 信封不能按论坛 Cookie 打开。
        let swapped = auction_application::admin::AdminCredentialEnvelope::new(
            MOOGLEMAIL_COOKIE.ciphertext(),
            MOOGLEMAIL_COOKIE.nonce(),
            MOOGLEMAIL_COOKIE.key_id.to_owned(),
            MOOGLEMAIL_COOKIE.fingerprint.to_owned(),
        )
        .expect("结构有效");
        let sealer = CredentialSealer::new(Arc::new(Keyring::single(
            DataKeyDomain::Credential,
            MOOGLEMAIL_COOKIE.data_key(),
        )));
        assert!(
            AdminCredentialOpener::open(
                &sealer,
                AdminCredentialKind::ForumCookie,
                MOOGLEMAIL_COOKIE.subject(),
                "mooglemail:main",
                &swapped,
            )
            .is_err()
        );
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
