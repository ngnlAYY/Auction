//! MoogleMail Worker 的模块装配；远程读取、维护与节奏各自拥有独立状态，
//! 不共享游标，也不把外部调用带进调度层。

pub(super) mod maintenance;
pub(super) mod poll;
pub(super) mod schedule;
