//! 000137 独立表段的 PG18 测试：损失事实与案件、受限权利与挑战、权利通知 outbox 的约束，运行账号授权与 restore
//! 重放，以及 136 到 137 升级时既有行不变、权利回填确定。
//!
//! 夹具：main 世界代管中的 position（处分代 1）加一份合法 Sale 合同；命令账本里已有的命令行供各表的外键引用。

mod acl;
mod admin_cases;
mod apply_ledger;
mod claim_release;
mod closed_set;
mod command_sql;
mod confirm_loss;
mod constraints;
mod custody_proof;
mod cycle;
mod dispositions;
mod fixture;
mod generation_chain;
mod listing_edges;
mod loss_case;
mod loss_schema;
mod order_continuity;
mod relist;
mod reminder;
mod rights_schema;
pub(in crate::consignment) mod sale_recovery;
mod sanction_effect;
mod sanction_handoff;
mod unban;
pub(in crate::consignment) mod unpaid_recovery;
mod withdrawal_direct;

pub(super) use fixture::{
    CASE, CHALLENGE, FACT, PROOF, RIGHT, T0, T10, WORK, case_sql, challenge_notice_sql,
    challenge_sql, command_ids, digest, fact_sql, loss_chain_sql, proof_sql, right_sql, seed,
    seed_notice,
};
