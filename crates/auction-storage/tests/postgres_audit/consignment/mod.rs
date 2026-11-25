//! 寄售审计故障在独立测试进程运行，避免污染 postgres 的就绪检查。
//!
//! `contracts`、`deposit`、`purchase`、`recovery` 等子目录按 postgres 目标的模块路径镜像它的夹具
//! （符号链接），夹具里的 `super::super::…` 与 `crate::consignment::…` 路径因此原样可用；各目录只另放
//! 本目标的审计失败用例，共用的注入与快照在 `audit_fault`。

mod audit_fault;
mod contracts;
mod deposit;
mod deposit_fixture;
mod dispatch;
mod funding;
mod ledger136;
mod order_admin;
mod purchase;
mod reaper;
mod recovery;
mod refusal;
mod returns;
mod rights;
mod termination;

// 复用 postgres 目标的夹具；其他方法仍由原目标的测试使用。
#[allow(dead_code)]
mod registration_fixture;
#[allow(dead_code)]
mod work_store_fixture;
#[allow(dead_code)]
mod writes_fixture;
