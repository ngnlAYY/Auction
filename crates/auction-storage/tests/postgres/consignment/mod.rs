//! 一口价挂单的 PG18 集成测试：迁移与约束、命令底座、资格与可见性谓词、解析工作原语、密钥轮换。
//!
//! 会触发 `record_audit_failure()` 的场景必须经 `audit_subprocess` 在子进程运行，
//! 用 `isolated_audit_scenario!` 登记，并在场景定义处标记父测试与场景函数名。

mod audit_subprocess;
mod commands;
mod eligibility;
mod key_rotation;
mod lookup;
mod migration;
mod work_store;
mod writes;
