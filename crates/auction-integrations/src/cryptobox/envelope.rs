//! 装备 key、外部 Cookie 与用户登录 key 共用的 AES-256-GCM 秘密信封原语：系统随机 12 字节
//! nonce、按用途分域的 HMAC-SHA256 明文指纹、恒定时间指纹比较与明文缓冲尽力清零。
//! AAD 拼接、密钥环解析与错误映射留在各适配器。已存信封依赖这里的字节格式（指纹域、nonce
//! 长度、追加标签），改变计算结果只能另立新的信封版本。

use ring::aead;
use ring::hmac;
use ring::rand::{SecureRandom, SystemRandom};
use subtle::ConstantTimeEq;

use super::DataKey;
use crate::safe_diagnostics;

/// AES-256-GCM nonce 字节数。
const NONCE_LENGTH: usize = 12;

/// 秘密用途：决定指纹域与诊断标签。
#[derive(Clone, Copy)]
pub(crate) enum SecretPurpose {
    EquipmentKey,
    MoogleMailCookie,
    EhentaiCookie,
    UserLoginKey,
}

impl SecretPurpose {
    /// 指纹域：`auction/cryptobox/secret/v1/<用途>` 加 NUL 分隔，其后接明文；字节不得修改。
    const fn fingerprint_domain(self) -> &'static [u8] {
        match self {
            Self::EquipmentKey => b"auction/cryptobox/secret/v1/equipment-key\0",
            Self::MoogleMailCookie => b"auction/cryptobox/secret/v1/mooglemail-cookie\0",
            Self::EhentaiCookie => b"auction/cryptobox/secret/v1/ehentai-cookie\0",
            Self::UserLoginKey => b"auction/cryptobox/secret/v1/user-login-key\0",
        }
    }

    /// 沿用各适配器原有的诊断 kind 与加解密 phase，日志检索口径不随收敛改变。
    const fn diagnostic_kind(self) -> &'static str {
        match self {
            Self::EquipmentKey => "equipment_crypto",
            Self::MoogleMailCookie | Self::EhentaiCookie => "credential_crypto",
            Self::UserLoginKey => "login_key",
        }
    }

    const fn encrypt_phase(self) -> &'static str {
        match self {
            Self::EquipmentKey | Self::UserLoginKey => "encrypt_key",
            Self::MoogleMailCookie | Self::EhentaiCookie => "encrypt_secret",
        }
    }

    const fn decrypt_phase(self) -> &'static str {
        match self {
            Self::EquipmentKey | Self::UserLoginKey => "decrypt_key",
            Self::MoogleMailCookie | Self::EhentaiCookie => "decrypt_secret",
        }
    }
}

/// 新信封的持久化字段；密钥标识由调用方按当前写入密钥填写。
pub(crate) struct SealedSecret {
    pub(crate) ciphertext: Vec<u8>,
    pub(crate) nonce: [u8; NONCE_LENGTH],
    pub(crate) fingerprint: String,
}

/// 加密失败的闭集；原因已按用途以 ERROR 记录。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SealError {
    RandomSource,
    CipherKey,
    Encryption,
}

/// 解密失败的闭集；认证与指纹失败一律失败关闭，不返回任何明文。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OpenError {
    InvalidNonce,
    CipherKey,
    Authentication,
    FingerprintMismatch,
}

/// 已通过认证标签与指纹校验的明文；离开作用域时尽力清零。
pub(crate) struct OpenedSecret(Vec<u8>);

impl OpenedSecret {
    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// 移交明文缓冲；接收方承担清零责任。
    pub(crate) fn into_bytes(mut self) -> Vec<u8> {
        std::mem::take(&mut self.0)
    }
}

