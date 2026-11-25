//! 寄售 Worker 的邮件适配：隔离重检读取器、处置传输、权利通知发信，以及派发、补寄与退件 Take
//! 三个 executor 的工作来源与外写传输，以及订单复核与收款确领推进的只读 Sent 读取。

mod collection;
mod content;
mod dispatch;
mod executors;
mod reader;
mod readiness;
mod recheck;
mod return_take;
mod rights_notice;
mod transport;
mod work;

pub use content::FrozenFirstDispatchContent;
pub use executors::{ExecutorInputs, executor_runtimes};
pub use reader::WorkerConsignmentRecheckReader;
pub use readiness::readiness_runtimes;
pub use rights_notice::{MoogleMailRightNoticeTransport, SealedNoticeDeliverer};
pub use transport::MoogleMailConsignmentTransport;
