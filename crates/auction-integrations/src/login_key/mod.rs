//! 用户登录 key 的签发、Admin 手工封装与受控解密；Users owner 只见 SHA-256
//! 摘要与 AES-256-GCM 信封，AAD 绑定用户 UUID、指纹绑定用途。Delivery 在发送时
//! 解密，Admin 创建/签发命令及其幂等重放通过专用 opener 返回生成的 key。
//! Admin 手工 key 保留旧版 32–256 字节闭集，Worker 解密只接受系统签发的 canonical Base64URL。

use auction_application::core::diagnostics::DiagnosticContext;

use crate::diagnostics as safe_diagnostics;

use std::{fmt, sync::Arc};

use auction_application::admin::{AdminLoginKeyIssuer, AdminLoginKeyOpener, AdminUserKeyIssue};
use auction_application::auction::ApplicationFuture;
use auction_application::core::ApplicationError;
use auction_application::mail::{
    MailLoginKey, MailLoginKeyEnvelope, MailLoginKeyEnvelopeReader, MailLoginKeyIssue,
    MailLoginKeyIssuer, MailLoginKeyReader, MailProcessingError,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::{
    aead, hmac,
    rand::{SecureRandom, SystemRandom},
};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::DataKey;
use crate::keyring::{Keyring, KeyringError};

const LOGIN_KEY_ENTROPY_BYTES: usize = 32;
const LOGIN_KEY_NONCE_BYTES: usize = 12;
const LOGIN_KEY_FINGERPRINT_DOMAIN: &[u8] = b"user-login-key";
const SECRET_FINGERPRINT_PREFIX: &[u8] = b"auction/cryptobox/secret/v1/";

/// 登录 key 加解密错误；对外统一映射为不暴露细节的应用错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginKeyCryptoError {
    RandomSource,
    Encryption,
    Decryption,
    InvalidEnvelope,
    KeyMismatch,
    FingerprintMismatch,
    InvalidPlaintext,
}

impl fmt::Display for LoginKeyCryptoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MoogleMail 登录 key 加密操作失败")
    }
}

impl std::error::Error for LoginKeyCryptoError {}

/// 为 Users owner 签发密文信封的登录 key issuer。
pub struct MoogleMailLoginKeyIssuer {
    data_key: Arc<DataKey>,
    random: SystemRandom,
}

impl MoogleMailLoginKeyIssuer {
    /// 使用敏感配置中的独立用户登录 key 数据密钥建立签发器。
    pub fn new(data_key: Arc<DataKey>) -> Self {
        Self { data_key, random: SystemRandom::new() }
    }

    fn issue_now(&self, user_id: Uuid) -> Result<MailLoginKeyIssue, ApplicationError> {
        let issue = self.generate_admin_now(user_id)?;
        let public_key_hash = issue.key.public_key_hash();
        Ok(MailLoginKeyIssue { public_key_hash, envelope: issue.envelope })
    }

    fn generate_admin_now(&self, user_id: Uuid) -> Result<AdminUserKeyIssue, ApplicationError> {
        if user_id.is_nil() {
            return Err(MailProcessingError::InvalidLoginKey.into());
        }
        let mut entropy = [0_u8; LOGIN_KEY_ENTROPY_BYTES];
        if let Err(diagnostic_cause) = self.random.fill(&mut entropy) {
            entropy.fill(0);
            safe_diagnostics::error(
                "login_key",
                "generate_entropy",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            return Err(crypto_error(LoginKeyCryptoError::RandomSource));
        }
        let encoded = URL_SAFE_NO_PAD.encode(entropy.as_slice());
        entropy.fill(0);
        let key = MailLoginKey::from_encoded(&encoded).map_err(ApplicationError::from);
        let mut encoded_bytes = encoded.into_bytes();
        encoded_bytes.fill(0);
        self.seal_admin_key_now(user_id, key?)
    }

    fn seal_admin_key_now(
        &self,
        user_id: Uuid,
        key: MailLoginKey,
    ) -> Result<AdminUserKeyIssue, ApplicationError> {
        if user_id.is_nil() {
            return Err(MailProcessingError::InvalidLoginKey.into());
        }
        let aad = login_key_aad(user_id);
        // 借用值对象中的短时明文，不为加密和指纹各自复制登录 key。
        let (ciphertext, nonce, fingerprint) = key
            .with_encoded(|value| {
                let plaintext = value.as_bytes();
                let (ciphertext, nonce) =
                    seal(&self.data_key, plaintext, aad.as_bytes(), &self.random)?;
                Ok((ciphertext, nonce, fingerprint(&self.data_key, plaintext)))
            })
            .map_err(crypto_error)?;
        let envelope = MailLoginKeyEnvelope::new(
            ciphertext,
            nonce,
            self.data_key.id().to_owned(),
            fingerprint,
        )
        .map_err(ApplicationError::from)?;
        Ok(AdminUserKeyIssue { key, envelope })
    }
}

impl MailLoginKeyIssuer for MoogleMailLoginKeyIssuer {
    fn issue(&self, user_id: Uuid) -> ApplicationFuture<'_, MailLoginKeyIssue> {
        Box::pin(async move { self.issue_now(user_id) })
    }
}

