//! 受限权利挑战码的 AES-256-GCM 封装与 HMAC-SHA256 认证。
//!
//! 封装用途前缀、MAC 域前缀与指纹域前缀互不相同，也与预约码、证据信封分开；AAD 绑定
//! challenge、权利、冻结身份摘要与权利版本，换任一字段都解不开、验不过。

use std::fmt;
use std::sync::Arc;

use auction_application::consignment::{
    ChallengeAad, RightChallengeCrypto, RightCryptoError, SealedChallengeSecret,
};
use auction_application::core::lower_hex;
use auction_domain::DataKeyDomain;
use auction_domain::consignment::rights::{CHALLENGE_CODE_ENTROPY_BYTES, ChallengeCode};
use ring::aead;
use ring::hmac;
use ring::rand::{SecureRandom, SystemRandom};
use subtle::ConstantTimeEq;

use crate::cryptobox::DataKey;
use crate::keyring::Keyring;

const NONCE_LENGTH: usize = 12;
const TAG_LENGTH: usize = 16;
const SEAL_AAD: &[u8] = b"auction/consignment/right-challenge-seal/v1\0";
const MAC_DOMAIN: &[u8] = b"auction/consignment/right-challenge-mac/v1\0";
const FINGERPRINT_DOMAIN: &[u8] = b"auction/cryptobox/secret/v1/consignment-right-challenge\0";

/// 挑战码密码适配器；密钥环必须是凭据域，其他域一律失败关闭。
pub struct ConsignmentRightChallengeCrypto {
    keys: Arc<Keyring>,
    random: SystemRandom,
}

impl ConsignmentRightChallengeCrypto {
    /// 用凭据域密钥环建立适配器；封装与 MAC 用当前数据钥，解封按信封里的钥标识。
    pub fn new(keys: Arc<Keyring>) -> Self {
        Self { keys, random: SystemRandom::new() }
    }

    fn require_credential_domain(&self) -> Result<(), RightCryptoError> {
        if self.keys.domain() != DataKeyDomain::Credential {
            return Err(RightCryptoError);
        }
        Ok(())
    }
}

impl RightChallengeCrypto for ConsignmentRightChallengeCrypto {
    fn seal(
        &self,
        code: &ChallengeCode,
        aad: ChallengeAad,
    ) -> Result<SealedChallengeSecret, RightCryptoError> {
        self.require_credential_domain()?;
        let data_key = self.keys.current();
        let key = data_key.aead_key().map_err(unavailable)?;
        let mut nonce = [0_u8; NONCE_LENGTH];
        self.random.fill(&mut nonce).map_err(unavailable)?;
        let mut ciphertext = Vec::with_capacity(CHALLENGE_CODE_ENTROPY_BYTES + TAG_LENGTH);
        ciphertext.extend_from_slice(code.as_bytes());
        let tag = key
            .seal_in_place_separate_tag(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(seal_aad(&aad)),
                &mut ciphertext,
            )
            .map_err(unavailable);
        let tag = match tag {
            Ok(tag) => tag,
            Err(error) => {
                ciphertext.fill(0);
                return Err(error);
            }
        };
        ciphertext.extend_from_slice(tag.as_ref());
        SealedChallengeSecret::new(
            ciphertext,
            nonce,
            data_key.id().to_owned(),
            fingerprint(data_key, code.as_bytes()),
        )
    }