impl Drop for OpenedSecret {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

/// 以系统随机 nonce 加密并计算明文指纹；nonce 只在此生成，调用方无法指定或复用。
pub(crate) fn seal_secret(
    data_key: &DataKey,
    purpose: SecretPurpose,
    aad: &[u8],
    plaintext: &[u8],
    random: &SystemRandom,
) -> Result<SealedSecret, SealError> {
    let mut nonce = [0_u8; NONCE_LENGTH];
    random.fill(&mut nonce).map_err(safe_diagnostics::error_map(
        purpose.diagnostic_kind(),
        "generate_nonce",
        || SealError::RandomSource,
    ))?;
    seal_with_nonce(data_key, purpose, aad, nonce, plaintext)
}

fn seal_with_nonce(
    data_key: &DataKey,
    purpose: SecretPurpose,
    aad: &[u8],
    nonce: [u8; NONCE_LENGTH],
    plaintext: &[u8],
) -> Result<SealedSecret, SealError> {
    let kind = purpose.diagnostic_kind();
    let key = data_key.aead_key().map_err(safe_diagnostics::error_map(
        kind,
        "prepare_encryption_key",
        || SealError::CipherKey,
    ))?;
    // 预留认证标签，避免追加标签时扩容并复制整份加密缓冲。
    let mut ciphertext = Vec::with_capacity(plaintext.len() + aead::AES_256_GCM.tag_len());
    ciphertext.extend_from_slice(plaintext);
    if let Err(error) = key
        .seal_in_place_append_tag(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(aad),
            &mut ciphertext,
        )
        .map_err(safe_diagnostics::error_map(kind, purpose.encrypt_phase(), || {
            SealError::Encryption
        }))
    {
        // 加密失败时缓冲仍是明文，返回前清零。
        ciphertext.fill(0);
        return Err(error);
    }
    Ok(SealedSecret { ciphertext, nonce, fingerprint: fingerprint(data_key, purpose, plaintext) })
}

/// 校验认证标签后再以恒定时间比较明文指纹；任一步失败都尽力清零解密缓冲并失败关闭。
pub(crate) fn open_secret(
    data_key: &DataKey,
    purpose: SecretPurpose,
    aad: &[u8],
    ciphertext: &[u8],
    nonce: &[u8],
    expected_fingerprint: &str,
) -> Result<OpenedSecret, OpenError> {
    let kind = purpose.diagnostic_kind();
    let nonce = aead::Nonce::try_assume_unique_for_key(nonce)
        .map_err(safe_diagnostics::error_map(kind, "validate_nonce", || OpenError::InvalidNonce))?;
    let key = data_key.aead_key().map_err(safe_diagnostics::error_map(
        kind,
        "prepare_decryption_key",
        || OpenError::CipherKey,
    ))?;
    // 先交给清零守卫再解密：认证失败、指纹不符或提前返回时整份缓冲（含标签）都会清零。
    let mut plaintext = OpenedSecret(ciphertext.to_vec());
    let length = key
        .open_in_place(nonce, aead::Aad::from(aad), &mut plaintext.0)
        .map_err(safe_diagnostics::error_map(kind, purpose.decrypt_phase(), || {
            OpenError::Authentication
        }))?
        .len();
    // 截短前清理认证标签所在的尾部，移交的缓冲只含明文。
    plaintext.0[length..].fill(0);
    plaintext.0.truncate(length);
    let actual_fingerprint = fingerprint(data_key, purpose, plaintext.as_bytes());
    if !bool::from(actual_fingerprint.as_bytes().ct_eq(expected_fingerprint.as_bytes())) {
        return Err(OpenError::FingerprintMismatch);
    }
    Ok(plaintext)
}

/// HMAC-SHA256(数据密钥材料, 用途域 ‖ 明文) 的小写十六进制；分块更新，不拼接含明文的缓冲。
fn fingerprint(data_key: &DataKey, purpose: SecretPurpose, plaintext: &[u8]) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA256, data_key.material());
    let mut context = hmac::Context::with_key(&key);
    context.update(purpose.fingerprint_domain());
    context.update(plaintext);
    auction_application::core::lower_hex(context.sign().as_ref())
}

