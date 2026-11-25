//! 受限权利存储（7D-2）的 PG18 测试：挑战失败上限、通知节流、会话轮换不延寿、浏览器绑定、
//! 真实与虚拟目标同一段写路径、预算、密钥轮换失败关闭、续办命令幂等，以及 `right_id` 与 SQL 的对照。
//!
//! 只写核心规则，每条一正一反；会改动进程级审计健康计数的场景不在这里。

mod budget;
mod challenge;
mod entitlement;
mod fixture;
mod identity;
mod key_rotation;
mod notice;
mod session;
