//! 寄售退件接管的领域规则：退回腿、接管判定、Take 起写判定与消费判定。
//!
//! 纯规则，不读时钟、不访问存储；唯一键、锁与事务由存储车道承担。
//! 任何事实矛盾都失败关闭：不接管、不起写、不消费，由人工或后续证据核对。

mod consumption;
mod errors;
mod intake;
mod leg;
mod take;

pub use consumption::{
    ConsumeCommandRef, ConsumeConflict, ConsumeDecision, ConsumeFacts, ConsumePlan,
    ExistingConsumption, ReconcileReason, TakeProofFacts, decide_consumption,
};
pub use errors::ReturnRuleError;
pub use intake::{
    AcquireConflict, AcquirePlan, AcquireQuarantine, ExistingIntake, KFinalDisposition,
    LockedAcquireFacts, ReturnIntakeFacts, RtsOrigin, decide_acquire_return, return_intake_id,
};
pub use leg::{ConsumeEffect, ReturnLeg};
pub use take::{
    COMMAND_PREFIX_V2, ClaimFacts, StartedRef, TakeStartDecision, TakeStartFacts,
    TakeStartRejection, TakeStartRequest, decide_take_start, start_return_take_command_id,
};
