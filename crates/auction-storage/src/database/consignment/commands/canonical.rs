//! 寄售命令的版本化二进制输入与系统命令 UUID。

use std::time::Duration;

use auction_application::consignment::{
    AutoCloseCandidate, AutoCloseEvidence, ConsignmentError, ListingDelistReason,
    ListingEquipmentLevel, ListingEquipmentLookupError, ListingLookupSnapshot, ListingOperation,
    LookupFence, LookupObservation, LookupRetryAfter, RegistrationInput,
};
use auction_application::core::RequestHash;
use auction_domain::consignment::{ListingPrice, ListingReason, ListingVersion};
use auction_domain::{CommandId, EquipmentKeyError};
use uuid::Uuid;

const CLOSE_NAMESPACE_V1: Uuid = Uuid::from_u128(0x82191932_162e_4704_93dd_ab97d6d45a4f);
const LOOKUP_NAMESPACE_V1: Uuid = Uuid::from_u128(0x7ebaa2f5_57f6_408d_a1ca_a96c6c0017f3);

/// V1：定宽大端整数，UUID 为 16 字节，可空值先写 0/1，UTF-8 与数组用 u64 长度。
struct Encoding(Vec<u8>);

impl Encoding {
    fn new(operation: ListingOperation, target: Option<Uuid>) -> Self {
        let mut encoded = Self(b"auction.consignment\x00\x01".to_vec());
        encoded.text(operation.as_str());
        encoded.optional(target, Self::uuid);
        encoded
    }

    fn text(&mut self, value: &str) {
        self.bytes(value.as_bytes());
    }

    fn bytes(&mut self, value: &[u8]) {
        self.unsigned(value.len() as u64);
        self.0.extend_from_slice(value);
    }

    fn uuid(&mut self, value: Uuid) {
        self.0.extend_from_slice(value.as_bytes());
    }

    fn optional<T>(&mut self, value: Option<T>, encode: impl FnOnce(&mut Self, T)) {
        self.0.push(u8::from(value.is_some()));
        if let Some(value) = value {
            encode(self, value);
        }
    }

