//! 寄售预约码与受保护证据引用的 AES-256-GCM 信封。
//!
//! AAD 与指纹域都带独立用途前缀，和登记用装备凭证分开；密文字段形状与迁移
//! `consignment_reservation_secrets`、`mail_evidence_envelopes` 一致：密文含 16 字节标签、
//! 12 字节随机 nonce、数据钥 ID 与小写十六进制明文指纹。

use std::fmt;
use std::sync::Arc;

use auction_application::consignment::{
    ReservationSecretAad, ReservationSecretCrypto, ReservationSecretCryptoError,
    SealedReservationSecret,
};
use auction_application::core::lower_hex;
use auction_domain::consignment::{DepositError, TokenText, format_token, parse_token};
use auction_domain::valid_text;
use ring::aead;
use ring::hmac;
use ring::rand::{SecureRandom, SystemRandom};
use subtle::ConstantTimeEq;
use thiserror::Error;
use uuid::Uuid;

use crate::cryptobox::DataKey;
use crate::keyring::{Keyring, KeyringError};

const NONCE_LENGTH: usize = 12;
const TAG_LENGTH: usize = 16;
const RESERVATION_AAD: &[u8] = b"auction/consignment/reservation-secret/v1\0";
const EVIDENCE_AAD: &[u8] = b"auction/consignment/mail-evidence/v1\0";
const RESERVATION_FINGERPRINT: &[u8] =
    b"auction/cryptobox/secret/v1/consignment-reservation-token\0";
const EVIDENCE_FINGERPRINT: &[u8] = b"auction/cryptobox/secret/v1/consignment-mail-evidence\0";

/// 受保护证据的用途闭集；进入 AAD，不能跨用途移植。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MailEvidencePurpose {
    /// 冻结清单。
    Manifest,
    /// 装备凭证引用。
    EquipmentCredential,
    /// 原始观察。
    OriginalObservation,
    /// 隔离证据。
    QuarantineEvidence,
}

impl MailEvidencePurpose {
    /// 稳定 wire 与数据库码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Manifest => "manifest",
            Self::EquipmentCredential => "equipment_credential",
            Self::OriginalObservation => "original_observation",
            Self::QuarantineEvidence => "quarantine_evidence",
        }
    }
}

/// 已封装秘密；形状与数据库列一一对应。
#[derive(Clone, PartialEq, Eq)]
pub struct ConsignmentSecretEnvelope {
    ciphertext: Vec<u8>,
    nonce: [u8; NONCE_LENGTH],
    encryption_key_id: String,
    fingerprint: String,
}

impl ConsignmentSecretEnvelope {
    /// 从持久列重建并校验形状；不验证认证标签。
    pub fn new(
        ciphertext: Vec<u8>,
        nonce: Vec<u8>,
        encryption_key_id: String,
        fingerprint: String,
    ) -> Result<Self, ConsignmentSecretError> {
        if ciphertext.len() < TAG_LENGTH + 1
            || nonce.len() != NONCE_LENGTH
            || !auction_domain::valid_key_id(&encryption_key_id)
            || !valid_fingerprint(&fingerprint)
        {
            return Err(ConsignmentSecretError::InvalidEnvelope);
        }
        let nonce: [u8; NONCE_LENGTH] = match nonce.try_into() {
            Ok(nonce) => nonce,
            Err(error) => {
                // Vec 转换失败只会是长度不符；错误值可能持有原 nonce，直接丢弃。
                drop(error);
                return Err(ConsignmentSecretError::InvalidEnvelope);
            }
        };
        Ok(Self { ciphertext, nonce, encryption_key_id, fingerprint })
    }

    /// 密文含认证标签。
    pub fn ciphertext(&self) -> &[u8] {
        &self.ciphertext
    }

    /// 12 字节 nonce。
    pub const fn nonce(&self) -> &[u8; NONCE_LENGTH] {
        &self.nonce
    }

    /// 数据钥 ID。
    pub fn encryption_key_id(&self) -> &str {
        &self.encryption_key_id
    }

    /// 明文指纹。
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}

impl fmt::Debug for ConsignmentSecretEnvelope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConsignmentSecretEnvelope")
            .field("ciphertext_bytes", &self.ciphertext.len())
            .field("nonce", &"[REDACTED]")
            .field("encryption_key_id", &"[REDACTED]")
            .field("fingerprint", &"[REDACTED]")
            .finish()
    }
}

