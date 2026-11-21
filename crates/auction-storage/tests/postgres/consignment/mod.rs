//! 一口价挂单的 PG18 集成测试：迁移与约束、命令底座、资格与可见性谓词、解析工作原语、密钥轮换。
//!
//! 会触发 `record_audit_failure()` 的场景必须经 `audit_subprocess` 在子进程运行，
//! 并在注入处标记对应的父测试，避免影响同进程的就绪检查。

mod audit_subprocess;
mod commands;
mod eligibility;
mod key_rotation;
mod lookup;
mod migration;
mod work_store;
mod writes;
