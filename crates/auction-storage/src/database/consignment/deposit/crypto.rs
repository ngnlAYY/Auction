//! 预约令牌的存储侧生成与摘要。
//!
//! 密码端口已归 `auction-application`；存储只生成 32 字节随机量并计算 SHA-256 摘要，
//! 不读取环境变量，也不把明文码写进日志、错误、回执或普通投递。

use auction_domain::consignment::deposit::{
    ReservationToken, TokenDigest, TokenText, format_token, token_digest,
};
use uuid::Uuid;

/// 存储生成的规范预约令牌及其摘要。
pub(super) struct IssuedReservationToken {
    pub(super) text: TokenText,
    pub(super) digest: TokenDigest,
}

/// 用操作系统 CSPRNG 生成 32 字节随机量。
///
/// `uuid` 的 v4 实现由 `getrandom` 提供系统随机源；两次 UUID 足以得到 32 字节。
pub(super) fn generate_reservation_token() -> IssuedReservationToken {
    let mut entropy = [0_u8; 32];
    entropy[..16].copy_from_slice(Uuid::new_v4().as_bytes());
    entropy[16..].copy_from_slice(Uuid::new_v4().as_bytes());
    let text = format_token(entropy);
    let digest = token_digest(&ReservationToken::from_entropy(entropy));
    IssuedReservationToken { text, digest }
}