    fn unsigned(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    fn signed(&mut self, value: i64) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    fn number(&mut self, value: f64) {
        // -0 与 +0 是同一个规范数值。
        self.unsigned(if value == 0.0 { 0 } else { value.to_bits() });
    }

    fn duration(&mut self, value: Duration) {
        self.unsigned(value.as_secs());
        self.0.extend_from_slice(&value.subsec_nanos().to_be_bytes());
    }

    fn digest(&self) -> RequestHash {
        RequestHash::from_canonical_bytes(&self.0)
    }
}

impl Drop for Encoding {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

/// 不变量 1、7：只在同步计算 MAC 时借出规范登记输入，登记不用裸 SHA-256。
/// 此编码应由比较钥适配器共用。调用方不得保留或记录字节。
#[allow(dead_code, reason = "由后续任务 2D 的登记 MAC 接线调用")]
pub(crate) fn with_registration_input<T>(
    input: &RegistrationInput<'_>,
    mac: impl FnOnce(&[u8]) -> T,
) -> Result<T, ConsignmentError> {
    let mut encoded = Encoding::new(ListingOperation::Register, None);
    encoded.text(input.identity.world_id());
    encoded.text(input.identity.external_equipment_id());
    // 在追加 key 前预留后续空间，避免含 key 的缓冲在追加字段时重分配。
    encoded.0.reserve(8 + 10 + 8 + "consigned".len() + 8 + 8);
    input
        .key
        .with_bytes(|key| {
            encoded.bytes(key);
            encoded.text(input.mode.as_str());
            encoded.signed(input.price.value());
            encoded.unsigned(u64::from(input.expected_fee_bps.value()));
            mac(&encoded.0)
        })
        .map_err(|EquipmentKeyError::InvalidFormat| ConsignmentError::InvalidEquipmentUrl)
}

/// 改价：目标、预期版本、整数标价。
#[allow(dead_code, reason = "由后续任务 2D 调用")]
pub(crate) fn reprice_hash(
    listing_id: Uuid,
    expected_version: ListingVersion,
    price: ListingPrice,
) -> RequestHash {
    let mut encoded = Encoding::new(ListingOperation::Reprice, Some(listing_id));
    encoded.unsigned(expected_version.value());
    encoded.signed(price.value());
    encoded.digest()
}

/// 取消：目标、预期版本。
#[allow(dead_code, reason = "由后续任务 2D 调用")]
pub(crate) fn withdraw_hash(listing_id: Uuid, expected_version: ListingVersion) -> RequestHash {
    let mut encoded = Encoding::new(ListingOperation::Withdraw, Some(listing_id));
    encoded.unsigned(expected_version.value());
    encoded.digest()
}

/// 管理员下架：目标、预期版本、已校验的私有说明。
#[allow(dead_code, reason = "由后续任务 2D 调用")]
pub(crate) fn admin_delist_hash(
    listing_id: Uuid,
    expected_version: ListingVersion,
    reason: &ListingDelistReason,
) -> RequestHash {
    let mut encoded = Encoding::new(ListingOperation::AdminDelist, Some(listing_id));
    encoded.unsigned(expected_version.value());
    encoded.text(reason.as_str());
    encoded.digest()
}

fn auto_close_input(candidate: AutoCloseCandidate) -> Encoding {
    let mut encoded = Encoding::new(ListingOperation::AutoClose, Some(candidate.listing_id));
    let reason = match candidate.evidence {
        AutoCloseEvidence::AuctionLotCreated(_) => ListingReason::AuctionLotCreated,
        AutoCloseEvidence::SellerIneligible(_) => ListingReason::SellerIneligible,
        AutoCloseEvidence::ServiceDisabled(_) => ListingReason::ServiceDisabled,
        AutoCloseEvidence::LookupDeadline(_) => ListingReason::LookupDeadline,
        AutoCloseEvidence::DisplayExpired(_) => ListingReason::DisplayExpired,
        AutoCloseEvidence::DepositExpired(_) => ListingReason::DepositExpired,
    };
    encoded.text(reason.as_str());
    match candidate.evidence {
        AutoCloseEvidence::AuctionLotCreated(lot_id) => encoded.uuid(lot_id.as_uuid()),
        AutoCloseEvidence::SellerIneligible(fact_id) => encoded.uuid(fact_id),
        AutoCloseEvidence::ServiceDisabled(epoch) => encoded.unsigned(epoch),
        AutoCloseEvidence::LookupDeadline(deadline)
        | AutoCloseEvidence::DisplayExpired(deadline)
        | AutoCloseEvidence::DepositExpired(deadline) => {
            encoded.signed(deadline.timestamp_micros());
        }
    }
    encoded
}

/// 自动清退：同一已证事实在各实例和重试之间使用同一命令 ID 及摘要。
#[allow(dead_code, reason = "由后续任务 2D / 2F 调用")]
pub(crate) fn auto_close_identity(candidate: AutoCloseCandidate) -> (CommandId, RequestHash) {
    let encoded = auto_close_input(candidate);
    (Uuid::new_v5(&CLOSE_NAMESPACE_V1, &encoded.0).into(), encoded.digest())
}

/// 解析：一项工作的一次围栏只可确定一次结果。观察与 token 只参加请求摘要。
#[allow(dead_code, reason = "由后续任务 2F 调用")]
pub(crate) fn lookup_identity(
    fence: LookupFence,
    observation: &LookupObservation,
) -> (CommandId, RequestHash) {
    let mut name = Encoding(b"result\x00\x01".to_vec());
    name.uuid(fence.work_id());
    name.unsigned(fence.generation());
    name.signed(fence.fence());
    let command_id = Uuid::new_v5(&LOOKUP_NAMESPACE_V1, &name.0).into();

    let mut encoded = Encoding::new(ListingOperation::ApplyLookup, Some(fence.listing_id()));
    encoded.uuid(fence.work_id());
    encoded.unsigned(fence.generation());
    encoded.signed(fence.fence());
    encoded.uuid(fence.token());
    match observation {
        LookupObservation::Equipment(snapshot) => {
            encoded.text("equipment");
            encode_snapshot(&mut encoded, snapshot);
        }
        LookupObservation::Failed(error) => {
            encoded.text("failed");
            encode_lookup_failure(&mut encoded, *error);
        }
    }
    (command_id, encoded.digest())
}

fn encode_snapshot(encoded: &mut Encoding, snapshot: &ListingLookupSnapshot) {
    let identity = snapshot.identity();
    let details = snapshot.details();
    let label = details.label();
    encoded.text(identity.world_id());
    encoded.text(identity.external_equipment_id());
    encoded.text(snapshot.owner_name());
    encoded.text(&label.name);
    encoded.optional(label.alt_name.as_deref(), Encoding::text);
    encoded.text(&label.category);
    match label.level {
        ListingEquipmentLevel::Assigned(level) => {
            encoded.text("assigned");
            encoded.unsigned(u64::from(level));
        }
        ListingEquipmentLevel::Unassigned => encoded.text("unassigned"),
        ListingEquipmentLevel::Soulbound => encoded.text("soulbound"),
    }
    encoded.0.push(u8::from(label.tradeable));
    encoded.unsigned(details.stat_groups().len() as u64);
    // 已校验的应用端口在构造详情时已对组和属性排序，并拒绝重名。
    for group in details.stat_groups() {
        encoded.text(&group.name);
        encoded.unsigned(group.stats.len() as u64);
        for stat in &group.stats {
            encoded.text(&stat.name);
            encoded.number(stat.value);
            encoded.optional(stat.base, Encoding::number);
        }
    }
}

fn encode_lookup_failure(encoded: &mut Encoding, error: ListingEquipmentLookupError) {
    use ListingEquipmentLookupError::{
        CredentialUnavailable, Permanent, SourceRateLimited, SourceTimeout, SourceUnavailable,
    };

    match error {
        Permanent(reason) => encoded.text(ListingReason::from(reason).as_str()),
        CredentialUnavailable => encoded.text("credential_unavailable"),
        SourceUnavailable => encoded.text("source_unavailable"),
        SourceTimeout => encoded.text("source_timeout"),
        SourceRateLimited(after) => {
            encoded.text("source_rate_limited");
            encoded.optional(after.map(LookupRetryAfter::duration), Encoding::duration);
        }
    }
}

#[cfg(test)]
mod tests {
    use auction_domain::consignment::{ListingPrice, ListingVersion};
    use uuid::Uuid;

