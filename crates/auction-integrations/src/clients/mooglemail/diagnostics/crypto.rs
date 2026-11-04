//! 固定用途派生、完整头部 AAD 和带清理守卫的 AES-GCM 边界。

use auction_application::diagnostics::DiagnosticContext;

use crate::diagnostics as safe_diagnostics;

use super::payload::SensitiveBytes;
use crate::DataKey;
use auction_application::MailDiagnosticError;
use ring::{
    aead, hmac,
    rand::{SecureRandom, SystemRandom},
};

pub const MAGIC: &[u8] = b"auction/mooglemail-diagnostic/v2\0";
const NONCE: usize = 12;

pub fn seal(
    key: &DataKey,
    source: &str,
    aad: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>, MailDiagnosticError> {
    let mut nonce = [0u8; NONCE];
    SystemRandom::new().fill(&mut nonce).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "generate_nonce",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Encryption
    })?;
    let derived = derive(key, source, b"encrypt");
    let aead = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::AES_256_GCM, &derived.0).map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "mooglemail_diagnostics",
                "prepare_encryption_key",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            MailDiagnosticError::Encryption
        })?,
    );
    let sealed_len = plaintext.len() + aead::AES_256_GCM.tag_len();
    // 在最终信封的正文区原地加密，避免另建并复制整份密文；出错仍清理明文缓冲。
    let mut output = SensitiveBytes(Vec::with_capacity(NONCE + sealed_len));
    output.0.extend_from_slice(&nonce);
    output.0.extend_from_slice(plaintext);
    let tag = aead
        .seal_in_place_separate_tag(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(aad),
            &mut output.0[NONCE..],
        )
        .map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "mooglemail_diagnostics",
                "encrypt_payload",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            MailDiagnosticError::Encryption
        })?;
    output.0.extend_from_slice(tag.as_ref());
    Ok(std::mem::take(&mut output.0))
}

pub fn open(
    key: &DataKey,
    source: &str,
    aad: &[u8],
    ciphertext: &[u8],
) -> Result<SensitiveBytes, MailDiagnosticError> {
    if ciphertext.len() < NONCE + 16 {
        return Err(MailDiagnosticError::Authentication);
    }
    let nonce: [u8; NONCE] =
        ciphertext[..NONCE].try_into().map_err(|_| MailDiagnosticError::Authentication)?;
    let derived = derive(key, source, b"encrypt");
    let aead = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::AES_256_GCM, &derived.0).map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "mooglemail_diagnostics",
                "prepare_decryption_key",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            MailDiagnosticError::Authentication
        })?,
    );
    let mut plain = SensitiveBytes(ciphertext[NONCE..].to_vec());
    let opened = aead
        .open_in_place(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(aad),
            &mut plain.0,
        )
        .map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "mooglemail_diagnostics",
                "decrypt_payload",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            MailDiagnosticError::Authentication
        })?;
    let length = opened.len();
    plain.0[length..].fill(0);
    plain.0.truncate(length);
    Ok(plain)
}

fn derive(key: &DataKey, source: &str, label: &[u8]) -> SensitiveBytes {
    let h = hmac::Key::new(hmac::HMAC_SHA256, key.material());
    let mut context = hmac::Context::with_key(&h);
    context.update(MAGIC);
    context.update(b"/source/");
    context.update(source.as_bytes());
    context.update(label);
    let mut out = SensitiveBytes(vec![0u8; 32]);
    out.0.copy_from_slice(context.sign().as_ref());
    out
}

pub fn aad(header: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(MAGIC.len() + header.len());
    output.extend_from_slice(MAGIC);
    output.extend_from_slice(header);
    output
}