impl AdminLoginKeyIssuer for MoogleMailLoginKeyIssuer {
    fn generate(&self, user_id: Uuid) -> ApplicationFuture<'_, AdminUserKeyIssue> {
        Box::pin(async move { self.generate_admin_now(user_id) })
    }

    fn seal(&self, user_id: Uuid, key: MailLoginKey) -> ApplicationFuture<'_, AdminUserKeyIssue> {
        Box::pin(async move { self.seal_admin_key_now(user_id, key) })
    }
}

/// 从 storage 密文信封读取并解密登录 key 的 Worker adapter。
pub struct MoogleMailLoginKeyReader<S> {
    source: Arc<S>,
    keys: Arc<Keyring>,
}

impl<S> MoogleMailLoginKeyReader<S>
where
    S: MailLoginKeyEnvelopeReader,
{
    /// 建立只供 Delivery transport 使用的解密读取器；旧密钥只用于解密。
    pub fn new(source: Arc<S>, keys: Arc<Keyring>) -> Self {
        Self { source, keys }
    }

    async fn read_now(&self, user_id: Uuid) -> Result<MailLoginKey, ApplicationError> {
        if user_id.is_nil() {
            return Err(MailProcessingError::InvalidLoginKey.into());
        }
        let envelope = self.source.load_login_key_envelope(user_id).await?;
        self.open_envelope_now(user_id, envelope)
    }

    fn open_envelope_now(
        &self,
        user_id: Uuid,
        envelope: MailLoginKeyEnvelope,
    ) -> Result<MailLoginKey, ApplicationError> {
        if user_id.is_nil() {
            return Err(MailProcessingError::InvalidLoginKey.into());
        }
        let data_key = match self.keys.resolve(envelope.encryption_key_id()) {
            Ok(data_key) => data_key,
            Err(KeyringError::UnknownKeyId) => {
                return Err(crypto_error(LoginKeyCryptoError::KeyMismatch));
            }
            Err(error) => {
                let _ = error;
                return Err(crypto_error(LoginKeyCryptoError::InvalidEnvelope));
            }
        };
        let aad = login_key_aad(user_id);
        let mut plaintext = open(
            data_key,
            envelope.ciphertext(),
            envelope.nonce(),
            envelope.encryption_key_id(),
            aad.as_bytes(),
        )
        .map_err(crypto_error)?;
        let encoded = match String::from_utf8(std::mem::take(&mut plaintext)) {
            Ok(encoded) => encoded,
            Err(error) => {
                let mut bytes = error.into_bytes();
                bytes.fill(0);
                return Err(crypto_error(LoginKeyCryptoError::InvalidPlaintext));
            }
        };
        let key = MailLoginKey::from_encoded(&encoded).map_err(ApplicationError::from);
        let mut encoded_bytes = encoded.into_bytes();
        encoded_bytes.fill(0);
        let key = key?;
        verify_fingerprint(data_key, &key, envelope.fingerprint()).map_err(crypto_error)?;
        Ok(key)
    }
}

