//! 秘密信封的冻结已知向量：由信封原语收敛前的三份实现以固定数据密钥（32 字节 0x07）
//! 各生成一份并逐字节冻结。数据库中已存的信封依赖同一格式（AAD、指纹域、12 字节 nonce、
//! AES-256-GCM 追加标签），任何格式漂移都会让这些向量无法打开或指纹不再相等。

use std::sync::Arc;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use ring::{aead, hmac};
use uuid::Uuid;

use crate::DataKey;

/// 一份冻结信封及生成它的全部输入；AAD 与指纹域写成原文，独立于实现固定格式。
pub(crate) struct KnownVector {
    pub(crate) key_id: &'static str,
    /// 适配器绑定的业务 UUID（装备、凭据或用户）。
    pub(crate) subject_id: &'static str,
    pub(crate) aad: &'static str,
    pub(crate) fingerprint_domain: &'static [u8],
    pub(crate) plaintext: &'static [u8],
    pub(crate) ciphertext_hex: &'static str,
    pub(crate) nonce_hex: &'static str,
    pub(crate) fingerprint: &'static str,
}

impl KnownVector {
    pub(crate) fn data_key(&self) -> Arc<DataKey> {
        Arc::new(
            DataKey::from_base64(self.key_id, &BASE64.encode(MATERIAL)).expect("固定测试密钥有效"),
        )
    }

    pub(crate) fn subject(&self) -> Uuid {
        Uuid::parse_str(self.subject_id).expect("向量 UUID 有效")
    }

    pub(crate) fn ciphertext(&self) -> Vec<u8> {
        decode_hex(self.ciphertext_hex)
    }

    pub(crate) fn nonce(&self) -> Vec<u8> {
        decode_hex(self.nonce_hex)
    }

    /// 以向量的 AAD 原文直接用 AES-256-GCM 解密任意信封；用于固定适配器新写入的 AAD。
    pub(crate) fn open_raw(&self, ciphertext: &[u8], nonce: &[u8]) -> Option<Vec<u8>> {
        let key = aead::LessSafeKey::new(
            aead::UnboundKey::new(&aead::AES_256_GCM, &MATERIAL).expect("固定测试密钥有效"),
        );
        let nonce = aead::Nonce::try_assume_unique_for_key(nonce).ok()?;
        let mut buffer = ciphertext.to_vec();
        let plaintext =
            key.open_in_place(nonce, aead::Aad::from(self.aad.as_bytes()), &mut buffer).ok()?;
        Some(plaintext.to_vec())
    }
}

const MATERIAL: [u8; 32] = [7; 32];

pub(crate) const EQUIPMENT_KEY: KnownVector = KnownVector {
    key_id: "equipment-2026-11",
    subject_id: "00000000-0000-4000-8000-0000000000e1",
    aad: "auction/equipment-key/v1/00000000-0000-4000-8000-0000000000e1/key",
    fingerprint_domain: b"auction/cryptobox/secret/v1/equipment-key\0",
    plaintext: b"a1b2c3d4e5",
    ciphertext_hex: "2cdc5a0888c77ce8a387a4d65c12fb643aa1c2168f97d423e20f",
    nonce_hex: "f981ededc7db5570522120b7",
    fingerprint: "433e1198353f34a53dbf9efe61eeac7e17eed24d6e5da58b0a296dd736373823",
};

pub(crate) const MOOGLEMAIL_COOKIE: KnownVector = KnownVector {
    key_id: "credential-2026-11",
    subject_id: "00000000-0000-4000-8000-0000000000c1",
    aad: "auction/credential/mail/v1/mooglemail:main/00000000-0000-4000-8000-0000000000c1",
    fingerprint_domain: b"auction/cryptobox/secret/v1/mooglemail-cookie\0",
    plaintext: b"ipb_member_id=1; ipb_pass_hash=deadbeef",
    ciphertext_hex: "7a8fed8a13d6f1a01de28aae9b60513023320e52e7f1077605f6814c528e493f\
                     33e7d7109d254f74013ea30467d993a7c7b205d043777b",
    nonce_hex: "ac9b36e100dd3b24d59917c2",
    fingerprint: "72345e7f5747923a37a1a7bcca193d76ec2700926871d184b79dcab2df520096",
};

