//! 已证代管件灭失的证据、案件、人工结果与卖家回执的存储原语（7C-4a）。
//!
//! 证据、事实、案件、结果与卖家回执全部只追加（不变量 8）：更正形成新事实，不改历史。
//! 人工结果只追加只读状态，没有金额、付款或发行列，也不写任何资金表（不变量 6）。
//! 本模块的原语都在调用方已开启的顶层事务内执行，命令账本、实物锁与 `position → lost` 迁移由
//! 调用方（`commands` 的三个命令、7M-2 的账本约束）负责；原语自身只做同事务内的存在、版本与冲突复核。
//! 灭失确认（7C-4b）的事务体在 `confirm`，当前代权责分流所需的锁内读取在 `routing`。

mod admin;
mod case;
mod commands;
mod confirm;
mod outcome;
mod proof;
mod proof_facts;
mod reads;
mod reconcile;
mod routing;
mod seller;
#[cfg(feature = "test-db")]
mod test_access;

pub use case::OpenedLossCase;
#[cfg(feature = "test-db")]
pub use test_access::TestLossProof;
