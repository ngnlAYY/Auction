//! 恢复处置与灭失确认写口的审计失败用例，以及镜像 postgres 目标 `recovery` 目录的夹具。

mod audit_failure;
// 镜像 postgres 目标的同名夹具（符号链接）；其余条目只由原目标的测试使用。
#[allow(dead_code)]
mod command_sql;
mod confirm_loss;
// 镜像 postgres 目标的同名夹具（符号链接）；其余条目只由原目标的测试使用。
#[allow(dead_code)]
mod fixture;
pub(super) mod sale_recovery;

pub(super) use fixture::{CASE, FACT, PROOF, case_sql, command_ids, fact_sql, proof_sql};