pub(crate) const EHENTAI_COOKIE: KnownVector = KnownVector {
    key_id: "credential-2026-11",
    subject_id: "00000000-0000-4000-8000-0000000000c2",
    aad: "auction/credential/forum/v1/ehentai:shared/00000000-0000-4000-8000-0000000000c2",
    fingerprint_domain: b"auction/cryptobox/secret/v1/ehentai-cookie\0",
    plaintext: b"ipb_member_id=2; ipb_pass_hash=cafebabe",
    ciphertext_hex: "0aa66d4ce00db3d04ca33e0ca0805f134c16435c3107081b08f5c4b9be00d0d6\
                     0019babf35d3678bdab6409bf28b446bf55d7bcd00c162",
    nonce_hex: "4ffcb9b40a555c5d86620da4",
    fingerprint: "3b38436f478f3e4d7f7b2efd01e3b0bd406a9835f67fba8cbf464c2d760aad7f",
};

pub(crate) const USER_LOGIN_KEY: KnownVector = KnownVector {
    key_id: "user_login-2026-11",
    subject_id: "00000000-0000-4000-8000-0000000000a1",
    aad: "auction/user-login-key/v1/00000000-0000-4000-8000-0000000000a1/key",
    fingerprint_domain: b"auction/cryptobox/secret/v1/user-login-key\0",
    plaintext: b"ERERERERERERERERERERERERERERERERERERERERERE",
    ciphertext_hex: "fb3140a6b43a9ae636b005f230c2b04036faba74b75a8a061c4f5db72bfc5d63\
                     ef37a9fe8dade07d416455b1144241212be22be70d37cc2356dcb6",
    nonce_hex: "522857ff3b69e7ee74720c1c",
    fingerprint: "f03532b69a8ee4bf1cb7106c7e5ec49dc5f4f05656fb154e5978ca210493e4aa",
};

pub(crate) const ALL: [&KnownVector; 4] =
    [&EQUIPMENT_KEY, &MOOGLEMAIL_COOKIE, &EHENTAI_COOKIE, &USER_LOGIN_KEY];

fn decode_hex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .chunks(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).expect("十六进制向量是 ASCII");
            u8::from_str_radix(text, 16).expect("十六进制向量有效")
        })
        .collect()
}

/// 不经任何生产实现，按 AAD 原文与指纹域原文重算：冻结的字节确实是约定格式的产物。
#[test]
fn frozen_vectors_match_literal_format() {
    let key = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::AES_256_GCM, &MATERIAL).expect("固定测试密钥有效"),
    );
    for vector in ALL {
        let nonce: [u8; 12] = vector.nonce().try_into().expect("nonce 固定 12 字节");
        let mut sealed = vector.plaintext.to_vec();
        key.seal_in_place_append_tag(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(vector.aad.as_bytes()),
            &mut sealed,
        )
        .expect("加密成功");
        assert_eq!(sealed, vector.ciphertext(), "{} 密文", vector.aad);

        let mac = hmac::Key::new(hmac::HMAC_SHA256, &MATERIAL);
        let mut context = hmac::Context::with_key(&mac);
        context.update(vector.fingerprint_domain);
        context.update(vector.plaintext);
        let fingerprint = auction_application::core::lower_hex(context.sign().as_ref());
        assert_eq!(fingerprint, vector.fingerprint, "{} 指纹", vector.aad);
        assert_eq!(
            vector.open_raw(&vector.ciphertext(), &vector.nonce()).as_deref(),
            Some(vector.plaintext)
        );
    }
}