/// 加解密失败闭集；不携带明文、密钥或信封内容。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsignmentSecretError {
    /// 身份或 AAD 输入无效。
    InvalidInput,
    /// 信封形状无效。
    InvalidEnvelope,
    /// 数据钥 ID 不在当前域密钥环。
    UnknownKey,
    /// 认证标签失败。
    Decryption,
    /// 明文指纹不符。
    FingerprintMismatch,
    /// 预约码明文字节不是 UTF-8。
    InvalidUtf8,
    /// 预约码不规范。
    InvalidToken,
    /// 系统随机源失败。
    RandomSource,
    /// 加密失败。
    Encryption,
    /// 密钥环域不是寄售秘密使用的凭据域。
    WrongDomain,
}

/// 新信封写入器；只持有当前数据钥。
pub struct ConsignmentSecretSealer {
    data_key: Arc<DataKey>,
    random: SystemRandom,
}

impl ConsignmentSecretSealer {
    /// 建立写入器；装配层应传入凭据域当前数据钥。
    pub fn new(data_key: Arc<DataKey>) -> Self {
        Self { data_key, random: SystemRandom::new() }
    }

    /// 封装备预约码；AAD 绑定预约、代次与卖家。
    pub fn seal_reservation(
        &self,
        reservation_id: Uuid,
        generation: u64,
        seller_user_id: Uuid,
        token: &TokenText,
    ) -> Result<ConsignmentSecretEnvelope, ConsignmentSecretError> {
        if reservation_id.is_nil() || generation == 0 || seller_user_id.is_nil() {
            return Err(ConsignmentSecretError::InvalidInput);
        }
        let aad = reservation_aad(reservation_id, generation, seller_user_id);
        self.seal(&aad, RESERVATION_FINGERPRINT, token.as_str().as_bytes())
    }

    /// 封装受保护证据引用；AAD 绑定证据 ID、用途和 owner。
    pub fn seal_evidence(
        &self,
        evidence_id: Uuid,
        purpose: MailEvidencePurpose,
        aad_owner: &str,
        plaintext: &[u8],
    ) -> Result<ConsignmentSecretEnvelope, ConsignmentSecretError> {
        if evidence_id.is_nil() || !valid_text(aad_owner, 128) || plaintext.is_empty() {
            return Err(ConsignmentSecretError::InvalidInput);
        }
        let aad = evidence_aad(evidence_id, purpose, aad_owner)
            .ok_or(ConsignmentSecretError::InvalidInput)?;
        self.seal(&aad, EVIDENCE_FINGERPRINT, plaintext)
    }

    fn seal(
        &self,
        aad: &[u8],
        fingerprint_domain: &[u8],
        plaintext: &[u8],
    ) -> Result<ConsignmentSecretEnvelope, ConsignmentSecretError> {
        let key = self.data_key.aead_key().map_err(encryption_failure)?;
        let mut nonce = [0_u8; NONCE_LENGTH];
        self.random.fill(&mut nonce).map_err(random_source_failure)?;
        let mut ciphertext = Vec::with_capacity(plaintext.len() + TAG_LENGTH);
        ciphertext.extend_from_slice(plaintext);
        let tag = match key.seal_in_place_separate_tag(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(aad),
            &mut ciphertext,
        ) {
            Ok(tag) => tag,
            Err(error) => {
                ciphertext.fill(0);
                return Err(encryption_failure(error));
            }
        };
        ciphertext.extend_from_slice(tag.as_ref());
        let fingerprint = fingerprint(&self.data_key, fingerprint_domain, plaintext);
        Ok(ConsignmentSecretEnvelope {
            ciphertext,
            nonce,
            encryption_key_id: self.data_key.id().to_owned(),
            fingerprint,
        })
    }
}

impl fmt::Debug for ConsignmentSecretSealer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("ConsignmentSecretSealer").finish_non_exhaustive()
    }
}

/// 信封读取器；只接受凭据域密钥环。
pub struct ConsignmentSecretOpener {
    keys: Arc<Keyring>,
}

impl ConsignmentSecretOpener {
    /// 建立读取器；域不符时所有打开操作失败关闭。
    pub fn new(keys: Arc<Keyring>) -> Self {
        Self { keys }
    }

