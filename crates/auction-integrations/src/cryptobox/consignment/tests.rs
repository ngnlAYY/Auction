//! 寄售认证信封的冻结向量、篡改反例和重放比较的覆盖。

use std::error::Error as _;
use std::sync::Arc;

use auction_application::consignment::{
    ListingCredentialError, ListingEquipmentKeyOpener, ListingRegistrationSecret,
    RegistrationInput, RegistrationReplayCrypto, WrappedRegistrationReplayKey,
};
use auction_application::core::lower_hex;
use auction_domain::consignment::{FeeBps, ListingEquipmentIdentity, ListingMode, ListingPrice};
use auction_domain::{DataKeyDomain, EquipmentId, EquipmentKey, EquipmentKeyEnvelope};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use ring::{aead, hmac};
use uuid::Uuid;

use super::{LogicalReplayKey, open_replay_key, replay_aad, seal_replay_key, update_text};
use crate::cryptobox::known_vectors::EQUIPMENT_KEY;
use crate::cryptobox::{DataKey, EquipmentKeyOpener, EquipmentKeySealer};
use crate::keyring::Keyring;

const REPLAY_ID: Uuid = Uuid::from_u128(0x00000000_0000_4000_8000_0000000000e1);
const NONCE: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
const TEST_LOGICAL_KEY: [u8; 32] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
    26, 27, 28, 29, 30, 31,
];

fn ring(domain: DataKeyDomain, key: Arc<DataKey>) -> Arc<Keyring> {
    Arc::new(Keyring::single(domain, key))
}

#[test]
fn registration_crypto_debug_omits_data_key_ids_and_fingerprints() {
    let current = EQUIPMENT_KEY.data_key();
    let previous = Arc::new(
        DataKey::from_base64("fake-debug-old", "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=")
            .expect("假旧数据钥"),
    );
    let sealer = EquipmentKeySealer::new(Arc::clone(&current));
    let keyring =
        Keyring::new(DataKeyDomain::Equipment, Arc::clone(&current), vec![Arc::clone(&previous)])
            .expect("测试装备密钥环");
    let opener = EquipmentKeyOpener::new(Arc::new(keyring));
    for (kind, output) in [
        ("EquipmentKeySealer", format!("{sealer:?}")),
        ("EquipmentKeyOpener", format!("{opener:?}")),
    ] {
        assert!(output.starts_with(kind), "应保留类型名");
        for key in [&current, &previous] {
            let exposed = (output.contains(key.id()), output.contains(key.fingerprint()));
            assert_eq!(exposed, (false, false), "{kind} 输出了数据钥 ID 或指纹");
        }
    }
}

fn registration_secret(
    aad: Uuid,
    ciphertext: Vec<u8>,
    nonce: Vec<u8>,
    key_id: &str,
    fingerprint: &str,
) -> Result<ListingRegistrationSecret, ListingCredentialError> {
    let envelope =
        EquipmentKeyEnvelope::new(ciphertext, nonce, key_id.to_owned(), fingerprint.to_owned())
            .map_err(super::unavailable)?;
    ListingRegistrationSecret::new(EquipmentId::from_uuid(aad), envelope)
}

fn raw_ciphertext(aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
    let vector = &EQUIPMENT_KEY;
    let key = vector.data_key().aead_key().expect("固定假钥有效");
    let mut ciphertext = plaintext.to_vec();
    key.seal_in_place_append_tag(
        aead::Nonce::assume_unique_for_key(NONCE),
        aead::Aad::from(aad),
        &mut ciphertext,
    )
    .expect("为反例构造合法认证标签");
    ciphertext
}

#[test]
fn registration_opener_keeps_the_equipment_envelope_known_vector() {
    let vector = &EQUIPMENT_KEY;
    let keyring = ring(DataKeyDomain::Equipment, vector.data_key());
    let opener = EquipmentKeyOpener::new(Arc::clone(&keyring));
    let secret = registration_secret(
        vector.subject(),
        vector.ciphertext(),
        vector.nonce(),
        vector.key_id,
        vector.fingerprint,
    )
    .expect("沿用冻结装备信封");
    let opened = ListingEquipmentKeyOpener::open(&opener, &secret).expect("开封冻结登记凭证");
    assert!(opened.with_bytes(|value| value == vector.plaintext).expect("读取固定假凭证"));

    let sealed = EquipmentKeySealer::new(Arc::clone(keyring.current()))
        .seal(secret.aad_equipment_id(), &opened)
        .expect("独立登记 UUID 无需装备行");
    assert!(
        vector.open_raw(sealed.ciphertext(), sealed.nonce()).as_deref() == Some(vector.plaintext)
    );
}

