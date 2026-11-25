//! 受限权利写口的审计失败用例，以及镜像 postgres 目标 `rights` 目录的夹具。

mod audit_failure;
// 镜像 postgres 目标的同名夹具（符号链接）；其余条目只由原目标的测试使用。
#[allow(dead_code)]
mod fixture;