    use super::{Encoding, reprice_hash, withdraw_hash};

    #[test]
    fn consignment_reprice_v1_digest_binds_target_version_price_and_operation() {
        let listing_id = Uuid::from_u128(12);
        let version = ListingVersion::new(3).expect("测试版本");
        let price = ListingPrice::new(255).expect("测试标价");
        let hash = reprice_hash(listing_id, version, price);
        assert_eq!(
            hash.as_bytes(),
            [
                0x2e, 0xb4, 0xaf, 0xc6, 0x6b, 0x52, 0xbc, 0x35, 0xdd, 0xcc, 0x33, 0x89, 0xe1, 0x00,
                0x1f, 0x46, 0x55, 0x14, 0xf6, 0xea, 0x3c, 0x44, 0x69, 0xa0, 0x91, 0x81, 0x23, 0x81,
                0x3f, 0xb3, 0x9e, 0x42,
            ]
        );
        assert_ne!(hash, reprice_hash(Uuid::from_u128(13), version, price));
        assert_ne!(hash, reprice_hash(listing_id, ListingVersion::FIRST, price));
        assert_ne!(
            hash,
            reprice_hash(listing_id, version, ListingPrice::new(256).expect("测试标价"))
        );
        assert_ne!(hash, withdraw_hash(listing_id, version));
    }

    #[test]
    fn consignment_encoding_keeps_order_null_and_utf8_boundaries() {
        let mut first = Encoding(Vec::new());
        first.text("甲");
        first.text("b");
        first.optional(Some(""), Encoding::text);
        first.optional(None, Encoding::text);
        assert_eq!(
            first.0,
            b"\0\0\0\0\0\0\0\x03\xe7\x94\xb2\0\0\0\0\0\0\0\x01b\x01\0\0\0\0\0\0\0\0\0"
        );

        let mut reversed = Encoding(Vec::new());
        reversed.text("b");
        reversed.text("甲");
        reversed.optional(None, Encoding::text);
        reversed.optional(Some(""), Encoding::text);
        assert_ne!(first.digest(), reversed.digest());

        let mut split = Encoding(Vec::new());
        split.text("甲b");
        split.text("");
        split.optional(Some(""), Encoding::text);
        split.optional(None, Encoding::text);
        assert_ne!(first.digest(), split.digest());
    }
}
