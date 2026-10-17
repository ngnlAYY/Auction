//! Recovery 退回履约记录与下一步 action 建立；子模块承载编排、证据、校验、幂等写入和行映射，
//! record_return_postgres 仍是唯一事务入口，外部副作用由后续 delivery owner 负责。

#[cfg(feature = "postgres")]
use super::*;

#[cfg(feature = "postgres")]
use action::insert_recovery_action;
#[cfg(feature = "postgres")]
use mapping::map_recovery_return_row;
#[cfg(feature = "postgres")]
use model::{
    ExpectedRecoveryReturn, RecoveryActionInsert, RecoveryReturnMetadata, RecoveryReturnRow,
};
#[cfg(feature = "postgres")]
use validation::{expected_recovery_return, validate_recovery_return_metadata};

#[path = "action.rs"]
mod action;
#[path = "mapping.rs"]
mod mapping;
#[path = "model.rs"]
mod model;
#[path = "record.rs"]
mod record;
#[path = "validation.rs"]
mod validation;

#[cfg(feature = "postgres")]
pub(super) use record::record_return_postgres;