    fn open(
        &self,
        sealed: &SealedChallengeSecret,
        aad: ChallengeAad,
    ) -> Result<ChallengeCode, RightCryptoError> {
        self.require_credential_domain()?;
        let data_key = self.keys.resolve(sealed.encryption_key_id()).map_err(unavailable)?;
        let key = data_key.aead_key().map_err(unavailable)?;
        if sealed.ciphertext().len() != CHALLENGE_CODE_ENTROPY_BYTES + TAG_LENGTH {
            return Err(RightCryptoError);
        }
        let mut buffer = sealed.ciphertext().to_vec();
        let opened = key
            .open_in_place(
                aead::Nonce::assume_unique_for_key(*sealed.nonce()),
                aead::Aad::from(seal_aad(&aad)),
                &mut buffer,
            )
            .map_err(unavailable)
            .and_then(|plaintext| {
                <[u8; CHALLENGE_CODE_ENTROPY_BYTES]>::try_from(&*plaintext).map_err(unavailable)
            });
        buffer.fill(0);
        let mut entropy = opened?;
        let actual = fingerprint(data_key, &entropy);
        let matches = bool::from(actual.as_bytes().ct_eq(sealed.fingerprint().as_bytes()));
        let code = ChallengeCode::from_entropy(entropy);
        entropy.fill(0);
        if !matches {
            return Err(RightCryptoError);
        }
        Ok(code)
    }

    /// MAC 用当前数据钥；轮换只会让轮换前签发、仍在 10 分钟有效期内的挑战验证失败。
    fn mac(&self, aad: ChallengeAad, code: &ChallengeCode) -> Result<[u8; 32], RightCryptoError> {
        self.require_credential_domain()?;
        let key = hmac::Key::new(hmac::HMAC_SHA256, self.keys.current().material());
        let mut context = hmac::Context::with_key(&key);
        context.update(MAC_DOMAIN);
        context.update(&aad_bytes(&aad));
        context.update(code.as_bytes());
        context.sign().as_ref().try_into().map_err(unavailable)
    }
}

impl fmt::Debug for ConsignmentRightChallengeCrypto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("ConsignmentRightChallengeCrypto").finish_non_exhaustive()
    }
}

/// 定长字段顺序编码；虚拟目标（无权利）以标记字节与有权利的编码区分。
fn aad_bytes(aad: &ChallengeAad) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(16 + 17 + 32 + 8);
    bytes.extend_from_slice(aad.challenge_id.as_bytes());
    match aad.right_id {
        Some(right_id) => {
            bytes.push(1);
            bytes.extend_from_slice(right_id.as_bytes());
        }
        None => bytes.push(0),
    }
    bytes.extend_from_slice(&aad.identity_digest);
    bytes.extend_from_slice(&aad.right_version.to_be_bytes());
    bytes
}

fn seal_aad(aad: &ChallengeAad) -> Vec<u8> {
    let mut bytes = SEAL_AAD.to_vec();
    bytes.extend_from_slice(&aad_bytes(aad));
    bytes
}

fn fingerprint(data_key: &DataKey, plaintext: &[u8]) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA256, data_key.material());
    let mut context = hmac::Context::with_key(&key);
    context.update(FINGERPRINT_DOMAIN);
    context.update(plaintext);
    lower_hex(context.sign().as_ref())
}

