//! 寄售第 4 期 v2 规范编码、摘要与系统命令 UUIDv5。
//!
//! 流为 `prefix || text(operation) || text(actor_kind) || actor_ref || target || inputs`。
//! UUID 用原 16 字节；非负整数与长度 u64 大端；文本是 u64 字节长度加 UTF-8；可空值单字节
//! 0/1；时间沿原秒 i64 大端加纳秒 u32 大端。`request_id`、处理时刻与租约 owner 名不入摘要。

use auction_domain::CommandId;
use auction_domain::consignment::deposit::{
    EnvelopeDecision, EnvelopeItemObservation, ReservationBinding,
};
use auction_domain::consignment::{CalendarVersion, OriginalMailKey};
use auction_time::Timestamp;
use uuid::Uuid;

use crate::ports::consignment::{
    AppendQuarantineObservationCommand, ApplyMailProofCommand, DecideEnvelopeCommand, DepositActor,
    DepositOperation, ExpireReservationCommand, IssueReservationCommand, RecheckQuarantineCommand,
    ReleaseOperationsHoldCommand, ReleaseQuarantineCommand, ReservationInvalidation,
    SeasonCloseCommand, SeasonCloseTarget, SeasonNoticeCommand, SetOperationsHoldCommand,
    StartDispositionWriteCommand, WithdrawDepositCommand,
};
use crate::ports::core::RequestHash;

/// v2 统一前缀，末尾一个 NUL 字节。
pub const COMMAND_PREFIX_V2: &[u8] = b"auction.consignment.command.v2\0";

/// 规范编码目标；目标字段不再在 operation inputs 中重复。
#[derive(Debug, Clone, Copy)]
pub enum CommandTarget<'a> {
    /// 挂单 UUID。
    Listing(Uuid),
    /// 隔离 UUID。
    Quarantine(Uuid),
    /// 预约 UUID。
    Reservation(Uuid),
    /// 最终决定 UUID。
    FinalDecision(Uuid),
    /// 原 attempt UUID。
    OriginalAttempt(Uuid),
    /// 稳定原信键。
    MailKey(&'a OriginalMailKey),
    /// 冻结季窗头。
    Season {
        /// 季身份。
        season_id: &'a str,
        /// 赛季开始。
        start_utc: Timestamp,
        /// 赛季结束。
        end_utc: Timestamp,
        /// 预停截止。
        admission_cutoff_utc: Timestamp,
        /// 日历版本。
        calendar_version: CalendarVersion,
        /// 提前本地日历日数。
        preclose_days: u8,
    },
}

/// 规范字节构建器；只由应用层使用。
#[derive(Debug, Clone)]
pub struct CommandEncoding {
    bytes: Vec<u8>,
}

impl CommandEncoding {
    /// 写入前缀、操作、actor、actor 稳定引用与目标。
    pub fn new(
        operation: DepositOperation,
        actor: &DepositActor,
        target: CommandTarget<'_>,
    ) -> Self {
        let mut encoding = Self { bytes: COMMAND_PREFIX_V2.to_vec() };
        encoding.text(operation.as_str());
        encoding.text(actor.kind());
        match actor {
            DepositActor::PublicUser(user_id) => encoding.bytes(user_id.as_uuid().as_bytes()),
            DepositActor::Admin(actor_ref) => encoding.text(actor_ref),
            DepositActor::Worker | DepositActor::System => {
                encoding.text(DepositActor::DEPOSIT_SERVICE_REF);
            }
        }
        match target {
            CommandTarget::Listing(id)
            | CommandTarget::Quarantine(id)
            | CommandTarget::Reservation(id)
            | CommandTarget::FinalDecision(id)
            | CommandTarget::OriginalAttempt(id) => encoding.uuid(id),
            CommandTarget::MailKey(key) => encoding.mail_key(key),
            CommandTarget::Season {
                season_id,
                start_utc,
                end_utc,
                admission_cutoff_utc,
                calendar_version,
                preclose_days,
            } => {
                encoding.text(season_id);
                encoding.timestamp(start_utc);
                encoding.timestamp(end_utc);
                encoding.timestamp(admission_cutoff_utc);
                encoding.bytes(&calendar_version.as_bytes());
                encoding.u64(u64::from(preclose_days));
            }
        }
        encoding
    }

    /// 追加原始字节。
    pub fn bytes(&mut self, value: &[u8]) {
        self.bytes.extend_from_slice(value);
    }

    /// 追加 u64 大端。
    pub fn u64(&mut self, value: u64) {
        self.bytes(&value.to_be_bytes());
    }

    /// 追加 UUID 原 16 字节。
    pub fn uuid(&mut self, value: Uuid) {
        self.bytes(value.as_bytes());
    }

    /// 追加长度前缀 UTF-8。
    pub fn text(&mut self, value: &str) {
        self.u64(value.len() as u64);
        self.bytes(value.as_bytes());
    }

    /// 追加稳定原信键。
    pub fn mail_key(&mut self, key: &OriginalMailKey) {
        self.text(key.authenticated_bot_uid());
        self.text(auction_domain::consignment::settlement_world_code(key.world()));
        self.text(key.external_mid());
    }

    /// 追加可空值：1 + 值或 0。
    pub fn optional<T>(&mut self, value: Option<T>, encode: impl FnOnce(&mut Self, T)) {
        match value {
            Some(value) => {
                self.bytes(&[1]);
                encode(self, value);
            }
            None => self.bytes(&[0]),
        }
    }

    /// 追加时间：i64 秒大端 + u32 纳秒大端。
    pub fn timestamp(&mut self, value: Timestamp) {
        self.bytes(&value.timestamp().to_be_bytes());
        self.bytes(&value.timestamp_subsec_nanos().to_be_bytes());
    }

    /// 返回当前规范字节。
    #[cfg(test)]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// 计算 SHA-256 摘要。
    pub fn request_hash(&self) -> RequestHash {
        RequestHash::from_canonical_bytes(&self.bytes)
    }
}

/// 系统命令身份：`UUIDv5(NAMESPACE_URL, prefix || text(operation) || business_key)`。
pub fn system_command_id(operation: DepositOperation, business_key: &[u8]) -> CommandId {
    let mut input = COMMAND_PREFIX_V2.to_vec();
    write_text(&mut input, operation.as_str());
    input.extend_from_slice(business_key);
    CommandId::from_uuid(Uuid::new_v5(&Uuid::NAMESPACE_URL, &input))
}

