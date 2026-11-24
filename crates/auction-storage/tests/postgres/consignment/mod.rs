//! 一口价挂单的 PG18 集成测试：迁移与约束、命令底座、资格与可见性谓词、解析工作原语、密钥轮换。
//!
//! 会写入进程级审计健康计数的场景在独立的 `postgres_audit` 测试目标运行。

mod commands;
mod delivery_guard;
mod delivery_reads;
mod deposit;
mod eligibility;
mod key_rotation;
mod lookup;
mod migration;
mod readiness;
mod reads;
mod registration_fixture;
mod work_store;
mod writes;
