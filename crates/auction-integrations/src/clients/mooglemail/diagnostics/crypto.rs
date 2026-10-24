//! 固定用途派生、完整头部 AAD 和带清理守卫的 AES-GCM 边界。

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
    SystemRandom::new().fill(&mut nonce).map_err(|_| MailDiagnosticError::Encryption)?;
    let derived = derive(key, source, b"encrypt");
    let aead = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::AES_256_GCM, &derived.0)
            .map_err(|_| MailDiagnosticError::Encryption)?,
    );
    let sealed_len = plaintext.len() + aead::AES_256_GCM.tag_len();
    let mut output = Vec::with_capacity(NONCE + sealed_len);
    output.extend_from_slice(&nonce);
    let mut plain = SensitiveBytes(Vec::with_capacity(sealed_len));
    plain.0.extend_from_slice(plaintext);
    aead.seal_in_place_append_tag(
        aead::Nonce::assume_unique_for_key(nonce),
        aead::Aad::from(aad),
        &mut plain.0,
    )
    .map_err(|_| MailDiagnosticError::Encryption)?;
    output.extend_from_slice(&plain.0);
    Ok(output)
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
        aead::UnboundKey::new(&aead::AES_256_GCM, &derived.0)
            .map_err(|_| MailDiagnosticError::Authentication)?,
    );
    let mut plain = SensitiveBytes(ciphertext[NONCE..].to_vec());
    let opened = aead
        .open_in_place(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(aad),
            &mut plain.0,
        )
        .map_err(|_| MailDiagnosticError::Authentication)?;
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

pub fn hex(bytes: impl AsRef<[u8]>) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    bytes
        .as_ref()
        .iter()
        .flat_map(|b| [DIGITS[(b >> 4) as usize] as char, DIGITS[(b & 15) as usize] as char])
        .collect()
}