/// 原信键业务键，与 `text(bot_uid),text(world),text(mid)` 相同。
pub fn mail_key_business_key(key: &OriginalMailKey) -> Vec<u8> {
    let mut output = Vec::new();
    write_text(&mut output, key.authenticated_bot_uid());
    write_text(&mut output, auction_domain::consignment::settlement_world_code(key.world()));
    write_text(&mut output, key.external_mid());
    output
}

pub(super) fn expire_business_key(
    reservation_id: Uuid,
    generation: u64,
    invalidation: ReservationInvalidation,
) -> Vec<u8> {
    let mut output = Vec::new();
    output.extend_from_slice(reservation_id.as_bytes());
    output.extend_from_slice(&generation.to_be_bytes());
    write_text(&mut output, invalidation.kind());
    write_invalidation_fact(&mut output, invalidation);
    output
}

pub(super) fn observation_business_key(
    quarantine_id: Uuid,
    remote_observation_id: &str,
) -> Vec<u8> {
    let mut output = quarantine_id.as_bytes().to_vec();
    write_text(&mut output, remote_observation_id);
    output
}

pub(super) fn start_write_business_key(
    decision_id: Uuid,
    action: crate::ports::consignment::DispositionAction,
    delivery_id: Uuid,
    fence: u64,
) -> Vec<u8> {
    let mut output = decision_id.as_bytes().to_vec();
    write_text(&mut output, action.as_str());
    output.extend_from_slice(delivery_id.as_bytes());
    output.extend_from_slice(&fence.to_be_bytes());
    output
}

pub(super) fn proof_business_key(
    attempt_id: Uuid,
    proof_kind: crate::ports::consignment::MailProofKind,
) -> Vec<u8> {
    let mut output = attempt_id.as_bytes().to_vec();
    write_text(&mut output, proof_kind.as_str());
    output
}

pub(super) fn season_notice_business_key(
    season_id: &str,
    position_id: Uuid,
    seller_user_id: auction_domain::UserId,
) -> Vec<u8> {
    let mut output = Vec::new();
    write_text(&mut output, season_id);
    output.extend_from_slice(position_id.as_bytes());
    output.extend_from_slice(seller_user_id.as_uuid().as_bytes());
    output
}

pub(super) fn season_close_business_key(season_id: &str, target: SeasonCloseTarget) -> Vec<u8> {
    let mut output = Vec::new();
    write_text(&mut output, season_id);
    write_season_close_target(&mut output, target);
    output
}

pub(super) fn canonical_issue_reservation(command: &IssueReservationCommand) -> CommandEncoding {
    let mut encoding = CommandEncoding::new(
        DepositOperation::IssueReservation,
        &DepositActor::PublicUser(command.seller_user_id),
        CommandTarget::Listing(command.listing_id),
    );
    encoding.u64(command.expected_version.value());
    encoding
}

pub(super) fn canonical_expire_reservation(command: &ExpireReservationCommand) -> CommandEncoding {
    let mut encoding = CommandEncoding::new(
        DepositOperation::ExpireReservation,
        &DepositActor::Worker,
        CommandTarget::Reservation(command.reservation_id),
    );
    encoding.u64(command.generation);
    encoding.text(command.invalidation.kind());
    match command.invalidation {
        ReservationInvalidation::Expired { expires_at } => encoding.timestamp(expires_at),
        ReservationInvalidation::Cancelled { command_id } => encoding.uuid(command_id.as_uuid()),
        ReservationInvalidation::IdentityLost { fact_id } => encoding.uuid(fact_id),
        ReservationInvalidation::ServiceDisabled { service_epoch } => encoding.u64(service_epoch),
    }
    encoding
}

pub(super) fn canonical_decide_envelope(command: &DecideEnvelopeCommand) -> CommandEncoding {
    let observation = &command.facts.observation;
    let mut encoding = CommandEncoding::new(
        DepositOperation::DecideEnvelope,
        &DepositActor::Worker,
        CommandTarget::MailKey(&command.mail_key),
    );
    encoding.uuid(observation.observation_id);
    encoding.bytes(&observation.session_binding_digest);
    encoding.bytes(&observation.manifest_digest);
    encoding.u64(observation.items.len() as u64);
    for item in &observation.items {
        encode_item(&mut encoding, item);
    }
    let mut reservations = command.facts.reservations.clone();
    reservations.sort_by(|left, right| {
        left.equipment
            .world_id()
            .cmp(right.equipment.world_id())
            .then_with(|| {
                left.equipment.external_equipment_id().cmp(right.equipment.external_equipment_id())
            })
            .then_with(|| left.reservation_id.cmp(&right.reservation_id))
    });
    encoding.u64(reservations.len() as u64);
    for reservation in &reservations {
        encode_reservation(&mut encoding, reservation, observation);
    }
    encoding.text(command.decision.decision_kind());
    match &command.decision {
        EnvelopeDecision::Quarantine(reason) => encoding.text(reason.as_str()),
        EnvelopeDecision::Return(manifest) => {
            let mut items = manifest.items.iter().collect::<Vec<_>>();
            items.sort_by_key(|item| item.ordinal);
            encoding.u64(items.len() as u64);
            for item in items {
                encoding.u64(u64::from(item.ordinal));
                encoding.text(item.reason.as_str());
            }
        }
        EnvelopeDecision::Accept(_) | EnvelopeDecision::ArchiveEmpty => encoding.u64(0),
    }
    encoding
}

pub(super) fn canonical_append_observation(
    command: &AppendQuarantineObservationCommand,
) -> CommandEncoding {
    let mut encoding = CommandEncoding::new(
        DepositOperation::AppendQuarantineObservation,
        &DepositActor::Worker,
        CommandTarget::Quarantine(command.quarantine_id),
    );
    encoding.text(&command.remote_observation_id);
    encoding.timestamp(command.observed_at);
    encoding.bytes(&command.observation.session_binding_digest);
    encoding.text(&command.observation.parser_version);
    encoding.bytes(&command.observation.manifest_digest);
    encoding.optional(command.evidence_ref.as_deref(), CommandEncoding::text);
    encoding
}

pub(super) fn canonical_recheck_quarantine(command: &RecheckQuarantineCommand) -> CommandEncoding {
    let mut encoding = CommandEncoding::new(
        DepositOperation::RecheckQuarantine,
        &DepositActor::Admin(command.admin_actor.clone()),
        CommandTarget::Quarantine(command.quarantine_id),
    );
    encoding.u64(command.expected_version);
    encoding
}

