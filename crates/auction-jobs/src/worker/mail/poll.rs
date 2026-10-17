//! MoogleMail Worker 的远程读取编排；每个 world 拥有独立调度器、
//! 唤醒器与持久化邮箱状态，本模块负责 claim → 续租 → 远程读取 → 提交/失败。

#[path = "../mail/poll/cycle.rs"]
pub(super) mod cycle;
#[path = "../mail/poll/failure.rs"]
pub(super) mod failure;
#[path = "../mail/poll/mapping.rs"]
pub(super) mod mapping;
#[path = "../mail/poll/processing/mod.rs"]
pub(super) mod processing;
#[path = "../mail/poll/types.rs"]
pub(super) mod types;

pub(crate) use cycle::run_loop;
pub use types::{
    MailPollFetchRequest, MailPollFetchedMessage, MailPollGateway, MailPollGatewayError,
    MailPollPage, MailPollRuntime, MailPollRuntimeConfig,
};