#[test]
fn registration_opener_rejects_each_changed_aad_component() {
    let vector = &EQUIPMENT_KEY;
    let opener = EquipmentKeyOpener::new(ring(DataKeyDomain::Equipment, vector.data_key()));
    for (component, offset) in [("用途", 8), ("版本", 23), ("内部 UUID", 25), ("字段", 62)]
    {
        let mut wrong = vector.aad.as_bytes().to_vec();
        wrong[offset] ^= 1;
        let secret = registration_secret(
            vector.subject(),
            raw_ciphertext(&wrong, vector.plaintext),
            NONCE.to_vec(),
            vector.key_id,
            vector.fingerprint,
        )
        .expect("错 AAD 不影响信封形状");
        assert!(
            ListingEquipmentKeyOpener::open(&opener, &secret).is_err(),
            "{component} 必须有认证保护"
        );
    }
    let moved = registration_secret(
        Uuid::from_u128(vector.subject().as_u128() ^ 1),
        vector.ciphertext(),
        vector.nonce(),
        vector.key_id,
        vector.fingerprint,
    )
    .expect("篡改内部身份的信封形状合法");
    assert!(ListingEquipmentKeyOpener::open(&opener, &moved).is_err());
}

#[test]
fn registration_opener_projects_wrong_keyring_invalid_nonce_and_truncated_ciphertext() {
    let vector = &EQUIPMENT_KEY;
    let right = EquipmentKeyOpener::new(ring(DataKeyDomain::Equipment, vector.data_key()));
    let same_id_wrong_material =
        Arc::new(DataKey::from_base64(vector.key_id, &BASE64.encode([9; 32])).expect("假数据钥"));
    let wrong = EquipmentKeyOpener::new(ring(DataKeyDomain::Equipment, same_id_wrong_material));
    let wrong_domain = EquipmentKeyOpener::new(ring(DataKeyDomain::Credential, vector.data_key()));
    let secret = registration_secret(
        vector.subject(),
        vector.ciphertext(),
        vector.nonce(),
        vector.key_id,
        vector.fingerprint,
    )
    .expect("固定信封");
    assert!(ListingEquipmentKeyOpener::open(&right, &secret).is_ok());
    for opener in [&wrong, &wrong_domain] {
        let error = ListingEquipmentKeyOpener::open(opener, &secret).expect_err("错环必须失败");
        assert_eq!(error, ListingCredentialError);
        assert_eq!(error.to_string(), "登记凭证不可用");
        assert!(error.source().is_none());
    }
    let mut short = vector.ciphertext();
    short.pop();
    let truncated = registration_secret(
        vector.subject(),
        short,
        vector.nonce(),
        vector.key_id,
        vector.fingerprint,
    )
    .expect("截短一字节的信封仍满足最低长度");
    assert!(ListingEquipmentKeyOpener::open(&right, &truncated).is_err());
    assert!(
        registration_secret(
            vector.subject(),
            vector.ciphertext(),
            vec![0; 11],
            vector.key_id,
            vector.fingerprint,
        )
        .is_err()
    );
}