    /// 打开预约码并重新校验规范编码。
    pub fn open_reservation(
        &self,
        reservation_id: Uuid,
        generation: u64,
        seller_user_id: Uuid,
        envelope: &ConsignmentSecretEnvelope,
    ) -> Result<TokenText, ConsignmentSecretError> {
        if reservation_id.is_nil() || generation == 0 || seller_user_id.is_nil() {
            return Err(ConsignmentSecretError::InvalidInput);
        }
        let aad = reservation_aad(reservation_id, generation, seller_user_id);
        let opened = self.open(&aad, RESERVATION_FINGERPRINT, envelope)?;
        let text = std::str::from_utf8(opened.as_bytes()).map_err(invalid_utf8)?;
        let token = parse_token(text).map_err(invalid_token)?;
        Ok(format_token(*token.as_bytes()))
    }

    /// 打开受保护证据引用。
    pub fn open_evidence(
        &self,
        evidence_id: Uuid,
        purpose: MailEvidencePurpose,
        aad_owner: &str,
        envelope: &ConsignmentSecretEnvelope,
    ) -> Result<Vec<u8>, ConsignmentSecretError> {
        if evidence_id.is_nil() || !valid_text(aad_owner, 128) {
            return Err(ConsignmentSecretError::InvalidInput);
        }
        let aad = evidence_aad(evidence_id, purpose, aad_owner)
            .ok_or(ConsignmentSecretError::InvalidInput)?;
        self.open(&aad, EVIDENCE_FINGERPRINT, envelope).map(SensitiveBytes::into_bytes)
    }

    fn open(
        &self,
        aad: &[u8],
        fingerprint_domain: &[u8],
        envelope: &ConsignmentSecretEnvelope,
    ) -> Result<SensitiveBytes, ConsignmentSecretError> {
        if self.keys.domain() != auction_domain::DataKeyDomain::Credential {
            return Err(ConsignmentSecretError::WrongDomain);
        }
        let data_key = self.keys.resolve(envelope.encryption_key_id()).map_err(|error| {
            if matches!(error, KeyringError::UnknownKeyId) {
                ConsignmentSecretError::UnknownKey
            } else {
                ConsignmentSecretError::InvalidEnvelope
            }
        })?;
        let key = data_key.aead_key().map_err(decryption_failure)?;
        let mut plaintext = SensitiveBytes(envelope.ciphertext().to_vec());
        let length = key
            .open_in_place(
                aead::Nonce::assume_unique_for_key(*envelope.nonce()),
                aead::Aad::from(aad),
                &mut plaintext.0,
            )
            .map_err(decryption_failure)?
            .len();
        plaintext.0.truncate(length);
        let actual = fingerprint(data_key, fingerprint_domain, plaintext.as_bytes());
        if !bool::from(actual.as_bytes().ct_eq(envelope.fingerprint().as_bytes())) {
            return Err(ConsignmentSecretError::FingerprintMismatch);
        }
        Ok(plaintext)
    }
}

impl fmt::Debug for ConsignmentSecretOpener {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConsignmentSecretOpener")
            .field("key_count", &self.keys.len())
            .finish()
    }
}

/// 4D 的预约令牌密码适配器：组合现有封装备与读取器，实现应用层端口。
pub struct ConsignmentReservationCrypto {
    sealer: ConsignmentSecretSealer,
    opener: ConsignmentSecretOpener,
}

impl ConsignmentReservationCrypto {
    /// 组合凭据域当前数据钥与密钥环。
    pub fn new(sealer: ConsignmentSecretSealer, opener: ConsignmentSecretOpener) -> Self {
        Self { sealer, opener }
    }
}

impl ReservationSecretCrypto for ConsignmentReservationCrypto {
    fn seal(
        &self,
        token: &TokenText,
        aad: ReservationSecretAad,
    ) -> Result<SealedReservationSecret, ReservationSecretCryptoError> {
        let envelope = self.sealer.seal_reservation(
            aad.reservation_id,
            aad.generation,
            aad.seller_user_id,
            token,
        )?;
        SealedReservationSecret::new(
            envelope.ciphertext().to_vec(),
            *envelope.nonce(),
            envelope.encryption_key_id().to_owned(),
            envelope.fingerprint().to_owned(),
        )
    }

