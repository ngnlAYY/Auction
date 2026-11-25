//! 寄售领域的集成测试入口：合同金额与义务图向量、资金分账向量、订单阶段矩阵、recovery 授权矩阵与拒收计数向量，
//! 各为一个子模块，合成一个测试目标（同目录同前缀文件过多时按职责收进子目录）。

// 集成测试目标只含测试代码，断言直接解包。
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod contract_vectors;
mod dispatch_vectors;
mod funding_vectors;
mod graph_parity;
mod order_stage_vectors;
mod purchase_vectors;
mod recovery_matrix;
mod recovery_withdraw_loss;
mod refusal_vectors;