#[test]
fn replay_wrapper_has_frozen_aes_gcm_vector_and_binds_purpose_version_and_id() {
    let data_key = EQUIPMENT_KEY.data_key();
    let aad = replay_aad(REPLAY_ID).expect("逻辑钥身份有效");
    let logical = LogicalReplayKey(TEST_LOGICAL_KEY);
    let ciphertext = seal_replay_key(&data_key, &aad, NONCE, &logical).expect("固定 nonce 封装");
    // 使用 OpenSSL EVP 独立生成，包含末尾 16 字节标签。
    assert!(
        lower_hex(&ciphertext)
            == "1880eb73190cdf4e7fbbef97ee5562f5f12da49d0c7761260ed779a192ca731a42a5ff55ca898ac1114f72034bd814f3"
    );
    let wrapped =
        WrappedRegistrationReplayKey::new(ciphertext.clone(), NONCE, data_key.id().to_owned())
            .expect("包装形状合法");
    let opener = EquipmentKeyOpener::new(ring(DataKeyDomain::Equipment, Arc::clone(&data_key)));
    assert!(opener.check_wrapped(REPLAY_ID, &wrapped).is_ok());
    assert!(opener.check_wrapped(Uuid::from_u128(REPLAY_ID.as_u128() ^ 1), &wrapped).is_err());
    assert!(opener.check_wrapped(Uuid::nil(), &wrapped).is_err());

    let fixed_aad =
        b"auction/consignment/registration-replay-key\0\x01\0\0\0\0\0\0\x40\0\x80\0\0\0\0\0\0\xe1";
    for (component, offset) in
        [("用途", 8), ("版本", fixed_aad.len() - 17), ("逻辑 ID", fixed_aad.len() - 1)]
    {
        let mut wrong = fixed_aad.to_vec();
        wrong[offset] ^= 1;
        let forged = raw_ciphertext(&wrong, &TEST_LOGICAL_KEY);
        let forged = WrappedRegistrationReplayKey::new(forged, NONCE, data_key.id().to_owned())
            .expect("形状合法");
        assert!(opener.check_wrapped(REPLAY_ID, &forged).is_err(), "{component} 必须有认证保护");
    }
    assert!(open_replay_key(&data_key, &aad, &NONCE, &ciphertext[..47]).is_err());
    let mut wrong_nonce = NONCE;
    wrong_nonce[0] ^= 1;
    let wrong =
        WrappedRegistrationReplayKey::new(ciphertext, wrong_nonce, data_key.id().to_owned())
            .expect("随机数篡改不改变形状");
    assert!(opener.check_wrapped(REPLAY_ID, &wrong).is_err());
}

#[test]
fn registration_mac_matches_rfc4231_and_the_canonical_input_vector() {
    // RFC 4231 §4.2；尾部零补到固定 32 字节，HMAC 的块填充保持相同。
    let mut standard_key = [0; 32];
    standard_key[..20].fill(0x0b);
    let standard_key = LogicalReplayKey(standard_key);
    let tag = hmac::sign(&standard_key.hmac_key(), b"Hi There");
    assert!(
        lower_hex(tag.as_ref())
            == "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
    );

    let logical = LogicalReplayKey(TEST_LOGICAL_KEY);
    let identity = ListingEquipmentIdentity::new("isekai", "42").expect("规范 EID");
    let key = EquipmentKey::new("ABCDEF0123").expect("假装备 key");
    let value = RegistrationInput {
        identity: &identity,
        key: &key,
        mode: ListingMode::Consigned,
        price: ListingPrice::new(101).expect("标价有效"),
        expected_fee_bps: FeeBps::new(500).expect("费率有效"),
    };
    let tag = logical.registration_mac(&value).expect("计算登记 MAC");
    // 用独立的 Python hmac + struct 生成，固定文本前缀、字段顺序、规范大小写和整数端序。
    assert!(
        lower_hex(tag.as_bytes())
            == "7bd1f112cf0432b394ea3b1bd97a4d6256fc114eb7947707c484e941cab41814"
    );

    let lower = EquipmentKey::new("abcdef0123").expect("同一假钥");
    assert!(tag.matches(
        &logical.registration_mac(&RegistrationInput { key: &lower, ..value }).expect("大小写归一")
    ));
    let different_key = EquipmentKey::new("abcdef0122").expect("另一假钥");
    let different_world = ListingEquipmentIdentity::new("main", "42").expect("另一世界");
    let different_eid = ListingEquipmentIdentity::new("isekai", "43").expect("另一 EID");
    for (field, changed) in [
        ("key", RegistrationInput { key: &different_key, ..value }),
        ("世界", RegistrationInput { identity: &different_world, ..value }),
        ("EID", RegistrationInput { identity: &different_eid, ..value }),
        ("模式", RegistrationInput { mode: ListingMode::Direct, ..value }),
        ("标价", RegistrationInput { price: ListingPrice::new(102).expect("有效"), ..value }),
        ("费率", RegistrationInput { expected_fee_bps: FeeBps::new(499).expect("有效"), ..value }),
    ] {
        assert!(
            !tag.matches(&logical.registration_mac(&changed).expect("规范字段")),
            "{field} 必须参与比较"
        );
    }
    let mut cleared = EquipmentKey::new("abcdef0123").expect("假凭证");
    cleared.clear();
    assert!(logical.registration_mac(&RegistrationInput { key: &cleared, ..value }).is_err());
}

