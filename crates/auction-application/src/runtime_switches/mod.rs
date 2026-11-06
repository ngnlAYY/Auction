//! runtime_switches 上下文公开 API。

pub use crate::ports::runtime_switches::{
    RuntimeReadinessPolicy, RuntimeSwitchCommand, RuntimeSwitchError, RuntimeSwitchEvent,
    RuntimeSwitchFuture, RuntimeSwitchName, RuntimeSwitchOperation, RuntimeSwitchRead,
    RuntimeSwitchRole, RuntimeSwitchStatus, RuntimeSwitchStore, RuntimeSwitchView, RuntimeSwitches,
    audit_label,
};
pub use crate::services::runtime_switches::RuntimeSwitchRefreshService;
