//! 寄售凭证开封与登记重放比较钥的生成、认证和重包。

use std::fmt;
use std::hint::black_box;

use auction_application::consignment::{
    ListingCredentialError, ListingEquipmentKeyOpener, ListingRegistrationSecret,
    RegistrationInput, RegistrationReplayCrypto, RegistrationRequestMac,
    WrappedRegistrationReplayKey,
};
use auction_domain::{DataKeyDomain, EquipmentKey};
use ring::aead;
use ring::hmac;
use ring::rand::{SecureRandom, SystemRandom};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use super::{DataKey, EquipmentKeyOpener};

const REPLAY_KEY_LENGTH: usize = 32;
const WRAP_PURPOSE: &[u8] = b"auction/consignment/registration-replay-key\0";
const WRAP_VERSION: u8 = 1;
const REGISTER_V1: &[u8] = b"auction/consignment/register/v1\0";

impl ListingEquipmentKeyOpener for EquipmentKeyOpener {
    fn open(
        &self,
        secret: &ListingRegistrationSecret,
    ) -> Result<EquipmentKey, ListingCredentialError> {
        self.require_equipment_domain()?;
        // 不变量 6、7：只用登记时固定的内部身份；本边界只返回安全分类。
        self.open(secret.aad_equipment_id(), secret.envelope()).map_err(unavailable)
    }
}

impl RegistrationReplayCrypto for EquipmentKeyOpener {
    fn generate_wrapped(
        &self,
        replay_key_id: Uuid,
    ) -> Result<WrappedRegistrationReplayKey, ListingCredentialError> {
        self.require_equipment_domain()?;
        let mut logical_key = LogicalReplayKey::empty();
        SystemRandom::new().fill(&mut logical_key.0).map_err(unavailable)?;
        self.wrap_replay_key(replay_key_id, &logical_key)
    }

    fn check_wrapped(
        &self,
        replay_key_id: Uuid,
        wrapped: &WrappedRegistrationReplayKey,
    ) -> Result<(), ListingCredentialError> {
        self.open_replay_key(replay_key_id, wrapped).map(drop)
    }

    fn registration_mac(
        &self,
        replay_key_id: Uuid,
        wrapped: &WrappedRegistrationReplayKey,
        input: &RegistrationInput<'_>,
    ) -> Result<RegistrationRequestMac, ListingCredentialError> {
        let logical_key = self.open_replay_key(replay_key_id, wrapped)?;
        logical_key.registration_mac(input)
    }

    fn rewrap(
        &self,
        replay_key_id: Uuid,
        wrapped: &WrappedRegistrationReplayKey,
    ) -> Result<WrappedRegistrationReplayKey, ListingCredentialError> {
        // 不变量 1、7：数据钥轮换仅改包装，旧账本仍使用同一逻辑钥。
        let logical_key = self.open_replay_key(replay_key_id, wrapped)?;
        let replacement = self.wrap_replay_key(replay_key_id, &logical_key)?;
        let verified = self.open_replay_key(replay_key_id, &replacement)?;
        // 两份秘密均为固定长度；比较不能按相同前缀长度提前返回。
        if !bool::from(logical_key.0.ct_eq(&verified.0)) {
            return Err(ListingCredentialError);
        }
        Ok(replacement)
    }
}

impl EquipmentKeyOpener {
    fn require_equipment_domain(&self) -> Result<(), ListingCredentialError> {
        if self.keys.domain() != DataKeyDomain::Equipment {
            return Err(ListingCredentialError);
        }
        Ok(())
    }

    fn wrap_replay_key(
        &self,
        replay_key_id: Uuid,
        logical_key: &LogicalReplayKey,
    ) -> Result<WrappedRegistrationReplayKey, ListingCredentialError> {
        let aad = replay_aad(replay_key_id)?;
        let mut nonce = [0; 12];
        SystemRandom::new().fill(&mut nonce).map_err(unavailable)?;
        let data_key = self.keys.current();
        let ciphertext = seal_replay_key(data_key, &aad, nonce, logical_key)?;
        WrappedRegistrationReplayKey::new(ciphertext, nonce, data_key.id().to_owned())
    }

    fn open_replay_key(
        &self,
        replay_key_id: Uuid,
        wrapped: &WrappedRegistrationReplayKey,
    ) -> Result<LogicalReplayKey, ListingCredentialError> {
        self.require_equipment_domain()?;
        let aad = replay_aad(replay_key_id)?;
        let data_key = self.keys.resolve(wrapped.encryption_key_id()).map_err(unavailable)?;
        open_replay_key(data_key, &aad, wrapped.nonce(), wrapped.wrapped_key())
    }
}