impl<S> MailLoginKeyReader for MoogleMailLoginKeyReader<S>
where
    S: MailLoginKeyEnvelopeReader + 'static,
{
    fn read(&self, user_id: Uuid) -> ApplicationFuture<'_, MailLoginKey> {
        Box::pin(async move { self.read_now(user_id).await })
    }
}

impl<S> AdminLoginKeyOpener for MoogleMailLoginKeyReader<S>
where
    S: MailLoginKeyEnvelopeReader + 'static,
{
    fn open(
        &self,
        user_id: Uuid,
        envelope: MailLoginKeyEnvelope,
    ) -> ApplicationFuture<'_, MailLoginKey> {
        Box::pin(async move { self.open_envelope_now(user_id, envelope) })
    }
}

fn verify_fingerprint(
    data_key: &DataKey,
    key: &MailLoginKey,
    expected: &str,
) -> Result<(), LoginKeyCryptoError> {
    let actual = key.with_encoded(|value| fingerprint(data_key, value.as_bytes()));
    if actual.as_bytes().ct_eq(expected.as_bytes()).into() {
        Ok(())
    } else {
        Err(LoginKeyCryptoError::FingerprintMismatch)
    }
}

fn fingerprint(data_key: &DataKey, plaintext: &[u8]) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA256, data_key.material());
    let mut context = hmac::Context::with_key(&key);
    context.update(SECRET_FINGERPRINT_PREFIX);
    context.update(LOGIN_KEY_FINGERPRINT_DOMAIN);
    context.update(&[0]);
    context.update(plaintext);
    auction_application::core::lower_hex(context.sign().as_ref())
}

