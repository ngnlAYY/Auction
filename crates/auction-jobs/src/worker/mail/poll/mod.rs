//! MoogleMail Worker 的远程读取编排；每个 world 拥有独立调度器、
//! 唤醒器与持久化邮箱状态，本模块负责 claim → 续租 → 远程读取 → 提交/失败。

pub(super) mod cycle;
pub(super) mod failure;
pub(super) mod mapping;
pub(super) mod processing;
pub(super) mod types;

pub(crate) use super::maintenance::run_loop as run_maintenance_loop;
pub use super::schedule::{PhasedMailSchedule, mail_world_phase};
pub(crate) use cycle::run_loop;
pub use types::{
    MailPollFetchRequest, MailPollFetchedMessage, MailPollGateway, MailPollGatewayError,
    MailPollPage, MailPollRuntime, MailPollRuntimeBindings, MailPollRuntimeConfig,
    MailPollSchedules,
};
