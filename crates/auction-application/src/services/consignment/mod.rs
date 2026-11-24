//! 寄售登记、本人及运营命令、单轮解析和维护用例。

pub mod deposit;
mod encoding;
mod maintenance;
mod manage;
mod register;
mod resolve;

pub use deposit::encoding::{
    canonical_system_recheck_request_hash, system_command_id, system_recheck_business_key,
};
pub use deposit::{
    ApplyMailProofInput, DepositCommandService, ReleaseQuarantineAdminInput,
    StartDispositionWriteInput,
};
pub use encoding::{auto_close_command, lookup_reap_exhausted_identity};
pub use maintenance::{ListingMaintenanceProgress, ListingMaintenanceService};
pub use manage::ManageListingService;
pub use register::{RegisterListingInput, RegisterListingService};
pub use resolve::{ListingLookupRound, ResolveListingService};

#[cfg(test)]
mod tests;
