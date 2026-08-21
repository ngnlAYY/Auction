# auction-domain

领域层：实体、值对象、状态机和不变量。这是依赖方向的最内层。

- `auction.rs`：`Auction` 聚合与 `AuctionStatus` 生命周期状态机。当前只覆盖初始状态迁移，
  不代表完整竞价行为。
- `ids.rs`：带类型的 UUID 包装，避免不同标识符互相混用。
- `error.rs`：领域错误分类。

允许依赖：serde、UUID、时间类型、`thiserror`。

**不得导入** Axum、Dioxus、SQLx、reqwest、环境变量读取或日志初始化。领域层不应知道自己
通过什么协议交付或持久化。

修改竞价写入、状态迁移或结算逻辑时，必须在代码或变更说明中写明保护了哪些不变量，
清单见根 `CLAUDE.md` 的「领域不变量」。
