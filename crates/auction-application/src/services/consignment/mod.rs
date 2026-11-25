//! 寄售登记、本人及运营命令、单轮解析和维护用例。

mod contract;
pub mod deposit;
pub mod dispatch;
mod encoding;
mod maintenance;
mod manage;
pub mod purchase;
pub mod recovery;
mod register;
mod resolve;
pub mod returns;
pub mod rights;

pub use contract::{
    ConsignmentSendFacts, ConsignmentSendIntent, ConsignmentTemplateError,
    build_consignment_send_intent, consignment_send_template_kind, consignment_send_template_name,
};
pub use deposit::encoding::{
    canonical_system_recheck_request_hash, system_command_id, system_recheck_business_key,
};
pub use deposit::{
    ApplyMailProofInput, DepositCommandService, ReleaseQuarantineAdminInput,
    StartDispositionWriteInput,
};
pub use dispatch::{DispatchService, ProveHandoffInput, RecordEvidenceInput, certify_cod_command};
pub use encoding::{auto_close_command, lookup_reap_exhausted_identity};
pub use maintenance::{ListingMaintenanceProgress, ListingMaintenanceService};
pub use manage::ManageListingService;
pub use purchase::{PurchaseCommandService, purchase_funding};
pub use recovery::encoding::{
    canonical_confirm_position_loss, canonical_fulfill_withdraw_intent,
    canonical_record_loss_case_outcome, canonical_record_position_loss_proof, canonical_relist,
    loss_proof_business_key, loss_proof_id, withdraw_intent_business_key,
};
pub use recovery::loss_admin::{
    ConfirmPositionLossInput, LossCaseAdminService, PositionLossConfirmService,
    RecordLossOutcomeInput, RecordLossProofInput,
};
pub use recovery::order_admin::{
    OrderAdminService, RecheckOrderInput, ResolveOrderInput, canonical_recheck_order,
    canonical_resolve_order,
};
pub use recovery::{
    AutoRelistService, ManualRelistService, PaymentReminderService, ReturnDispositionService,
    apply_sanction_command, deliver_right_notice_command, open_sanction_command,
    reset_cycle_command,
};
pub use register::{RegisterListingInput, RegisterListingService};
pub use resolve::{ListingLookupRound, ResolveListingService};
pub use returns::{ConsumeReturnInput, ReturnService, StartTakeInput};
pub use rights::{
    ChallengeRightInput, ClaimRightInput, LocateRightInput, RightsService, VerifyRightInput,
};

#[cfg(test)]
mod tests;