#[test]
fn registration_mac_distinguishes_adjacent_text_boundaries() {
    // 合法的世界、装备 key 与模式限制了输入形状；在文本编码入口单独验证分界。
    let key = LogicalReplayKey(TEST_LOGICAL_KEY).hmac_key();
    let sign = |first: &[u8], second: &[u8]| {
        let mut mac = hmac::Context::with_key(&key);
        update_text(&mut mac, first).expect("编码首段");
        update_text(&mut mac, second).expect("编码次段");
        mac.sign()
    };
    let left = sign(b"1", b"23");
    let right = sign(b"12", b"3");
    assert!(left.as_ref() != right.as_ref(), "相邻文本的长度前缀必须影响 MAC");
}

#[test]
fn replay_generation_uses_fresh_logical_keys_and_keeps_debug_redacted() {
    let data_key = EQUIPMENT_KEY.data_key();
    let opener = EquipmentKeyOpener::new(ring(DataKeyDomain::Equipment, Arc::clone(&data_key)));
    let wrong_domain =
        EquipmentKeyOpener::new(ring(DataKeyDomain::Credential, Arc::clone(&data_key)));
    let unknown = Arc::new(
        DataKey::from_base64("fake-other-data-key", &BASE64.encode([7; 32])).expect("假钥有效"),
    );
    let wrong_ring = EquipmentKeyOpener::new(ring(DataKeyDomain::Equipment, unknown));
    let first = opener.generate_wrapped(REPLAY_ID).expect("生成比较钥");
    let second = opener.generate_wrapped(REPLAY_ID).expect("生成另一逻辑钥");
    let first_clear = opener.open_replay_key(REPLAY_ID, &first).expect("解包装");
    let second_clear = opener.open_replay_key(REPLAY_ID, &second).expect("解包装");
    assert!(first.encryption_key_id() == data_key.id());
    assert!(first.nonce() != second.nonce());
    assert!(first_clear.0 != second_clear.0);
    assert!(wrong_domain.generate_wrapped(REPLAY_ID).is_err());
    assert!(wrong_domain.check_wrapped(REPLAY_ID, &first).is_err());
    assert!(wrong_ring.check_wrapped(REPLAY_ID, &first).is_err());
    assert!(format!("{first_clear:?}") == "[REDACTED]");
    assert!(format!("{first:?}") == "[REDACTED]");
    assert!(!format!("{opener:?}").contains(data_key.id()));
}

#[test]
fn replay_rewrap_preserves_mac_when_only_the_new_data_key_is_available() {
    let previous = EQUIPMENT_KEY.data_key();
    let old = EquipmentKeyOpener::new(ring(DataKeyDomain::Equipment, Arc::clone(&previous)));
    let current = Arc::new(
        DataKey::from_base64("fake-current-data-key", &BASE64.encode([8; 32])).expect("假钥有效"),
    );
    let current_only =
        EquipmentKeyOpener::new(ring(DataKeyDomain::Equipment, Arc::clone(&current)));
    let rotating = EquipmentKeyOpener::new(Arc::new(
        Keyring::new(DataKeyDomain::Equipment, current, vec![previous]).expect("轮换密钥环"),
    ));

    let wrapped = old.generate_wrapped(REPLAY_ID).expect("原包装");
    let rewrapped = rotating.rewrap(REPLAY_ID, &wrapped).expect("重包原逻辑钥");
    assert!(current_only.check_wrapped(REPLAY_ID, &rewrapped).is_ok());
    assert!(current_only.check_wrapped(REPLAY_ID, &wrapped).is_err());
    assert!(old.check_wrapped(REPLAY_ID, &rewrapped).is_err());
    let identity = ListingEquipmentIdentity::new("main", "73").expect("测试装备身份");
    let key = EquipmentKey::new("aabbccddee").expect("假装备 key");
    let input = RegistrationInput {
        identity: &identity,
        key: &key,
        mode: ListingMode::Direct,
        price: ListingPrice::new(51).expect("合法标价"),
        expected_fee_bps: FeeBps::new(0).expect("合法费率"),
    };
    let before = old.registration_mac(REPLAY_ID, &wrapped, &input).expect("原命令 MAC");
    let after = current_only.registration_mac(REPLAY_ID, &rewrapped, &input).expect("重包后 MAC");
    assert!(before.matches(&after));
}
