//! 寄售 Worker 的 4D1 适配：隔离重检读取器与处置传输。

mod reader;
mod readiness;
mod transport;

pub use reader::WorkerConsignmentRecheckReader;
pub use readiness::readiness_runtimes;
pub use transport::MoogleMailConsignmentTransport;