fn seal(
    data_key: &DataKey,
    plaintext: &[u8],
    aad: &[u8],
    random: &SystemRandom,
) -> Result<(Vec<u8>, Vec<u8>), LoginKeyCryptoError> {
    let unbound = aead::UnboundKey::new(&aead::AES_256_GCM, data_key.material()).map_err(
        |diagnostic_cause| {
            safe_diagnostics::error(
                "login_key",
                "prepare_encryption_key",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            LoginKeyCryptoError::Encryption
        },
    )?;
    let key = aead::LessSafeKey::new(unbound);
    let mut nonce = vec![0_u8; LOGIN_KEY_NONCE_BYTES];
    random.fill(&mut nonce).map_err(|diagnostic_cause| {
        safe_diagnostics::warn(
            "login_key",
            "generate_nonce",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        LoginKeyCryptoError::RandomSource
    })?;
    let nonce_array: [u8; LOGIN_KEY_NONCE_BYTES] =
        nonce.as_slice().try_into().map_err(|_| LoginKeyCryptoError::Encryption)?;
    // 预留认证标签，避免追加标签时扩容并复制整份加密缓冲。
    let mut buffer = Vec::with_capacity(plaintext.len() + aead::AES_256_GCM.tag_len());
    buffer.extend_from_slice(plaintext);
    if let Err(diagnostic_cause) = key.seal_in_place_append_tag(
        aead::Nonce::assume_unique_for_key(nonce_array),
        aead::Aad::from(aad),
        &mut buffer,
    ) {
        buffer.fill(0);
        safe_diagnostics::error(
            "login_key",
            "encrypt_key",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        return Err(LoginKeyCryptoError::Encryption);
    }
    Ok((buffer, nonce))
}

fn open(
    data_key: &DataKey,
    ciphertext: &[u8],
    nonce: &[u8],
    key_id: &str,
    aad: &[u8],
) -> Result<Vec<u8>, LoginKeyCryptoError> {
    if key_id != data_key.id() || nonce.len() != LOGIN_KEY_NONCE_BYTES || ciphertext.len() < 16 {
        return Err(if key_id != data_key.id() {
            LoginKeyCryptoError::KeyMismatch
        } else {
            LoginKeyCryptoError::InvalidEnvelope
        });
    }
    let unbound = aead::UnboundKey::new(&aead::AES_256_GCM, data_key.material()).map_err(
        |diagnostic_cause| {
            safe_diagnostics::error(
                "login_key",
                "prepare_decryption_key",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            LoginKeyCryptoError::Decryption
        },
    )?;
    let key = aead::LessSafeKey::new(unbound);
    let nonce_array: [u8; LOGIN_KEY_NONCE_BYTES] =
        nonce.try_into().map_err(|_| LoginKeyCryptoError::InvalidEnvelope)?;
    let mut buffer = ciphertext.to_vec();
    let length = match key
        .open_in_place(
            aead::Nonce::assume_unique_for_key(nonce_array),
            aead::Aad::from(aad),
            &mut buffer,
        )
        .map(|plaintext| plaintext.len())
    {
        Ok(length) => length,
        Err(diagnostic_cause) => {
            safe_diagnostics::debug(
                "login_key",
                "decrypt_key",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );

            buffer.fill(0);
            return Err(LoginKeyCryptoError::Decryption);
        }
    };
    buffer[length..].fill(0);
    buffer.truncate(length);
    Ok(buffer)
}

fn login_key_aad(user_id: Uuid) -> String {
    format!("auction/user-login-key/v1/{user_id}/key")
}

fn crypto_error(error: LoginKeyCryptoError) -> ApplicationError {
    let _ = error;
    MailProcessingError::InvalidLoginKey.into()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use auction_application::admin::{AdminLoginKeyIssuer, AdminLoginKeyOpener};
    use auction_application::auction::ApplicationFuture;
    use auction_application::mail::{
        MailLoginKeyEnvelope, MailLoginKeyEnvelopeReader, MailLoginKeyIssuer,
    };
    use auction_domain::DataKeyDomain;
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
    use uuid::Uuid;

    use super::{MoogleMailLoginKeyIssuer, MoogleMailLoginKeyReader};
    use crate::keyring::Keyring;

    fn data_key(id: &str, byte: u8) -> Arc<crate::DataKey> {
        Arc::new(crate::DataKey::from_base64(id, &BASE64.encode([byte; 32])).expect("测试密钥有效"))
    }

    struct StaticEnvelope(MailLoginKeyEnvelope);

    impl MailLoginKeyEnvelopeReader for StaticEnvelope {
        fn load_login_key_envelope(
            &self,
            _user_id: Uuid,
        ) -> ApplicationFuture<'_, MailLoginKeyEnvelope> {
            let envelope = self.0.clone();
            Box::pin(async move { Ok(envelope) })
        }
    }

    #[tokio::test]
    async fn previous_login_key_rotates_to_current() {
        let user_id = Uuid::new_v4();
        let old_issuer = MoogleMailLoginKeyIssuer::new(data_key("user_login-2025-11", 1));
        let issue = MailLoginKeyIssuer::issue(&old_issuer, user_id).await.expect("旧 key 签发成功");
        assert_eq!(issue.envelope.encryption_key_id(), "user_login-2025-11");

        let rotated = Arc::new(
            Keyring::new(
                DataKeyDomain::UserLogin,
                data_key("user_login-2026-11", 2),
                vec![data_key("user_login-2025-11", 1)],
            )
            .expect("轮换密钥环有效"),
        );
        let reader = MoogleMailLoginKeyReader::new(
            Arc::new(StaticEnvelope(issue.envelope.clone())),
            Arc::clone(&rotated),
        );
        let key = AdminLoginKeyOpener::open(&reader, user_id, issue.envelope)
            .await
            .expect("旧 key 仍可解密");
        let new_issuer = MoogleMailLoginKeyIssuer::new(Arc::clone(rotated.current()));
        let resealed =
            AdminLoginKeyIssuer::seal(&new_issuer, user_id, key).await.expect("重新密封成功");
        assert_eq!(resealed.envelope.encryption_key_id(), "user_login-2026-11");

        // 只含新 key 的环无法解密旧信封（失败关闭）。
        let created =
            MailLoginKeyIssuer::issue(&old_issuer, user_id).await.expect("旧 key 再签发成功");
        let stale_envelope = created.envelope.clone();
        let single =
            Arc::new(Keyring::single(DataKeyDomain::UserLogin, data_key("user_login-2026-11", 2)));
        let stale_reader =
            MoogleMailLoginKeyReader::new(Arc::new(StaticEnvelope(stale_envelope.clone())), single);
        assert!(AdminLoginKeyOpener::open(&stale_reader, user_id, stale_envelope).await.is_err());
    }
}