/// 固定长度的逻辑钥；仅在加密适配器中存在。解密原地操作的缓冲也在所有退出路径清零。
struct LogicalReplayKey([u8; REPLAY_KEY_LENGTH]);

impl LogicalReplayKey {
    const fn empty() -> Self {
        Self([0; REPLAY_KEY_LENGTH])
    }

    fn hmac_key(&self) -> hmac::Key {
        hmac::Key::new(hmac::HMAC_SHA256, &self.0)
    }

    fn registration_mac(
        &self,
        input: &RegistrationInput<'_>,
    ) -> Result<RegistrationRequestMac, ListingCredentialError> {
        let key = self.hmac_key();
        let mut mac = hmac::Context::with_key(&key);
        mac.update(REGISTER_V1);
        // 不变量 1、7：冻结 v1 的字段顺序和编码，直接向 HMAC 喂入借用的凭证。
        update_text(&mut mac, input.identity.world_id().as_bytes())?;
        update_text(&mut mac, input.identity.external_equipment_id().as_bytes())?;
        input.key.with_bytes(|bytes| update_text(&mut mac, bytes)).map_err(unavailable)??;
        update_text(&mut mac, input.mode.as_str().as_bytes())?;
        mac.update(&input.price.value().to_be_bytes());
        mac.update(&input.expected_fee_bps.value().to_be_bytes());
        let tag = mac.sign();
        let bytes = tag.as_ref().try_into().map_err(unavailable)?;
        Ok(RegistrationRequestMac::new(bytes))
    }
}

impl fmt::Debug for LogicalReplayKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

fn clear(bytes: &mut [u8]) {
    bytes.fill(0);
    black_box(bytes);
}

impl Drop for LogicalReplayKey {
    fn drop(&mut self) {
        clear(&mut self.0);
    }
}

fn replay_aad(replay_key_id: Uuid) -> Result<Vec<u8>, ListingCredentialError> {
    if replay_key_id.is_nil() {
        return Err(ListingCredentialError);
    }
    let mut aad = Vec::with_capacity(WRAP_PURPOSE.len() + 1 + 16);
    aad.extend_from_slice(WRAP_PURPOSE);
    aad.push(WRAP_VERSION);
    aad.extend_from_slice(replay_key_id.as_bytes());
    Ok(aad)
}

fn seal_replay_key(
    data_key: &DataKey,
    aad: &[u8],
    nonce: [u8; 12],
    logical_key: &LogicalReplayKey,
) -> Result<Vec<u8>, ListingCredentialError> {
    let key = data_key.aead_key().map_err(unavailable)?;
    let mut buffer = logical_key.0;
    let tag = key
        .seal_in_place_separate_tag(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(aad),
            &mut buffer,
        )
        .map_err(unavailable);
    let ciphertext = tag.map(|tag| [buffer.as_slice(), tag.as_ref()].concat());
    clear(&mut buffer);
    ciphertext
}

fn open_replay_key(
    data_key: &DataKey,
    aad: &[u8],
    nonce: &[u8; 12],
    ciphertext: &[u8],
) -> Result<LogicalReplayKey, ListingCredentialError> {
    let key = data_key.aead_key().map_err(unavailable)?;
    let mut buffer: [u8; REPLAY_KEY_LENGTH + 16] = ciphertext.try_into().map_err(unavailable)?;
    let result = key
        .open_in_place(
            aead::Nonce::assume_unique_for_key(*nonce),
            aead::Aad::from(aad),
            &mut buffer,
        )
        .map_err(unavailable)
        .and_then(|plaintext| {
            let plaintext: &[u8; REPLAY_KEY_LENGTH] =
                (&*plaintext).try_into().map_err(unavailable)?;
            let mut logical_key = LogicalReplayKey::empty();
            logical_key.0.copy_from_slice(plaintext);
            Ok(logical_key)
        });
    clear(&mut buffer);
    result
}

fn update_text(mac: &mut hmac::Context, bytes: &[u8]) -> Result<(), ListingCredentialError> {
    let length = u32::try_from(bytes.len()).map_err(unavailable)?;
    mac.update(&length.to_be_bytes());
    mac.update(bytes);
    Ok(())
}

fn unavailable<E>(_error: E) -> ListingCredentialError {
    // 不变量 7：错误链可能随底层实现改变，本边界只保留不可用分类。
    ListingCredentialError
}

#[cfg(test)]
mod tests;