    fn open(
        &self,
        sealed: &SealedReservationSecret,
        aad: ReservationSecretAad,
    ) -> Result<TokenText, ReservationSecretCryptoError> {
        let envelope = ConsignmentSecretEnvelope::new(
            sealed.ciphertext().to_vec(),
            sealed.nonce().to_vec(),
            sealed.encryption_key_id().to_owned(),
            sealed.fingerprint().to_owned(),
        )?;
        Ok(self.opener.open_reservation(
            aad.reservation_id,
            aad.generation,
            aad.seller_user_id,
            &envelope,
        )?)
    }
}

/// 预约码加密器的启动配置错误；固定文案不回显密钥材料。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ConsignmentReservationCryptoConfigError {
    /// 寄售激活但凭据域密钥环缺失。
    #[error(
        "寄售已启用，但缺少 credential 数据密钥；请在 secrets.toml 配置 credential_data_key_id 与 credential_data_key"
    )]
    MissingCredentialKey,
}

/// 按寄售是否激活与已加载的凭据域密钥环构造预约码加密器。
///
/// 未激活时不要求密钥并返回 `None`；激活且缺少密钥环返回固定错误，调用方必须拒绝启动。
/// 该判定只读取已加载的密钥环，不回显密钥 ID 或材料。
pub fn consignment_reservation_crypto(
    enabled: bool,
    keyring: Option<Arc<Keyring>>,
) -> Result<Option<Arc<ConsignmentReservationCrypto>>, ConsignmentReservationCryptoConfigError> {
    if !enabled {
        return Ok(None);
    }
    let keyring = keyring.ok_or(ConsignmentReservationCryptoConfigError::MissingCredentialKey)?;
    Ok(Some(Arc::new(ConsignmentReservationCrypto::new(
        ConsignmentSecretSealer::new(Arc::clone(keyring.current())),
        ConsignmentSecretOpener::new(keyring),
    ))))
}

impl From<ConsignmentSecretError> for ReservationSecretCryptoError {
    fn from(_error: ConsignmentSecretError) -> Self {
        // 端口只暴露固定不可用分类；底层认证、密钥或编码细节不跨装配边界。
        Self
    }
}

impl fmt::Debug for ConsignmentReservationCrypto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("ConsignmentReservationCrypto").finish_non_exhaustive()
    }
}

struct SensitiveBytes(Vec<u8>);

impl SensitiveBytes {
    fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    fn into_bytes(mut self) -> Vec<u8> {
        std::mem::take(&mut self.0)
    }
}

impl Drop for SensitiveBytes {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

fn reservation_aad(reservation_id: Uuid, generation: u64, seller_user_id: Uuid) -> Vec<u8> {
    let mut aad = Vec::with_capacity(RESERVATION_AAD.len() + 32);
    aad.extend_from_slice(RESERVATION_AAD);
    aad.extend_from_slice(reservation_id.as_bytes());
    aad.extend_from_slice(&generation.to_be_bytes());
    aad.extend_from_slice(seller_user_id.as_bytes());
    aad
}

fn encryption_failure(_: ring::error::Unspecified) -> ConsignmentSecretError {
    ConsignmentSecretError::Encryption
}

fn random_source_failure(_: ring::error::Unspecified) -> ConsignmentSecretError {
    ConsignmentSecretError::RandomSource
}

fn decryption_failure(_: ring::error::Unspecified) -> ConsignmentSecretError {
    ConsignmentSecretError::Decryption
}

fn invalid_utf8(_: std::str::Utf8Error) -> ConsignmentSecretError {
    ConsignmentSecretError::InvalidUtf8
}

fn invalid_token(_: DepositError) -> ConsignmentSecretError {
    ConsignmentSecretError::InvalidToken
}

fn evidence_aad(
    evidence_id: Uuid,
    purpose: MailEvidencePurpose,
    aad_owner: &str,
) -> Option<Vec<u8>> {
    if !valid_text(aad_owner, 128) {
        return None;
    }
    let purpose = purpose.as_str();
    let mut aad =
        Vec::with_capacity(EVIDENCE_AAD.len() + 16 + 4 + purpose.len() + 4 + aad_owner.len());
    aad.extend_from_slice(EVIDENCE_AAD);
    aad.extend_from_slice(evidence_id.as_bytes());
    aad.extend_from_slice(&u32::try_from(purpose.len()).ok()?.to_be_bytes());
    aad.extend_from_slice(purpose.as_bytes());
    aad.extend_from_slice(&u32::try_from(aad_owner.len()).ok()?.to_be_bytes());
    aad.extend_from_slice(aad_owner.as_bytes());
    Some(aad)
}

fn fingerprint(data_key: &DataKey, domain: &[u8], plaintext: &[u8]) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA256, data_key.material());
    let mut context = hmac::Context::with_key(&key);
    context.update(domain);
    context.update(plaintext);
    lower_hex(context.sign().as_ref())
}

