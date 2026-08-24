//! Recovery action 运行时的模块装配。
//!
//! 准备、激活、观察、重试和买家黑名单处理分别由子模块负责；本文件只保留 runtime
//! 使用的稳定内部出口，不改变 action fence 或外部副作用顺序。

use super::*;

#[path = "recovery/activation.rs"]
mod activation;
#[path = "recovery/blacklist.rs"]
mod blacklist;
#[path = "recovery/dispatch.rs"]
mod dispatch;
#[path = "recovery/observation.rs"]
mod observation;
#[path = "recovery/retry.rs"]
mod retry;

pub(super) use activation::*;
pub(super) use blacklist::*;
pub(super) use dispatch::*;
pub(super) use observation::*;
pub(super) use retry::*;