fn unavailable<E>(_error: E) -> RightCryptoError {
    // 不变量 7：底层错误链不跨端口边界，只保留不可用分类。
    RightCryptoError
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use auction_application::consignment::{ChallengeAad, RightChallengeCrypto};
    use auction_domain::DataKeyDomain;
    use auction_domain::consignment::rights::ChallengeCode;
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
    use ring::{aead, hmac};
    use uuid::Uuid;

    use super::{ConsignmentRightChallengeCrypto, aad_bytes};
    use crate::cryptobox::DataKey;
    use crate::keyring::Keyring;

    fn data_key(id: &str, fill: u8) -> Arc<DataKey> {
        Arc::new(DataKey::from_base64(id, &BASE64.encode([fill; 32])).expect("固定数据钥"))
    }

    fn crypto_with(domain: DataKeyDomain, key: Arc<DataKey>) -> ConsignmentRightChallengeCrypto {
        ConsignmentRightChallengeCrypto::new(Arc::new(Keyring::single(domain, key)))
    }

    fn crypto() -> ConsignmentRightChallengeCrypto {
        crypto_with(DataKeyDomain::Credential, data_key("fixture-right-key", 7))
    }

    fn aad() -> ChallengeAad {
        ChallengeAad {
            challenge_id: Uuid::from_u128(1),
            right_id: Some(Uuid::from_u128(2)),
            identity_digest: [3; 32],
            right_version: 4,
        }
    }

    fn changed_aads() -> [ChallengeAad; 5] {
        [
            ChallengeAad { challenge_id: Uuid::from_u128(9), ..aad() },
            ChallengeAad { right_id: Some(Uuid::from_u128(9)), ..aad() },
            ChallengeAad { right_id: None, ..aad() },
            ChallengeAad { identity_digest: [9; 32], ..aad() },
            ChallengeAad { right_version: 5, ..aad() },
        ]
    }

    #[test]
    fn seal_round_trips_and_any_aad_change_fails_to_open() {
        let crypto = crypto();
        let code = ChallengeCode::from_entropy([0x42; 16]);
        let sealed = crypto.seal(&code, aad()).expect("封装挑战码");
        assert_eq!(crypto.open(&sealed, aad()).expect("原 AAD 解封").as_bytes(), code.as_bytes());
        for changed in changed_aads() {
            assert!(crypto.open(&sealed, changed).is_err());
        }
    }

    #[test]
    fn mac_is_bound_to_every_aad_field_and_the_code() {
        let crypto = crypto();
        let code = ChallengeCode::from_entropy([0x42; 16]);
        let tag = crypto.mac(aad(), &code).expect("MAC");
        assert_eq!(crypto.mac(aad(), &code).expect("MAC"), tag);
        for changed in changed_aads() {
            assert_ne!(crypto.mac(changed, &code).expect("MAC"), tag);
        }
        let other = ChallengeCode::from_entropy([0x43; 16]);
        assert_ne!(crypto.mac(aad(), &other).expect("MAC"), tag);
    }

    /// 冻结 MAC 的字节格式：MAC 会持久化，域前缀或 AAD 布局漂移都会让该向量失败。
    #[test]
    fn mac_known_vector_is_frozen() {
        let code = ChallengeCode::from_entropy([0x42; 16]);
        let tag = crypto().mac(aad(), &code).expect("MAC");
        let hex: String = tag.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(hex, "c44e8da083dfc80c131e8d9b6f96a60383cc361e624031446021df9c61234aad");
    }

    /// 用同一数据钥原文复现 AEAD，检验封装用途前缀不可与预约密文互换。
    #[test]
    fn seal_purpose_prefix_is_not_interchangeable_with_reservation_secret() {
        const SEAL_PREFIX: &[u8] = b"auction/consignment/right-challenge-seal/v1\0";
        const RESERVATION_PREFIX: &[u8] = b"auction/consignment/reservation-secret/v1\0";
        let key = || {
            aead::LessSafeKey::new(
                aead::UnboundKey::new(&aead::AES_256_GCM, &[7; 32]).expect("固定测试密钥"),
            )
        };
        let with_prefix = |prefix: &[u8]| [prefix, &aad_bytes(&aad())].concat();
        let code = ChallengeCode::from_entropy([0x42; 16]);
        let crypto = crypto();
        let sealed = crypto.seal(&code, aad()).expect("封装挑战码");

        let open_raw = |prefix: &[u8]| {
            let mut buffer = sealed.ciphertext().to_vec();
            key()
                .open_in_place(
                    aead::Nonce::assume_unique_for_key(*sealed.nonce()),
                    aead::Aad::from(with_prefix(prefix)),
                    &mut buffer,
                )
                .is_ok()
        };
        assert!(open_raw(SEAL_PREFIX), "封装 AAD 必须带本用途前缀");
        assert!(!open_raw(RESERVATION_PREFIX));

        let nonce = [5_u8; 12];
        let mut reservation_like = code.as_bytes().to_vec();
        let tag = key()
            .seal_in_place_separate_tag(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(with_prefix(RESERVATION_PREFIX)),
                &mut reservation_like,
            )
            .expect("预约前缀封装");
        reservation_like.extend_from_slice(tag.as_ref());
        let foreign = auction_application::consignment::SealedChallengeSecret::new(
            reservation_like,
            nonce,
            sealed.encryption_key_id().to_owned(),
            sealed.fingerprint().to_owned(),
        )
        .expect("形状合法");
        assert!(crypto.open(&foreign, aad()).is_err());
    }

    /// 同一 AAD 与码下，MAC 用途的输出不同于以封装前缀计算的同构 HMAC。
    #[test]
    fn mac_purpose_differs_from_seal_purpose() {
        let code = ChallengeCode::from_entropy([0x42; 16]);
        let tag = crypto().mac(aad(), &code).expect("MAC");
        let key = hmac::Key::new(hmac::HMAC_SHA256, &[7; 32]);
        let mut context = hmac::Context::with_key(&key);
        context.update(b"auction/consignment/right-challenge-seal/v1\0");
        context.update(&aad_bytes(&aad()));
        context.update(code.as_bytes());
        assert_ne!(context.sign().as_ref(), tag.as_slice());
    }

    #[test]
    fn non_credential_domain_and_foreign_keys_fail_closed() {
        let code = ChallengeCode::from_entropy([0x42; 16]);
        let sealed = crypto().seal(&code, aad()).expect("封装挑战码");
        let wrong_domain = crypto_with(DataKeyDomain::Equipment, data_key("fixture-right-key", 7));
        assert!(wrong_domain.seal(&code, aad()).is_err());
        assert!(wrong_domain.open(&sealed, aad()).is_err());
        assert!(wrong_domain.mac(aad(), &code).is_err());
        let other_key = crypto_with(DataKeyDomain::Credential, data_key("fixture-right-key", 8));
        assert!(other_key.open(&sealed, aad()).is_err());
        let unknown_id = crypto_with(DataKeyDomain::Credential, data_key("fixture-other-key", 7));
        assert!(unknown_id.open(&sealed, aad()).is_err());
    }

    #[test]
    fn rotated_keyring_still_opens_old_envelopes() {
        let code = ChallengeCode::from_entropy([0x42; 16]);
        let old = data_key("fixture-right-key", 7);
        let sealed = crypto().seal(&code, aad()).expect("封装挑战码");
        let rotated = ConsignmentRightChallengeCrypto::new(Arc::new(
            Keyring::new(DataKeyDomain::Credential, data_key("fixture-new-key", 9), vec![old])
                .expect("轮换密钥环"),
        ));
        assert_eq!(
            rotated.open(&sealed, aad()).expect("旧信封仍可解封").as_bytes(),
            code.as_bytes()
        );
    }

    #[test]
    fn tampered_ciphertext_fails_and_debug_hides_secrets() {
        let crypto = crypto();
        let code = ChallengeCode::from_entropy([0x42; 16]);
        let sealed = crypto.seal(&code, aad()).expect("封装挑战码");
        let mut bytes = sealed.ciphertext().to_vec();
        bytes[0] ^= 1;
        let tampered = auction_application::consignment::SealedChallengeSecret::new(
            bytes,
            *sealed.nonce(),
            sealed.encryption_key_id().to_owned(),
            sealed.fingerprint().to_owned(),
        )
        .expect("形状合法");
        assert!(crypto.open(&tampered, aad()).is_err());
        let wrong_fingerprint = auction_application::consignment::SealedChallengeSecret::new(
            sealed.ciphertext().to_vec(),
            *sealed.nonce(),
            sealed.encryption_key_id().to_owned(),
            "0".repeat(64),
        )
        .expect("形状合法");
        assert!(crypto.open(&wrong_fingerprint, aad()).is_err());
        let debug = format!("{sealed:?}{crypto:?}{code:?}");
        assert!(!debug.contains("fixture-right-key"));
        assert!(!debug.contains(sealed.fingerprint()));
    }
}
