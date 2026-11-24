//! 使用真实 PostgreSQL 18 检查寄售资格、时间窗口、碰撞与历史证据。

mod collision;
mod fixtures;
mod history;
mod intake;
mod plan;
mod relist;
mod visibility;

mod production {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/database/consignment/eligibility/postgres_bridge.inc"
    ));
}
