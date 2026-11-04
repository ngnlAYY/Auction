//! 仅供 dev-dependencies 使用的回环外部服务替身；不连接真实账号。

// 该 crate 只进入测试进程；常量 selector 与测试状态锁中毒时直接失败，避免为不可恢复的测试装配伪造回退。
#![allow(clippy::expect_used)]

pub mod forum;
pub mod mooglemail;
