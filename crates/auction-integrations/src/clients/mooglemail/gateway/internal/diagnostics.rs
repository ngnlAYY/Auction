//! 联合证据失败的闭集阶段；不记录页面、URL、UID、装备标识或凭据材料。

use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};
use auction_application::settlement::InternalEvidenceError;

use super::{MoogleMailErrorCategory, MoogleMailOperation, MoogleMailWorld};
use crate::safe_diagnostics;

#[derive(Clone, Copy)]
pub(super) enum EvidenceStage {
    Request,
    SessionAcquire,
    SessionOwnership,
    RuntimeBinding,
    OwnerCheck,
    IdentityGet,
    UidParse,
    ProfileCapability,
    ProfileRead,
    ProfileValidation,
    AccountBinding,
    ComposeGet,
    ComposeWorld,
    ComposeParse,
    ComposeDirty,
    InventoryGet,
    InventoryParse,
    InventoryMissing,
    CashGet,
    CashWorld,
    CashParse,
    SnapshotProof,
    SnapshotGuard,
    Deadline,
}

impl EvidenceStage {
    const fn diagnostic_phase(self) -> &'static str {
        match self {
            Self::Request => "validate_request",
            Self::SessionAcquire => "acquire_session",
            Self::SessionOwnership => "validate_session_owner",
            Self::RuntimeBinding => "validate_runtime_binding",
            Self::OwnerCheck => "check_owner",
            Self::IdentityGet => "read_identity",
            Self::UidParse => "parse_identity_uid",
            Self::ProfileCapability => "require_profile_reader",
            Self::ProfileRead => "read_profile",
            Self::ProfileValidation => "validate_profile",
            Self::AccountBinding => "validate_account_binding",
            Self::ComposeGet => "read_compose",
            Self::ComposeWorld => "validate_compose_world",
            Self::ComposeParse => "parse_compose",
            Self::ComposeDirty => "reject_dirty_compose",
            Self::InventoryGet => "read_inventory",
            Self::InventoryParse => "parse_inventory",
            Self::InventoryMissing => "require_equipment",
            Self::CashGet => "read_balance",
            Self::CashWorld => "validate_balance_world",
            Self::CashParse => "parse_balance",
            Self::SnapshotProof => "validate_snapshot",
            Self::SnapshotGuard => "acquire_snapshot_guard",
            Self::Deadline => "enforce_deadline",
        }
    }
}

pub(super) fn at(
    stage: EvidenceStage,
    world: MoogleMailWorld,
    error: InternalEvidenceError,
) -> InternalEvidenceError {
    safe_diagnostics::warn(
        "mooglemail_evidence",
        stage.diagnostic_phase(),
        safe_diagnostics::project(&error),
        DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
    );
    error
}

// 原错误只投影三个闭集枚举；不格式化整个错误或其证据引用、底层文本。
pub(super) fn read_error(
    _operation: MoogleMailOperation,
    world: MoogleMailWorld,
    category: MoogleMailErrorCategory,
) {
    safe_diagnostics::warn(
        "mooglemail_evidence",
        "reject_typed_read",
        safe_diagnostics::category(safe_diagnostics::mooglemail_kind(category)),
        DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
    );
}