#[cfg(test)]
mod tests {
    use super::{NONCE_LENGTH, OpenError, SecretPurpose, open_secret, seal_with_nonce};
    use crate::cryptobox::known_vectors::{
        EHENTAI_COOKIE, EQUIPMENT_KEY, KnownVector, MOOGLEMAIL_COOKIE, USER_LOGIN_KEY,
    };

    const CASES: [(SecretPurpose, &KnownVector); 4] = [
        (SecretPurpose::EquipmentKey, &EQUIPMENT_KEY),
        (SecretPurpose::MoogleMailCookie, &MOOGLEMAIL_COOKIE),
        (SecretPurpose::EhentaiCookie, &EHENTAI_COOKIE),
        (SecretPurpose::UserLoginKey, &USER_LOGIN_KEY),
    ];

    /// 收敛后的原语以冻结向量的 nonce 重新加密，密文与指纹逐字节等于收敛前的实现。
    #[test]
    fn fixed_nonce_reproduces_frozen_vectors_byte_for_byte() {
        for (purpose, vector) in CASES {
            assert_eq!(purpose.fingerprint_domain(), vector.fingerprint_domain);
            let nonce: [u8; NONCE_LENGTH] = vector.nonce().try_into().expect("12 字节 nonce");
            let sealed = seal_with_nonce(
                &vector.data_key(),
                purpose,
                vector.aad.as_bytes(),
                nonce,
                vector.plaintext,
            )
            .expect("加密成功");
            assert_eq!(sealed.ciphertext, vector.ciphertext(), "{} 密文", vector.aad);
            assert_eq!(sealed.nonce.as_slice(), vector.nonce().as_slice());
            assert_eq!(sealed.fingerprint, vector.fingerprint, "{} 指纹", vector.aad);

            let opened = open_secret(
                &vector.data_key(),
                purpose,
                vector.aad.as_bytes(),
                &vector.ciphertext(),
                &vector.nonce(),
                vector.fingerprint,
            )
            .expect("冻结信封可打开");
            assert_eq!(opened.as_bytes(), vector.plaintext);
            assert_eq!(opened.into_bytes(), vector.plaintext);
        }
    }

    /// 篡改密文、换绑 AAD、指纹或用途不符、nonce 长度错误都失败关闭。
    #[test]
    fn tampered_envelopes_fail_closed() {
        let vector = &MOOGLEMAIL_COOKIE;
        let key = vector.data_key();
        let open = |purpose, aad: &[u8], ciphertext: &[u8], nonce: &[u8], fingerprint| {
            open_secret(&key, purpose, aad, ciphertext, nonce, fingerprint).map(|_| ())
        };
        let purpose = SecretPurpose::MoogleMailCookie;
        let aad = vector.aad.as_bytes();
        let mut tampered = vector.ciphertext();
        tampered[0] ^= 1;
        assert_eq!(
            open(purpose, aad, &tampered, &vector.nonce(), vector.fingerprint),
            Err(OpenError::Authentication)
        );
        assert_eq!(
            open(
                purpose,
                b"auction/credential/mail/v1/other",
                &vector.ciphertext(),
                &vector.nonce(),
                vector.fingerprint
            ),
            Err(OpenError::Authentication)
        );
        assert_eq!(
            open(purpose, aad, &vector.ciphertext(), &vector.nonce()[..11], vector.fingerprint),
            Err(OpenError::InvalidNonce)
        );
        assert_eq!(
            open(purpose, aad, &vector.ciphertext(), &vector.nonce(), EHENTAI_COOKIE.fingerprint),
            Err(OpenError::FingerprintMismatch)
        );
        // 同一 AAD 与密文按另一用途打开：认证通过但指纹域不同，必须拒绝。
        assert_eq!(
            open(
                SecretPurpose::EhentaiCookie,
                aad,
                &vector.ciphertext(),
                &vector.nonce(),
                vector.fingerprint
            ),
            Err(OpenError::FingerprintMismatch)
        );
    }
}