/// 首次隔离自动排队的系统重检业务键。
pub fn system_recheck_business_key(quarantine_id: Uuid) -> Vec<u8> {
    quarantine_id.as_bytes().to_vec()
}

/// 系统重检的规范摘要；命令 ID 另由 [`system_command_id`] 从同一业务键派生。
pub fn canonical_system_recheck_request_hash(
    quarantine_id: Uuid,
    expected_version: u64,
) -> RequestHash {
    let mut encoding = CommandEncoding::new(
        DepositOperation::RecheckQuarantine,
        &DepositActor::System,
        CommandTarget::Quarantine(quarantine_id),
    );
    encoding.u64(expected_version);
    encoding.request_hash()
}

pub(super) fn canonical_release_quarantine(command: &ReleaseQuarantineCommand) -> CommandEncoding {
    let mut encoding = CommandEncoding::new(
        DepositOperation::ReleaseQuarantine,
        &command.actor,
        CommandTarget::Quarantine(command.quarantine_id),
    );
    encoding.u64(command.expected_version);
    encoding.uuid(command.observation_id);
    encoding.text(command.proposed_kind.as_str());
    if let Some(binding) = &command.auto_binding {
        encoding.bytes(&binding.observation_digest);
        encoding.text(&binding.source_ref);
    }
    encoding
}

pub(super) fn canonical_start_disposition_write(
    command: &StartDispositionWriteCommand,
) -> CommandEncoding {
    let mut encoding = CommandEncoding::new(
        DepositOperation::StartDispositionWrite,
        &DepositActor::Worker,
        CommandTarget::FinalDecision(command.final_decision_id),
    );
    encoding.text(command.action.as_str());
    encoding.uuid(command.delivery_id);
    encoding.u64(command.fence);
    encoding.uuid(command.work_id);
    encoding.uuid(command.attempt_id);
    encoding.uuid(command.claim_token);
    encoding.bytes(&command.session_binding_digest);
    encoding.bytes(&command.expected_manifest_digest);
    encoding
}

pub(super) fn canonical_apply_mail_proof(command: &ApplyMailProofCommand) -> CommandEncoding {
    let mut encoding = CommandEncoding::new(
        DepositOperation::ApplyMailProof,
        &DepositActor::Worker,
        CommandTarget::OriginalAttempt(command.original_attempt_id),
    );
    encoding.text(command.proof_kind.as_str());
    encoding.uuid(command.final_decision_id);
    encoding.text(command.action.as_str());
    encoding.uuid(command.delivery_id);
    encoding.u64(command.fence);
    encoding.uuid(command.observation_id);
    encoding.text(&command.validator_version);
    encoding.bytes(&command.proof_digest);
    encoding
}

pub(super) fn canonical_withdraw(command: &WithdrawDepositCommand) -> CommandEncoding {
    let mut encoding = CommandEncoding::new(
        DepositOperation::Withdraw,
        &DepositActor::PublicUser(command.seller_user_id),
        CommandTarget::Listing(command.listing_id),
    );
    encoding.u64(command.expected_version.value());
    encoding
}

pub(super) fn canonical_set_hold(command: &SetOperationsHoldCommand) -> CommandEncoding {
    let mut encoding = CommandEncoding::new(
        DepositOperation::AdminDelist,
        &DepositActor::Admin(command.admin_actor.clone()),
        CommandTarget::Listing(command.listing_id),
    );
    encoding.u64(command.expected_version.value());
    encoding.text(command.reason.as_str());
    encoding
}

pub(super) fn canonical_release_hold(command: &ReleaseOperationsHoldCommand) -> CommandEncoding {
    let mut encoding = CommandEncoding::new(
        DepositOperation::ReleaseOperationsHold,
        &DepositActor::Admin(command.admin_actor.clone()),
        CommandTarget::Listing(command.listing_id),
    );
    encoding.u64(command.expected_version.value());
    encoding.uuid(command.hold_id);
    encoding
}

pub(super) fn canonical_season_notice(command: &SeasonNoticeCommand) -> CommandEncoding {
    let mut encoding = CommandEncoding::new(
        DepositOperation::SeasonNotice,
        &DepositActor::Worker,
        season_target(&command.season),
    );
    encoding.uuid(command.position_id);
    encoding.uuid(command.seller_user_id.as_uuid());
    encoding.text(command.notice_kind.as_str());
    encoding
}

pub(super) fn canonical_season_close(command: &SeasonCloseCommand) -> CommandEncoding {
    let mut encoding = CommandEncoding::new(
        DepositOperation::SeasonClose,
        &DepositActor::Worker,
        season_target(&command.season),
    );
    encode_season_close_target(&mut encoding, command.target);
    encoding
}

fn season_target<'a>(season: &'a auction_domain::consignment::FrozenSeason) -> CommandTarget<'a> {
    CommandTarget::Season {
        season_id: &season.season_id,
        start_utc: season.start_utc,
        end_utc: season.end_utc,
        admission_cutoff_utc: season.admission_cutoff_utc,
        calendar_version: season.calendar_version,
        preclose_days: season.preclose_days,
    }
}

fn encode_item(encoding: &mut CommandEncoding, item: &EnvelopeItemObservation) {
    encoding.u64(u64::from(item.ordinal));
    encoding.text(item.kind.as_str());
    encoding.optional(item.raw_eid.as_deref(), CommandEncoding::text);
    encoding.optional(item.canonical_eid.as_deref(), CommandEncoding::text);
    encoding.optional(item.amount, CommandEncoding::u64);
    encoding.optional(item.protected_ref, CommandEncoding::uuid);
}

fn encode_reservation(
    encoding: &mut CommandEncoding,
    reservation: &ReservationBinding,
    observation: &auction_domain::consignment::deposit::EnvelopeObservation,
) {
    encoding.text(reservation.equipment.world_id());
    encoding.text(reservation.equipment.external_equipment_id());
    let ordinal = observation
        .items
        .iter()
        .find(|item| {
            item.canonical_eid.as_deref() == Some(reservation.equipment.external_equipment_id())
                && item.world.is_some_and(|world| {
                    auction_domain::consignment::settlement_world_code(world)
                        == reservation.equipment.world_id()
                })
        })
        .map_or(0, |item| u64::from(item.ordinal));
    encoding.u64(ordinal);
    encoding.uuid(reservation.reservation_id);
    encoding.u64(reservation.generation);
    encoding.uuid(reservation.listing_id);
    encoding.u64(reservation.listing_version.value());
}

