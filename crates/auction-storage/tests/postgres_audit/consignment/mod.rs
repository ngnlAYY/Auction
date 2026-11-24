//! 寄售审计故障在独立测试进程运行，避免污染 postgres 的就绪检查。

mod deposit;
mod deposit_fixture;
mod reaper;
mod termination;

// 复用 postgres 目标的夹具；其他方法仍由原目标的测试使用。
#[allow(dead_code)]
mod registration_fixture;
#[allow(dead_code)]
mod work_store_fixture;
#[allow(dead_code)]
mod writes_fixture;
