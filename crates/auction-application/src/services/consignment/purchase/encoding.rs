//! 购买与 v2 改价的规范编码与摘要（v2 统一前缀）。
//!
//! 流为 `prefix || text(operation) || text(actor_kind) || actor_ref(用户 UUID 原 16 字节) ||
//! target(挂单 UUID 原 16 字节) || inputs`；非负整数与长度 u64 大端，文本是 u64 字节长度加
//! UTF-8。`request_id` 与处理时刻不入摘要；`command_id` 是账本键，也不入摘要。
//! 黄金向量固定在本 crate 的测试里，字节不得再改。

use auction_domain::UserId;
use auction_domain::consignment::{ListingPrice, ListingVersion};
use uuid::Uuid;

use crate::ports::consignment::DepositActor;
use crate::ports::core::RequestHash;
use crate::services::consignment::deposit::encoding::COMMAND_PREFIX_V2;

/// 购买的操作码。
pub const PURCHASE_OPERATION: &str = "purchase";
/// v2 改价的操作码。
pub const REPRICE_V2_OPERATION: &str = "reprice";

/// 购买的规范字节：操作、买家、挂单、预期版本、预期价格。
pub fn canonical_purchase(
    buyer: UserId,
    listing_id: Uuid,
    expected_version: ListingVersion,
    expected_price: ListingPrice,
) -> Vec<u8> {
    canonical(PURCHASE_OPERATION, buyer, listing_id, expected_version, expected_price)
}

/// v2 改价的规范字节：操作、卖家、挂单、预期版本、新价格。
pub fn canonical_reprice_v2(
    seller: UserId,
    listing_id: Uuid,
    expected_version: ListingVersion,
    price: ListingPrice,
) -> Vec<u8> {
    canonical(REPRICE_V2_OPERATION, seller, listing_id, expected_version, price)
}

/// 规范字节的 SHA-256 摘要。
pub fn request_hash(canonical_bytes: &[u8]) -> RequestHash {
    RequestHash::from_canonical_bytes(canonical_bytes)
}

fn canonical(
    operation: &str,
    actor: UserId,
    listing_id: Uuid,
    expected_version: ListingVersion,
    price: ListingPrice,
) -> Vec<u8> {
    let mut bytes = COMMAND_PREFIX_V2.to_vec();
    write_text(&mut bytes, operation);
    write_text(&mut bytes, DepositActor::PublicUser(actor).kind());
    bytes.extend_from_slice(actor.as_uuid().as_bytes());
    bytes.extend_from_slice(listing_id.as_bytes());
    bytes.extend_from_slice(&expected_version.value().to_be_bytes());
    // 价格已由 `ListingPrice` 限定在 1..=10^9，按 u64 大端编码。
    bytes.extend_from_slice(&price.value().unsigned_abs().to_be_bytes());
    bytes
}

fn write_text(output: &mut Vec<u8>, value: &str) {
    output.extend_from_slice(&(value.len() as u64).to_be_bytes());
    output.extend_from_slice(value.as_bytes());
}