fn valid_fingerprint(value: &str) -> bool {
    value.len() == 64
        && value.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use auction_application::consignment::{ReservationSecretAad, ReservationSecretCrypto};
    use auction_domain::DataKeyDomain;
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
    use uuid::Uuid;

    use super::ConsignmentReservationCrypto;
    use super::ConsignmentReservationCryptoConfigError;
    use super::ConsignmentSecretEnvelope;
    use super::ConsignmentSecretError;
    use super::ConsignmentSecretOpener;
    use super::ConsignmentSecretSealer;
    use super::MailEvidencePurpose;
    use super::consignment_reservation_crypto;
    use crate::cryptobox::DataKey;
    use crate::keyring::Keyring;

    #[test]
    fn envelope_shape_requires_nonce_key_id_fingerprint_and_tagged_ciphertext() {
        let key_id = "fixture-consign-key".to_owned();
        let fingerprint = "a".repeat(64);
        assert!(
            ConsignmentSecretEnvelope::new(
                vec![0; 17],
                vec![0; 12],
                key_id.clone(),
                fingerprint.clone()
            )
            .is_ok()
        );
        assert_eq!(
            ConsignmentSecretEnvelope::new(
                vec![0; 17],
                vec![0; 11],
                key_id.clone(),
                fingerprint.clone()
            ),
            Err(ConsignmentSecretError::InvalidEnvelope)
        );
        assert_eq!(
            ConsignmentSecretEnvelope::new(
                vec![0; 16],
                vec![0; 12],
                key_id.clone(),
                fingerprint.clone()
            ),
            Err(ConsignmentSecretError::InvalidEnvelope)
        );
        assert_eq!(
            ConsignmentSecretEnvelope::new(
                vec![0; 17],
                vec![0; 12],
                key_id,
                fingerprint.to_ascii_uppercase()
            ),
            Err(ConsignmentSecretError::InvalidEnvelope)
        );
    }

    #[test]
    fn reservation_and_evidence_envelopes_bind_purpose_and_owner() {
        let key = Arc::new(
            DataKey::from_base64("fixture-consign-key", &BASE64.encode([3_u8; 32]))
                .expect("固定数据钥"),
        );
        let sealer = ConsignmentSecretSealer::new(Arc::clone(&key));
        let opener = ConsignmentSecretOpener::new(Arc::new(Keyring::single(
            DataKeyDomain::Credential,
            Arc::clone(&key),
        )));
        let token = auction_domain::consignment::format_token([7_u8; 32]);
        let reservation_id = Uuid::from_u128(11);
        let seller_id = Uuid::from_u128(12);
        let envelope =
            sealer.seal_reservation(reservation_id, 3, seller_id, &token).expect("封装备预约");
        assert_eq!(envelope.nonce().len(), 12);
        assert_eq!(envelope.encryption_key_id(), "fixture-consign-key");
        assert_eq!(
            opener
                .open_reservation(reservation_id, 3, seller_id, &envelope)
                .expect("打开预约")
                .as_str(),
            token.as_str()
        );
        assert_eq!(
            opener.open_reservation(reservation_id, 4, seller_id, &envelope),
            Err(ConsignmentSecretError::Decryption)
        );
        assert_eq!(
            opener.open_reservation(reservation_id, 3, Uuid::from_u128(13), &envelope),
            Err(ConsignmentSecretError::Decryption)
        );
        let wrong_domain = ConsignmentSecretOpener::new(Arc::new(Keyring::single(
            DataKeyDomain::Equipment,
            Arc::clone(&key),
        )));
        assert_eq!(
            wrong_domain.open_reservation(reservation_id, 3, seller_id, &envelope),
            Err(ConsignmentSecretError::WrongDomain)
        );

        let evidence = sealer
            .seal_evidence(
                Uuid::from_u128(21),
                MailEvidencePurpose::Manifest,
                "fixture-owner",
                b"fixture evidence",
            )
            .expect("封装备证据");
        assert_eq!(
            opener
                .open_evidence(
                    Uuid::from_u128(21),
                    MailEvidencePurpose::Manifest,
                    "fixture-owner",
                    &evidence,
                )
                .expect("打开证据"),
            b"fixture evidence"
        );
        assert!(
            opener
                .open_evidence(
                    Uuid::from_u128(21),
                    MailEvidencePurpose::EquipmentCredential,
                    "fixture-owner",
                    &evidence,
                )
                .is_err()
        );
        assert!(
            opener
                .open_evidence(
                    Uuid::from_u128(21),
                    MailEvidencePurpose::Manifest,
                    "other-owner",
                    &evidence,
                )
                .is_err()
        );
        let max_owner = "o".repeat(128);
        assert!(
            sealer
                .seal_evidence(
                    Uuid::from_u128(22),
                    MailEvidencePurpose::Manifest,
                    &max_owner,
                    b"fixture evidence",
                )
                .is_ok()
        );
        assert_eq!(
            sealer.seal_evidence(
                Uuid::from_u128(22),
                MailEvidencePurpose::Manifest,
                &"o".repeat(129),
                b"fixture evidence",
            ),
            Err(ConsignmentSecretError::InvalidInput)
        );
        let debug = format!("{evidence:?}");
        assert!(!debug.contains(evidence.fingerprint()));
        assert!(!debug.contains(evidence.encryption_key_id()));
    }

    #[test]
    fn reservation_port_binds_reservation_generation_and_seller_and_redacts_debug() {
        let key = Arc::new(
            DataKey::from_base64("fixture-reservation-key", &BASE64.encode([5_u8; 32]))
                .expect("固定数据钥"),
        );
        let crypto = ConsignmentReservationCrypto::new(
            ConsignmentSecretSealer::new(Arc::clone(&key)),
            ConsignmentSecretOpener::new(Arc::new(Keyring::single(
                DataKeyDomain::Credential,
                Arc::clone(&key),
            ))),
        );
        let token = auction_domain::consignment::format_token([9_u8; 32]);
        let aad = ReservationSecretAad {
            reservation_id: Uuid::from_u128(31),
            generation: 4,
            seller_user_id: Uuid::from_u128(32),
        };
        let sealed = crypto.seal(&token, aad).expect("封装备预约");
        assert_eq!(crypto.open(&sealed, aad).expect("打开预约").as_str(), token.as_str());
        for changed in [
            ReservationSecretAad { reservation_id: Uuid::from_u128(33), ..aad },
            ReservationSecretAad { generation: 5, ..aad },
            ReservationSecretAad { seller_user_id: Uuid::from_u128(34), ..aad },
        ] {
            assert_eq!(
                crypto.open(&sealed, changed),
                Err(auction_application::consignment::ReservationSecretCryptoError)
            );
        }
        let debug = format!("{sealed:?}");
        assert!(!debug.contains(token.as_str()));
        assert!(!debug.contains(sealed.encryption_key_id()));
        assert!(!debug.contains(sealed.fingerprint()));
        assert!(!format!("{crypto:?}").contains(sealed.encryption_key_id()));
    }

    #[test]
    fn active_reservation_crypto_without_credential_key_fails_closed() {
        let error = consignment_reservation_crypto(true, None)
            .expect_err("激活时缺少凭据域密钥必须拒绝启动");
        assert_eq!(error, ConsignmentReservationCryptoConfigError::MissingCredentialKey);
        let display = error.to_string();
        assert!(display.contains("credential_data_key"));
        assert!(!display.contains("fixture-reservation-key"));
        assert!(!format!("{error:?}").contains("fixture-reservation-key"));
    }

    #[test]
    fn inactive_reservation_crypto_does_not_require_a_key() {
        assert!(consignment_reservation_crypto(false, None).expect("未激活时不要求密钥").is_none());
    }
}
