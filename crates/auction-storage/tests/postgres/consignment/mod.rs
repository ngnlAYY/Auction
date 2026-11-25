//! 一口价挂单的 PG18 集成测试：迁移与约束、命令底座、资格与可见性谓词、解析工作原语、密钥轮换。
//!
//! 会写入进程级审计健康计数的场景在独立的 `postgres_audit` 测试目标运行。

mod acceptance;
mod collection;
mod commands;
mod contracts;
mod delivery_guard;
mod delivery_reads;
mod deposit;
mod dispatch;
mod eligibility;
mod funding;
mod key_rotation;
mod ledger136;
mod lookup;
mod migration;
mod order_admin;
mod payout;
mod purchase;
mod readiness;
mod reads;
mod recovery;
mod refusal;
mod registration_fixture;
mod returns;
mod rights;
mod work_store;
mod writes;
