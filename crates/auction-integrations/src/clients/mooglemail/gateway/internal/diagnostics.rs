//! 联合证据失败的闭集阶段；不记录页面、URL、UID、装备标识或凭据材料。

use auction_application::InternalEvidenceError;

use super::{MoogleMailErrorCategory, MoogleMailOperation, MoogleMailWorld};

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
    const fn as_str(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::SessionAcquire => "session_acquire",
            Self::SessionOwnership => "session_ownership",
            Self::RuntimeBinding => "runtime_binding",
            Self::OwnerCheck => "owner_check",
            Self::IdentityGet => "identity_get",
            Self::UidParse => "uid_parse",
            Self::ProfileCapability => "profile_capability",
            Self::ProfileRead => "profile_read",
            Self::ProfileValidation => "profile_validation",
            Self::AccountBinding => "account_binding",
            Self::ComposeGet => "compose_get",
            Self::ComposeWorld => "compose_world",
            Self::ComposeParse => "compose_parse",
            Self::ComposeDirty => "compose_dirty",
            Self::InventoryGet => "inventory_get",
            Self::InventoryParse => "inventory_parse",
            Self::InventoryMissing => "inventory_missing",
            Self::CashGet => "cash_get",
            Self::CashWorld => "cash_world",
            Self::CashParse => "cash_parse",
            Self::SnapshotProof => "snapshot_proof",
            Self::SnapshotGuard => "snapshot_guard",
            Self::Deadline => "deadline",
        }
    }
}

pub(super) fn at(
    stage: EvidenceStage,
    world: MoogleMailWorld,
    error: InternalEvidenceError,
) -> InternalEvidenceError {
    tracing::warn!(
        stage = stage.as_str(),
        world = world.as_str(),
        error = ?error,
        "MoogleMail internal evidence failed"
    );
    error
}

// 原错误只投影三个闭集枚举；不格式化整个错误或其证据引用、底层文本。
pub(super) fn read_error(
    operation: MoogleMailOperation,
    world: MoogleMailWorld,
    category: MoogleMailErrorCategory,
) {
    tracing::warn!(
        stage = "typed_read",
        operation = operation.as_str(),
        world = world.as_str(),
        category = category.as_str(),
        "MoogleMail internal evidence read rejected"
    );
}
