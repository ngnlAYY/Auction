//! 镜像 postgres 目标 `recovery/sale_recovery` 的夹具：真实写端取得并上架的 `World` 与已收款成交存量。

// 镜像 postgres 目标的同名夹具（符号链接）；其余条目只由原目标的测试使用。
#[allow(dead_code)]
mod fixture;
// 镜像 postgres 目标的同名夹具（符号链接）；其余条目只由原目标的测试使用。
#[allow(dead_code)]
pub(in crate::consignment) mod refund_call;

pub(super) use fixture::{World, id};
