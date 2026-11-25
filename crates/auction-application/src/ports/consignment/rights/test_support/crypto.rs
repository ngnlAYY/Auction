//! 测试用挑战码密码适配器：HMAC-SHA256 做 MAC，异或流加指纹做封装。

use auction_domain::consignment::rights::{CHALLENGE_CODE_ENTROPY_BYTES, ChallengeCode};
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};

use crate::ports::consignment::{
    ChallengeAad, RightChallengeCrypto, RightCryptoError, SealedChallengeSecret,
};
use crate::ports::core::lower_hex;

type HmacSha256 = Hmac<Sha256>;

/// 测试用挑战码密码适配器。
pub(crate) struct FakeChallengeCrypto {
    key: [u8; 32],
}

impl FakeChallengeCrypto {
    pub(crate) fn new() -> Self {
        Self { key: [0x6b; 32] }
    }

    fn aad_bytes(aad: &ChallengeAad) -> Vec<u8> {
        let mut bytes = aad.challenge_id.as_bytes().to_vec();
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

    fn stream(&self, aad: &ChallengeAad) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(self.key);
        hash.update(Self::aad_bytes(aad));
        hash.update(b"seal");
        hash.finalize().into()
    }

    fn fingerprint(code: &[u8], aad: &ChallengeAad) -> String {
        let mut hash = Sha256::new();
        hash.update(code);
        hash.update(Self::aad_bytes(aad));
        lower_hex(&hash.finalize())
    }
}

impl RightChallengeCrypto for FakeChallengeCrypto {
    fn seal(
        &self,
        code: &ChallengeCode,
        aad: ChallengeAad,
    ) -> Result<SealedChallengeSecret, RightCryptoError> {
        let stream = self.stream(&aad);
        let ciphertext = code.as_bytes().iter().zip(stream).map(|(byte, key)| byte ^ key).collect();
        SealedChallengeSecret::new(
            ciphertext,
            [0; 12],
            "fake-1".to_owned(),
            Self::fingerprint(code.as_bytes(), &aad),
        )
    }

    fn open(
        &self,
        sealed: &SealedChallengeSecret,
        aad: ChallengeAad,
    ) -> Result<ChallengeCode, RightCryptoError> {
        let stream = self.stream(&aad);
        let mut plain = [0_u8; CHALLENGE_CODE_ENTROPY_BYTES];
        if sealed.ciphertext().len() != plain.len() {
            return Err(RightCryptoError);
        }
        for (slot, (byte, key)) in plain.iter_mut().zip(sealed.ciphertext().iter().zip(stream)) {
            *slot = byte ^ key;
        }
        if Self::fingerprint(&plain, &aad) != sealed.fingerprint() {
            return Err(RightCryptoError);
        }
        Ok(ChallengeCode::from_entropy(plain))
    }

    fn mac(&self, aad: ChallengeAad, code: &ChallengeCode) -> Result<[u8; 32], RightCryptoError> {
        let Ok(mut mac) = HmacSha256::new_from_slice(&self.key) else {
            return Err(RightCryptoError);
        };
        mac.update(&Self::aad_bytes(&aad));
        mac.update(code.as_bytes());
        Ok(mac.finalize().into_bytes().into())
    }
}
