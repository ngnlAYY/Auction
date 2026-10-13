//! 外部凭据密封适配器。
//!
//! mail_cookie 与 forum_cookie 使用不同的 AAD 和指纹域，即使误用同一份数据密钥也不能
//! 跨用途解密或通过指纹校验。该模块不负责读取配置或数据库，只接收装配根提供的
//! DataKey，并把明文转换为应用层的不透明密文信封。

use std::{fmt, sync::Arc};

use auction_application::{
    AdminCredentialEnvelope, AdminCredentialError, AdminCredentialKind, AdminCredentialOpener,
    AdminCredentialSealer, ApplicationError,
};
use ring::{
    aead, hmac,
    rand::{SecureRandom, SystemRandom},
};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::DataKey;

mod check;
pub use check::CredentialVerifier;

const NONCE_LENGTH: usize = 12;
const MAIL_AAD_PREFIX: &str = "auction/credential/mail/v1/";
const FORUM_AAD_PREFIX: &str = "auction/credential/forum/v1/";
const MAIL_FINGERPRINT_DOMAIN: &[u8] = b"auction/cryptobox/secret/v1/mooglemail-cookie\0";
const FORUM_FINGERPRINT_DOMAIN: &[u8] = b"auction/cryptobox/secret/v1/ehentai-cookie\0";

/// Admin 外部凭据密封器。
pub struct CredentialSealer {
    data_key: Arc<DataKey>,
    random: SystemRandom,
}

impl CredentialSealer {
    /// 使用独立于装备和用户登录 key 的数据密钥建立密封器。
    pub fn new(data_key: Arc<DataKey>) -> Self {
        Self { data_key, random: SystemRandom::new() }
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
        self.random.fill(&mut nonce).map_err(|_| crypto_error())?;
        let key = self.data_key.aead_key().map_err(|_| crypto_error())?;
        let mut ciphertext = secret.to_vec();
        key.seal_in_place_append_tag(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(aad.as_bytes()),
            &mut ciphertext,
        )
        .map_err(|_| crypto_error())?;
        let fingerprint = fingerprint(&self.data_key, kind, secret);
        AdminCredentialEnvelope::new(
            ciphertext,
            nonce.to_vec(),
            self.data_key.id().to_owned(),
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
            || envelope.encryption_key_id() != self.data_key.id()
        {
            return Err(crypto_error());
        }
        let aad = credential_aad(kind, credential_id, scope);
        let key = self.data_key.aead_key().map_err(|_| crypto_error())?;
        let mut ciphertext = envelope.ciphertext().to_vec();
        let plaintext = key
            .open_in_place(
                aead::Nonce::try_assume_unique_for_key(envelope.nonce())
                    .map_err(|_| crypto_error())?,
                aead::Aad::from(aad.as_bytes()),
                &mut ciphertext,
            )
            .map_err(|_| crypto_error())?;
        let secret = plaintext.to_vec();
        let actual_fingerprint = fingerprint(&self.data_key, kind, &secret);
        if actual_fingerprint.as_bytes().ct_eq(envelope.fingerprint().as_bytes()).into() {
            ciphertext.fill(0);
            Ok(secret)
        } else {
            ciphertext.fill(0);
            let mut secret = secret;
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
            .field("key_id", &self.data_key.id())
            .field("key_fingerprint", &self.data_key.fingerprint())
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
    let mut input = Vec::with_capacity(domain.len() + secret.len());
    input.extend_from_slice(domain);
    input.extend_from_slice(secret);
    hex_encode(hmac::sign(&key, &input).as_ref())
}

fn crypto_error() -> ApplicationError {
    ApplicationError::AdminCredential(AdminCredentialError::CryptoUnavailable)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}