fn encode_season_close_target(encoding: &mut CommandEncoding, target: SeasonCloseTarget) {
    encoding.text(target.kind());
    match target {
        SeasonCloseTarget::World(world) => {
            encoding.text(auction_domain::consignment::settlement_world_code(world));
        }
        SeasonCloseTarget::Listing(id) | SeasonCloseTarget::Position(id) => encoding.uuid(id),
    }
}

fn write_invalidation_fact(output: &mut Vec<u8>, invalidation: ReservationInvalidation) {
    match invalidation {
        ReservationInvalidation::Expired { expires_at } => {
            output.extend_from_slice(&expires_at.timestamp().to_be_bytes());
            output.extend_from_slice(&expires_at.timestamp_subsec_nanos().to_be_bytes());
        }
        ReservationInvalidation::Cancelled { command_id } => {
            output.extend_from_slice(command_id.as_uuid().as_bytes());
        }
        ReservationInvalidation::IdentityLost { fact_id } => {
            output.extend_from_slice(fact_id.as_bytes());
        }
        ReservationInvalidation::ServiceDisabled { service_epoch } => {
            output.extend_from_slice(&service_epoch.to_be_bytes());
        }
    }
}

fn write_text(output: &mut Vec<u8>, value: &str) {
    output.extend_from_slice(&(value.len() as u64).to_be_bytes());
    output.extend_from_slice(value.as_bytes());
}

