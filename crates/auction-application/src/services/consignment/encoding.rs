//! 无凭证命令的定序摘要和系统命令的稳定身份。

use auction_domain::CommandId;
use auction_domain::consignment::{ListingReason, ListingVersion};
use auction_time::Timestamp;
use uuid::Uuid;

use crate::ports::consignment::{
    AutoCloseCandidate, AutoCloseEvidence, AutoCloseListingCommand, ListingEquipmentLevel,
    ListingEquipmentLookupError, ListingLookupSnapshot, ListingOperation, LookupFence,
    LookupObservation,
};
use crate::ports::core::RequestHash;

/// 不变量 1、7：v1 固定字段次序，整数大端，文本使用 u64 长度前缀，
/// 可空值用单字节 0/1 区分。摘要只编码业务语义字段，审计关联号在编码边界之外。
/// 登记含秘密的 MAC 由比较钥端口负责。
pub(super) struct ListingCommandEncoding(Vec<u8>);

impl ListingCommandEncoding {
    fn new(operation: ListingOperation) -> Self {
        let mut input = Self(b"auction.consignment.command.v1\0".to_vec());
        input.text(operation.as_str());
        input
    }

    pub(super) fn target_version(
        operation: ListingOperation,
        listing_id: Uuid,
        version: ListingVersion,
    ) -> Self {
        let mut input = Self::new(operation);
        input.bytes(listing_id.as_bytes());
        input.bytes(&version.value().to_be_bytes());
        input
    }

    pub(super) fn bytes(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }

    pub(super) fn text(&mut self, value: &str) {
        self.bytes(&(value.len() as u64).to_be_bytes());
        self.bytes(value.as_bytes());
    }

    pub(super) fn request_hash(&self) -> RequestHash {
        RequestHash::from_canonical_bytes(&self.0)
    }

    fn timestamp(&mut self, value: Timestamp) {
        self.bytes(&value.timestamp().to_be_bytes());
        self.bytes(&value.timestamp_subsec_nanos().to_be_bytes());
    }

    fn optional<T>(&mut self, value: Option<T>, encode: impl FnOnce(&mut Self, T)) {
        match value {
            Some(value) => {
                self.bytes(&[1]);
                encode(self, value);
            }
            None => self.bytes(&[0]),
        }
    }

    fn number(&mut self, value: f64) {
        // JSON 数值的正负零是同一个观察，不用它制造命令冲突。
        let bits = if value == 0.0 { 0 } else { value.to_bits() };
        self.bytes(&bits.to_be_bytes());
    }

    fn equipment(&mut self, snapshot: &ListingLookupSnapshot) {
        let identity = snapshot.identity();
        self.text(identity.world_id());
        self.text(identity.external_equipment_id());
        self.text(snapshot.owner_name());

        let details = snapshot.details();
        let label = details.label();
        self.text(&label.name);
        self.optional(label.alt_name.as_deref(), Self::text);
        self.text(&label.category);
        match label.level {
            ListingEquipmentLevel::Assigned(level) => {
                self.text("assigned");
                self.bytes(&level.to_be_bytes());
            }
            ListingEquipmentLevel::Unassigned => self.text("unassigned"),
            ListingEquipmentLevel::Soulbound => self.text("soulbound"),
        }
        self.bytes(&[u8::from(label.tradeable)]);
        let groups = details.stat_groups();
        self.bytes(&(groups.len() as u64).to_be_bytes());
        for group in groups {
            self.text(&group.name);
            self.bytes(&(group.stats.len() as u64).to_be_bytes());
            for stat in &group.stats {
                self.text(&stat.name);
                self.number(stat.value);
                self.optional(stat.base, Self::number);
            }
        }
    }

    fn lookup_error(&mut self, error: ListingEquipmentLookupError) {
        match error {
            ListingEquipmentLookupError::Permanent(reason) => {
                self.text(ListingReason::from(reason).as_str());
            }
            ListingEquipmentLookupError::CredentialUnavailable => {
                self.text(ListingReason::CredentialUnavailable.as_str());
            }
            ListingEquipmentLookupError::SourceUnavailable => self.text("source_unavailable"),
            ListingEquipmentLookupError::SourceTimeout => self.text("source_timeout"),
            ListingEquipmentLookupError::SourceRateLimited(delay) => {
                self.text("source_rate_limited");
                self.optional(delay, |input, delay| {
                    input.bytes(&delay.duration().as_secs().to_be_bytes());
                    input.bytes(&delay.duration().subsec_nanos().to_be_bytes());
                });
            }
        }
    }
}

pub(super) fn lookup_command_identity(
    fence: LookupFence,
    observation: &LookupObservation,
) -> (CommandId, RequestHash) {
    let mut input = ListingCommandEncoding::new(ListingOperation::ApplyLookup);
    input.bytes(&fence.generation().to_be_bytes());
    input.bytes(&fence.fence().to_be_bytes());
    let command_id = Uuid::new_v5(&fence.work_id(), &input.0);
    input.bytes(fence.work_id().as_bytes());
    input.bytes(fence.listing_id().as_bytes());
    input.bytes(fence.token().as_bytes());
    match observation {
        LookupObservation::Equipment(snapshot) => {
            input.bytes(&[0]);
            input.equipment(snapshot);
        }
        LookupObservation::Failed(error) => {
            input.bytes(&[1]);
            input.lookup_error(*error);
        }
    }
    (CommandId::from_uuid(command_id), input.request_hash())
}

pub(super) fn auto_close_command(candidate: AutoCloseCandidate) -> AutoCloseListingCommand {
    let mut input = ListingCommandEncoding::new(ListingOperation::AutoClose);
    input.bytes(candidate.listing_id.as_bytes());
    let reason = match candidate.evidence {
        AutoCloseEvidence::AuctionLotCreated(lot_id) => {
            input.bytes(lot_id.as_uuid().as_bytes());
            ListingReason::AuctionLotCreated
        }
        AutoCloseEvidence::SellerIneligible(fact_id) => {
            input.bytes(fact_id.as_bytes());
            ListingReason::SellerIneligible
        }
        AutoCloseEvidence::ServiceDisabled(epoch) => {
            input.bytes(&epoch.to_be_bytes());
            ListingReason::ServiceDisabled
        }
        AutoCloseEvidence::LookupDeadline(deadline) => {
            input.timestamp(deadline);
            ListingReason::LookupDeadline
        }
        AutoCloseEvidence::DisplayExpired(deadline) => {
            input.timestamp(deadline);
            ListingReason::DisplayExpired
        }
        AutoCloseEvidence::DepositExpired(deadline) => {
            input.timestamp(deadline);
            ListingReason::DepositExpired
        }
    };
    input.text(reason.as_str());
    let command_id = Uuid::new_v5(&candidate.listing_id, &input.0);
    AutoCloseListingCommand {
        request_hash: input.request_hash(),
        command_id: CommandId::from_uuid(command_id),
        candidate,
    }
}