fn write_season_close_target(output: &mut Vec<u8>, target: SeasonCloseTarget) {
    write_text(output, target.kind());
    match target {
        SeasonCloseTarget::World(world) => {
            write_text(output, auction_domain::consignment::settlement_world_code(world));
        }
        SeasonCloseTarget::Listing(id) | SeasonCloseTarget::Position(id) => {
            output.extend_from_slice(id.as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use auction_domain::CommandId;
    use auction_domain::SettlementWorld;
    use auction_domain::UserId;
    use auction_domain::consignment::deposit::{
        EnvelopeCapabilities, EnvelopeDecision, EnvelopeItemKind, EnvelopeItemObservation,
        EnvelopeObservation, LockedEnvelopeFacts, MatchedDepositItem, QuarantineReason,
        ReservationBinding, ReservationState, ReturnItem, ReturnItemReason, ReturnManifest,
        SenderIdentity,
    };
    use auction_domain::consignment::{
        CalendarVersion, FrozenSeason, ListingEquipmentIdentity, ListingVersion, OriginalMailKey,
    };
    use auction_time::{Duration, Timestamp};
    use uuid::Uuid;

    use super::{
        CommandEncoding, canonical_append_observation, canonical_apply_mail_proof,
        canonical_decide_envelope, canonical_expire_reservation, canonical_issue_reservation,
        canonical_recheck_quarantine, canonical_release_hold, canonical_release_quarantine,
        canonical_season_close, canonical_season_notice, canonical_set_hold,
        canonical_start_disposition_write, canonical_withdraw, expire_business_key,
        mail_key_business_key, season_notice_business_key, start_write_business_key,
        system_command_id,
    };
    use crate::ports::consignment::{
        AppendQuarantineObservationCommand, ApplyMailProofCommand, AutoReleaseBinding,
        DecideEnvelopeCommand, DepositActor, DispositionAction, ExpireReservationCommand,
        IssueReservationCommand, ListingDelistReason, MailProofKind, RecheckQuarantineCommand,
        ReleaseOperationsHoldCommand, ReleaseQuarantineCommand, ReservationInvalidation,
        SeasonCloseCommand, SeasonCloseTarget, SeasonNoticeCommand, SeasonNoticeKind,
        SetOperationsHoldCommand, StartDispositionWriteCommand, WithdrawDepositCommand,
    };
    use crate::ports::core::RequestHash;

    const ISSUE_HEX: &str = "61756374696f6e2e636f6e7369676e6d656e742e636f6d6d616e642e763200000000000000001169737375655f7265736572766174696f6e000000000000000b7075626c69635f7573657200000000000000000000000000000007000000000000000000000000000000010000000000000003";
    const ISSUE_SHA: &str = "64477d1fcff0e6353d49f0ea276f367c3189335a638d489bc466b1c5e40d46e6";
    const EXPIRE_HEX: &str = "61756374696f6e2e636f6e7369676e6d656e742e636f6d6d616e642e76320000000000000000126578706972655f7265736572766174696f6e0000000000000006776f726b6572000000000000000b636f6e7369676e6d656e7400000000000000000000000000000002000000000000000400000000000000076578706972656400000000695735a500000000";
    const EXPIRE_SHA: &str = "a704d74b13f205ae27da4855ec750eddf56f149a3d1b9539650e6238bfdfb552";
    // 2026-11-21 统筹方裁定冻结：decision_kind 与 decision_evidence 分开编码。
    const DECIDE_HEX: &str = "61756374696f6e2e636f6e7369676e6d656e742e636f6d6d616e642e763200000000000000000f6465636964655f656e76656c6f70650000000000000006776f726b6572000000000000000b636f6e7369676e6d656e740000000000000002343200000000000000046d61696e0000000000000003313030000000000000000000000000000000031111111111111111111111111111111111111111111111111111111111111111222222222222222222222222222222222222222222222222222222222222222200000000000000020000000000000001000000000000000965717569706d656e74010000000000000002343201000000000000000234320001000000000000000000000000000000050000000000000002000000000000000965717569706d656e74010000000000000002343301000000000000000234330000000000000000000200000000000000046d61696e0000000000000002343200000000000000010000000000000000000000000000000b000000000000000100000000000000000000000000000065000000000000000300000000000000046d61696e0000000000000002343300000000000000020000000000000000000000000000000c000000000000000100000000000000000000000000000066000000000000000300000000000000066163636570740000000000000000";
    const DECIDE_SHA: &str = "09087abf1e8573ff3b83b71fa8a619d9462072c22a8c755144b38fd9cd1ec313";
    const APPEND_HEX: &str = "61756374696f6e2e636f6e7369676e6d656e742e636f6d6d616e642e763200000000000000001d617070656e645f71756172616e74696e655f6f62736572766174696f6e0000000000000006776f726b6572000000000000000b636f6e7369676e6d656e740000000000000000000000000000000400000000000000056f62732d3100000000695735a500000000333333333333333333333333333333333333333333333333333333333333333300000000000000027031444444444444444444444444444444444444444444444444444444444444444401000000000000000465762d31";
    const APPEND_SHA: &str = "765fe97ad183bcbc2df74bedb54ee33d394c81a4574c228e81a7657569b9783e";
    const RECHECK_HEX: &str = "61756374696f6e2e636f6e7369676e6d656e742e636f6d6d616e642e76320000000000000000127265636865636b5f71756172616e74696e65000000000000000561646d696e000000000000000761646d696e2d31000000000000000000000000000000040000000000000005";
    const RECHECK_SHA: &str = "7fb570c4a6c599d5367874bc12878266fdf6014171e2064b033ab5016b3d96db";
    const RELEASE_ADMIN_HEX: &str = "61756374696f6e2e636f6e7369676e6d656e742e636f6d6d616e642e763200000000000000001272656c656173655f71756172616e74696e65000000000000000561646d696e000000000000000761646d696e2d3100000000000000000000000000000004000000000000000500000000000000000000000000000006000000000000000672657475726e";
    const RELEASE_ADMIN_SHA: &str =
        "6f859ec796a8ee3c456d6c256b3d75e8a64a7ba28e3274a626155b45f53c9a81";
    const RELEASE_AUTO_HEX: &str = "61756374696f6e2e636f6e7369676e6d656e742e636f6d6d616e642e763200000000000000001272656c656173655f71756172616e74696e65000000000000000673797374656d000000000000000b636f6e7369676e6d656e7400000000000000000000000000000004000000000000000500000000000000000000000000000006000000000000000672657475726e55555555555555555555555555555555555555555555555555555555555555550000000000000008736f757263652d31";
    const RELEASE_AUTO_SHA: &str =
        "0239ca0b3167ac050452ae3c9543b8769bbae49872d335b6140adac900f49453";
    const START_HEX: &str = "61756374696f6e2e636f6e7369676e6d656e742e636f6d6d616e642e763200000000000000001773746172745f646973706f736974696f6e5f77726974650000000000000006776f726b6572000000000000000b636f6e7369676e6d656e7400000000000000000000000000000008000000000000000474616b650000000000000000000000000000000900000000000000070000000000000000000000000000000a0000000000000000000000000000000b0000000000000000000000000000000c66666666666666666666666666666666666666666666666666666666666666667777777777777777777777777777777777777777777777777777777777777777";
    const START_SHA: &str = "fc5741fb44bb6343c8cb8cc058aa0c90e3353b9b1f11d4389a5d8a03bb09845b";
    const APPLY_HEX: &str = "61756374696f6e2e636f6e7369676e6d656e742e636f6d6d616e642e76320000000000000000106170706c795f6d61696c5f70726f6f660000000000000006776f726b6572000000000000000b636f6e7369676e6d656e740000000000000000000000000000000b000000000000000a65786163745f74616b6500000000000000000000000000000008000000000000000474616b6500000000000000000000000000000009000000000000000700000000000000000000000000000006000000000000000276318888888888888888888888888888888888888888888888888888888888888888";
    const APPLY_SHA: &str = "1341a8286f9bae817ff80cb80777d146efce4755767ad00d8d92295c9133b3f4";
    const WITHDRAW_HEX: &str = "61756374696f6e2e636f6e7369676e6d656e742e636f6d6d616e642e76320000000000000000087769746864726177000000000000000b7075626c69635f7573657200000000000000000000000000000007000000000000000000000000000000010000000000000003";
    const WITHDRAW_SHA: &str = "3a03cfcad73843b678c2ae00be66d69618a1ba6b734c26b6cce9fe0d5d42f523";
    const HOLD_HEX: &str = "61756374696f6e2e636f6e7369676e6d656e742e636f6d6d616e642e763200000000000000000c61646d696e5f64656c697374000000000000000561646d696e000000000000000761646d696e2d310000000000000000000000000000000100000000000000030000000000000004686f6c64";
    const HOLD_SHA: &str = "b3b62a93607fc2d7f3d9eb0aa50c051a79bb13c63ecfa38301a91f8ff5e7724d";
    const RELEASE_HOLD_HEX: &str = "61756374696f6e2e636f6e7369676e6d656e742e636f6d6d616e642e763200000000000000000c72656c656173655f686f6c64000000000000000561646d696e000000000000000761646d696e2d310000000000000000000000000000000100000000000000030000000000000000000000000000000d";
    const RELEASE_HOLD_SHA: &str =
        "9a34e038d17e47f5e1993019afc7e1c6160c51e0bbeb4c8243f8e8d81c7570a4";
    const NOTICE_HEX: &str = "61756374696f6e2e636f6e7369676e6d656e742e636f6d6d616e642e763200000000000000000d736561736f6e5f6e6f746963650000000000000006776f726b6572000000000000000b636f6e7369676e6d656e74000000000000000a323032362d31312d3031000000006ae6e37000000000000000006bd582f000000000000000006ae2eef000000000aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa00000000000000030000000000000000000000000000000e000000000000000000000000000000070000000000000013707265636c6f73655f7769746864726177616c";
    const NOTICE_SHA: &str = "29cc5ca09a2720887aa2f4f35319eb2f05900867e80d1109b78895d53cc8c915";
    const CLOSE_HEX: &str = "61756374696f6e2e636f6e7369676e6d656e742e636f6d6d616e642e763200000000000000000c736561736f6e5f636c6f73650000000000000006776f726b6572000000000000000b636f6e7369676e6d656e74000000000000000a323032362d31312d3031000000006ae6e37000000000000000006bd582f000000000000000006ae2eef000000000aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa000000000000000300000000000000076c697374696e6700000000000000000000000000000001";
    const CLOSE_SHA: &str = "611e07b126634d2ef2a9c99a5a8d123f1c02f4a387b7ca44d83abf68c32dc4f3";

    fn uuid(value: u128) -> Uuid {
        Uuid::from_u128(value)
    }

    fn user(value: u128) -> UserId {
        UserId::from_uuid(uuid(value))
    }

    fn at() -> Timestamp {
        Timestamp::from_ymd_hms(2026, 1, 2, 3, 4, 5).expect("测试时刻")
    }

    fn version() -> ListingVersion {
        ListingVersion::new(3).expect("测试版本")
    }

    fn mail_key() -> OriginalMailKey {
        OriginalMailKey::new("42", SettlementWorld::Main, "100").expect("测试原信键")
    }

    fn equipment(eid: &str) -> ListingEquipmentIdentity {
        ListingEquipmentIdentity::new("main", eid).expect("测试装备")
    }

    fn item(ordinal: u32, eid: &str, protected_ref: Option<Uuid>) -> EnvelopeItemObservation {
        EnvelopeItemObservation {
            ordinal,
            kind: EnvelopeItemKind::Equipment,
            display_name: Some(format!("Fake Gear {eid}")),
            raw_eid: Some(eid.to_owned()),
            canonical_eid: Some(eid.to_owned()),
            amount: None,
            world: Some(SettlementWorld::Main),
            seller_user_id: Some(user(7)),
            tradeable: true,
            history_ok: true,
            protected_ref,
        }
    }

    fn reservation(index: u128, eid: &str) -> ReservationBinding {
        ReservationBinding {
            reservation_id: uuid(10 + index),
            generation: 1,
            listing_id: uuid(100 + index),
            listing_version: version(),
            seller_user_id: user(7),
            equipment: equipment(eid),
            expires_at: at() + Duration::hours(1),
            state: ReservationState::Active,
        }
    }

    fn observation() -> EnvelopeObservation {
        EnvelopeObservation {
            observation_id: uuid(3),
            session_binding_digest: [0x11; 32],
            manifest_digest: [0x22; 32],
            parser_version: "p1".to_owned(),
            subject_is_consign: true,
            sender_identity: SenderIdentity::Verified,
            list_detail_consistent: true,
            asset_count: Some(2),
            duplicate_raw_eid: false,
            canonical_alias_ambiguous: false,
            unknown_attachment: false,
            credits: Some(0),
            cod: Some(0),
            returnable: true,
            takeable: Some(true),
            system_rts: false,
            legacy_auction_owner: None,
            items: vec![item(1, "42", Some(uuid(5))), item(2, "43", None)],
        }
    }

    fn facts() -> LockedEnvelopeFacts {
        LockedEnvelopeFacts {
            mail_key: Some(mail_key()),
            observation: observation(),
            reservations: vec![reservation(2, "43"), reservation(1, "42")],
            envelope_seller_user_id: Some(user(7)),
            capabilities: EnvelopeCapabilities { safe_original_return: true, take_ready: true },
            read_failure: false,
            already_decided: false,
            now: Timestamp::UNIX_EPOCH,
        }
    }

    fn matched_item(index: u32) -> MatchedDepositItem {
        MatchedDepositItem {
            ordinal: index,
            listing_id: uuid(100 + u128::from(index)),
            listing_version: version(),
            seller_user_id: user(7),
            reservation_id: uuid(10 + u128::from(index)),
            reservation_generation: 1,
            equipment: equipment(if index == 1 { "42" } else { "43" }),
            credential_ref: if index == 1 { Some(uuid(5)) } else { None },
        }
    }

    fn decide_command() -> DecideEnvelopeCommand {
        DecideEnvelopeCommand {
            command_id: CommandId::from_uuid(uuid(99)),
            request_hash: RequestHash::new([0; 32]),
            mail_key: mail_key(),
            facts: facts(),
            decision: EnvelopeDecision::Accept(vec![matched_item(1), matched_item(2)]),
        }
    }

    fn decide_command_with(decision: EnvelopeDecision) -> DecideEnvelopeCommand {
        let mut command = decide_command();
        command.decision = decision;
        command
    }

    fn season() -> FrozenSeason {
        FrozenSeason {
            world: SettlementWorld::Isekai,
            season_id: "2026-11-01".to_owned(),
            start_utc: Timestamp::from_ymd_hms(2026, 11, 1, 7, 0, 0).expect("开始"),
            end_utc: Timestamp::from_ymd_hms(2027, 5, 1, 7, 0, 0).expect("结束"),
            admission_cutoff_utc: Timestamp::from_ymd_hms(2026, 10, 29, 7, 0, 0).expect("预停"),
            calendar_version: CalendarVersion::from_bytes([0xaa; 32]),
            preclose_days: 3,
        }
    }

    fn hex(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut output = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            output.push(char::from(HEX[usize::from(byte >> 4)]));
            output.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        output
    }

    fn text_bytes(value: &str) -> Vec<u8> {
        let mut bytes = (value.len() as u64).to_be_bytes().to_vec();
        bytes.extend_from_slice(value.as_bytes());
        bytes
    }

    fn assert_vector(encoding: CommandEncoding, expected_hex: &str, expected_sha: &str) {
        assert_eq!(hex(encoding.as_bytes()), expected_hex);
        let hash = encoding.request_hash();
        assert_eq!(hex(&hash.as_bytes()), expected_sha);
    }

    #[test]
    fn every_phase_four_operation_has_a_frozen_golden_vector() {
        assert_vector(
            canonical_issue_reservation(&IssueReservationCommand {
                command_id: CommandId::from_uuid(uuid(90)),
                seller_user_id: user(7),
                listing_id: uuid(1),
                expected_version: version(),
                request_hash: RequestHash::new([0; 32]),
                request_id: None,
            }),
            ISSUE_HEX,
            ISSUE_SHA,
        );
        assert_vector(
            canonical_expire_reservation(&ExpireReservationCommand {
                command_id: CommandId::from_uuid(uuid(91)),
                reservation_id: uuid(2),
                generation: 4,
                invalidation: ReservationInvalidation::Expired { expires_at: at() },
                request_hash: RequestHash::new([0; 32]),
            }),
            EXPIRE_HEX,
            EXPIRE_SHA,
        );
        assert_vector(canonical_decide_envelope(&decide_command()), DECIDE_HEX, DECIDE_SHA);
        assert_vector(
            canonical_append_observation(&AppendQuarantineObservationCommand {
                command_id: CommandId::from_uuid(uuid(92)),
                request_hash: RequestHash::new([0; 32]),
                quarantine_id: uuid(4),
                remote_observation_id: "obs-1".to_owned(),
                observed_at: at(),
                observation: {
                    let mut observation = observation();
                    observation.session_binding_digest = [0x33; 32];
                    observation.manifest_digest = [0x44; 32];
                    observation
                },
                evidence_ref: Some("ev-1".to_owned()),
            }),
            APPEND_HEX,
            APPEND_SHA,
        );
        assert_vector(
            canonical_recheck_quarantine(&RecheckQuarantineCommand {
                command_id: CommandId::from_uuid(uuid(93)),
                admin_actor: "admin-1".to_owned(),
                quarantine_id: uuid(4),
                expected_version: 5,
                request_hash: RequestHash::new([0; 32]),
                request_id: None,
            }),
            RECHECK_HEX,
            RECHECK_SHA,
        );
        assert_vector(
            canonical_release_quarantine(&ReleaseQuarantineCommand {
                command_id: CommandId::from_uuid(uuid(94)),
                actor: DepositActor::Admin("admin-1".to_owned()),
                quarantine_id: uuid(4),
                expected_version: 5,
                observation_id: uuid(6),
                proposed_kind: auction_domain::consignment::deposit::ReleaseKind::Return,
                request_hash: RequestHash::new([0; 32]),
                request_id: None,
                auto_binding: None,
            }),
            RELEASE_ADMIN_HEX,
            RELEASE_ADMIN_SHA,
        );
        assert_vector(
            canonical_release_quarantine(&ReleaseQuarantineCommand {
                command_id: CommandId::from_uuid(uuid(95)),
                actor: DepositActor::System,
                quarantine_id: uuid(4),
                expected_version: 5,
                observation_id: uuid(6),
                proposed_kind: auction_domain::consignment::deposit::ReleaseKind::Return,
                request_hash: RequestHash::new([0; 32]),
                request_id: None,
                auto_binding: Some(AutoReleaseBinding {
                    observation_digest: [0x55; 32],
                    source_ref: "source-1".to_owned(),
                }),
            }),
            RELEASE_AUTO_HEX,
            RELEASE_AUTO_SHA,
        );
        assert_vector(
            canonical_start_disposition_write(&StartDispositionWriteCommand {
                command_id: CommandId::from_uuid(uuid(96)),
                request_hash: RequestHash::new([0; 32]),
                final_decision_id: uuid(8),
                action: DispositionAction::Take,
                delivery_id: uuid(9),
                fence: 7,
                work_id: uuid(10),
                attempt_id: uuid(11),
                claim_token: uuid(12),
                session_binding_digest: [0x66; 32],
                expected_manifest_digest: [0x77; 32],
            }),
            START_HEX,
            START_SHA,
        );
        assert_vector(
            canonical_apply_mail_proof(&ApplyMailProofCommand {
                command_id: CommandId::from_uuid(uuid(97)),
                request_hash: RequestHash::new([0; 32]),
                original_attempt_id: uuid(11),
                proof_kind: MailProofKind::ExactTake,
                final_decision_id: uuid(8),
                action: DispositionAction::Take,
                delivery_id: uuid(9),
                fence: 7,
                observation_id: uuid(6),
                validator_version: "v1".to_owned(),
                proof_digest: [0x88; 32],
            }),
            APPLY_HEX,
            APPLY_SHA,
        );
        assert_vector(
            canonical_withdraw(&WithdrawDepositCommand {
                command_id: CommandId::from_uuid(uuid(98)),
                seller_user_id: user(7),
                listing_id: uuid(1),
                expected_version: version(),
                request_hash: RequestHash::new([0; 32]),
                request_id: None,
            }),
            WITHDRAW_HEX,
            WITHDRAW_SHA,
        );
        assert_vector(
            canonical_set_hold(&SetOperationsHoldCommand {
                admin_actor: "admin-1".to_owned(),
                command_id: CommandId::from_uuid(uuid(99)),
                listing_id: uuid(1),
                expected_version: version(),
                request_hash: RequestHash::new([0; 32]),
                request_id: None,
                reason: ListingDelistReason::new("hold".to_owned()).expect("说明"),
            }),
            HOLD_HEX,
            HOLD_SHA,
        );
        assert_vector(
            canonical_release_hold(&ReleaseOperationsHoldCommand {
                admin_actor: "admin-1".to_owned(),
                command_id: CommandId::from_uuid(uuid(100)),
                listing_id: uuid(1),
                expected_version: version(),
                hold_id: uuid(13),
                request_hash: RequestHash::new([0; 32]),
                request_id: None,
            }),
            RELEASE_HOLD_HEX,
            RELEASE_HOLD_SHA,
        );
        assert_vector(
            canonical_season_notice(&SeasonNoticeCommand {
                command_id: CommandId::from_uuid(uuid(101)),
                season: season(),
                position_id: uuid(14),
                seller_user_id: user(7),
                notice_kind: SeasonNoticeKind::PrecloseWithdrawal,
                request_hash: RequestHash::new([0; 32]),
            }),
            NOTICE_HEX,
            NOTICE_SHA,
        );
        assert_vector(
            canonical_season_close(&SeasonCloseCommand {
                command_id: CommandId::from_uuid(uuid(102)),
                season: season(),
                target: SeasonCloseTarget::Listing(uuid(1)),
                request_hash: RequestHash::new([0; 32]),
            }),
            CLOSE_HEX,
            CLOSE_SHA,
        );
    }

    #[test]
    fn system_ids_have_frozen_golden_vectors() {
        // 2026-11-21 统筹方裁定冻结：decide_envelope 的业务键仍为 K，系统 ID 重算后不变。
        assert_eq!(
            system_command_id(
                crate::ports::consignment::DepositOperation::DecideEnvelope,
                &mail_key_business_key(&mail_key()),
            )
            .as_uuid()
            .to_string(),
            "3286d0e8-0733-513b-afab-a1b4a2ece1ed"
        );
        assert_eq!(
            system_command_id(
                crate::ports::consignment::DepositOperation::StartDispositionWrite,
                &start_write_business_key(uuid(8), DispositionAction::Take, uuid(9), 7),
            )
            .as_uuid()
            .to_string(),
            "2999169f-e2c0-503f-8e57-3de49e2a3175"
        );
        assert_eq!(
            system_command_id(
                crate::ports::consignment::DepositOperation::ExpireReservation,
                &expire_business_key(
                    uuid(2),
                    4,
                    ReservationInvalidation::Expired { expires_at: at() }
                ),
            )
            .as_uuid()
            .to_string(),
            "7f485a6d-c30e-5275-9d6e-02b374e12def"
        );
        assert_eq!(
            system_command_id(
                crate::ports::consignment::DepositOperation::SeasonNotice,
                &season_notice_business_key("2026-11-01", uuid(14), user(7)),
            )
            .as_uuid()
            .to_string(),
            "85c6b883-7467-50b9-aa5b-8754ca0a9fcb"
        );
        let mut release_key = uuid(4).as_bytes().to_vec();
        release_key.extend_from_slice(uuid(6).as_bytes());
        assert_eq!(
            system_command_id(
                crate::ports::consignment::DepositOperation::ReleaseQuarantine,
                &release_key,
            )
            .as_uuid()
            .to_string(),
            "f15f48c8-b002-5ec9-86b7-de5a1997862f"
        );
    }

    #[test]
    fn request_id_and_processing_time_do_not_change_digest() {
        let first = IssueReservationCommand {
            command_id: CommandId::from_uuid(uuid(90)),
            seller_user_id: user(7),
            listing_id: uuid(1),
            expected_version: version(),
            request_hash: RequestHash::new([0; 32]),
            request_id: Some("audit-1".to_owned()),
        };
        let second = IssueReservationCommand {
            command_id: first.command_id,
            seller_user_id: first.seller_user_id,
            listing_id: first.listing_id,
            expected_version: first.expected_version,
            request_hash: first.request_hash,
            request_id: Some("audit-2".to_owned()),
        };
        assert_eq!(
            canonical_issue_reservation(&first).request_hash(),
            canonical_issue_reservation(&second).request_hash()
        );
        let mut later = decide_command();
        later.facts.now = at();
        assert_eq!(
            canonical_decide_envelope(&decide_command()).request_hash(),
            canonical_decide_envelope(&later).request_hash()
        );
    }

    #[test]
    fn decide_evidence_encodes_kind_then_ordered_return_reasons() {
        let command = decide_command_with(EnvelopeDecision::Return(ReturnManifest {
            items: vec![
                ReturnItem {
                    ordinal: 2,
                    canonical_eid: Some("43".to_owned()),
                    reason: ReturnItemReason::ReservationCountMismatch,
                },
                ReturnItem {
                    ordinal: 1,
                    canonical_eid: Some("42".to_owned()),
                    reason: ReturnItemReason::Unregistered,
                },
            ],
        }));
        let mut expected = text_bytes("return");
        expected.extend_from_slice(&2_u64.to_be_bytes());
        expected.extend_from_slice(&1_u64.to_be_bytes());
        expected.extend_from_slice(&text_bytes("unregistered"));
        expected.extend_from_slice(&2_u64.to_be_bytes());
        expected.extend_from_slice(&text_bytes("reservation_count_mismatch"));
        assert!(canonical_decide_envelope(&command).as_bytes().ends_with(&expected));
    }

    #[test]
    fn decide_quarantine_evidence_is_the_reason_text() {
        let command =
            decide_command_with(EnvelopeDecision::Quarantine(QuarantineReason::ReturnUnproven));
        let mut expected = text_bytes("quarantine");
        expected.extend_from_slice(&text_bytes("return_unproven"));
        assert!(canonical_decide_envelope(&command).as_bytes().ends_with(&expected));
    }

    #[test]
    fn decide_accept_and_archive_empty_evidence_are_empty_lists() {
        for decision in [
            EnvelopeDecision::Accept(vec![matched_item(1), matched_item(2)]),
            EnvelopeDecision::ArchiveEmpty,
        ] {
            let command = decide_command_with(decision);
            let mut expected = text_bytes(command.decision.decision_kind());
            expected.extend_from_slice(&0_u64.to_be_bytes());
            assert!(canonical_decide_envelope(&command).as_bytes().ends_with(&expected));
        }
    }

    #[test]
    fn different_decision_evidence_changes_the_digest() {
        let first = decide_command_with(EnvelopeDecision::Return(ReturnManifest {
            items: vec![ReturnItem {
                ordinal: 1,
                canonical_eid: Some("42".to_owned()),
                reason: ReturnItemReason::Unregistered,
            }],
        }));
        let second = decide_command_with(EnvelopeDecision::Return(ReturnManifest {
            items: vec![ReturnItem {
                ordinal: 1,
                canonical_eid: Some("42".to_owned()),
                reason: ReturnItemReason::NotTradeable,
            }],
        }));
        assert_ne!(
            canonical_decide_envelope(&first).request_hash(),
            canonical_decide_envelope(&second).request_hash()
        );
        assert_ne!(
            canonical_decide_envelope(&decide_command_with(EnvelopeDecision::Quarantine(
                QuarantineReason::ReturnUnproven,
            )))
            .request_hash(),
            canonical_decide_envelope(&decide_command_with(EnvelopeDecision::Quarantine(
                QuarantineReason::UnsafeReturnShape,
            )))
            .request_hash()
        );
    }

    #[test]
    fn reservation_set_sorts_by_business_key_and_items_keep_original_order() {
        let command = decide_command();
        let mut reversed = command.clone();
        reversed.facts.reservations.reverse();
        assert_eq!(
            canonical_decide_envelope(&command).as_bytes(),
            canonical_decide_envelope(&reversed).as_bytes()
        );
        let mut reordered = command;
        reordered.facts.observation.items.reverse();
        assert_ne!(
            canonical_decide_envelope(&decide_command()).as_bytes(),
            canonical_decide_envelope(&reordered).as_bytes()
        );
    }

    #[test]
    fn nullable_fields_use_one_byte_tags() {
        let base = AppendQuarantineObservationCommand {
            command_id: CommandId::from_uuid(uuid(92)),
            request_hash: RequestHash::new([0; 32]),
            quarantine_id: uuid(4),
            remote_observation_id: "obs-1".to_owned(),
            observed_at: at(),
            observation: observation(),
            evidence_ref: None,
        };
        let mut present = base.clone();
        present.evidence_ref = Some("ev-1".to_owned());
        let absent = canonical_append_observation(&base);
        let present = canonical_append_observation(&present);
        assert_eq!(absent.as_bytes().last(), Some(&0));
        assert!(present.as_bytes().len() > absent.as_bytes().len());
        assert_eq!(present.as_bytes()[absent.as_bytes().len() - 1], 1);
    }

    #[test]
    fn v1_reprice_vector_is_unchanged() {
        use crate::ports::consignment::ListingOperation;
        use crate::services::consignment::encoding::ListingCommandEncoding;

        let mut input = ListingCommandEncoding::target_version(
            ListingOperation::Reprice,
            uuid(1),
            ListingVersion::new(3).expect("版本"),
        );
        input.bytes(&101_i64.to_be_bytes());
        assert_eq!(
            hex(&input.request_hash().as_bytes()),
            "5db257758a554cbb72ec138b68e31ee416e2de30da5d9143660927699f6cc6d7"
        );
    }
}
