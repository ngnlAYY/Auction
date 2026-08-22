# auction-domain

领域层：实体、值对象、状态机和不变量。这是依赖方向的最内层。

- `src/auction/mod.rs`：`Auction` 聚合与 `AuctionStatus` 生命周期状态机，包含状态版本、结束时间
  和取消/软截止边界。
- `src/bidding/amount.rs`：带信用网格不变量的竞价金额值对象。
- `src/bidding/policy.rs`：旧论坛兼容的分段加价、代理反超、软截止和取消截止规则。
- `src/bidding/bid.rs`：不可变的公开竞价事实和产生机制。
- `src/bidding/competition.rs`：锁内纯竞价规划器，输出公开事实、代理转换和最终 lot 投影。
- `src/proxy/mod.rs`：Fast/Slow 代理授权状态与追加式转换事实。
- `src/identity/mod.rs`：带类型的 UUID 包装，避免不同标识符互相混用。
- `src/errors/mod.rs`：领域错误分类。
- `src/settlement/mod.rs`：结算策略、金额、明细和后续义务的值对象、曲线计算与闭集校验。

允许依赖：serde、UUID、时间类型、`thiserror`。

**不得导入** Axum、Dioxus、SQLx、reqwest、环境变量读取或日志初始化。领域层不应知道自己
通过什么协议交付或持久化。

修改竞价写入、状态迁移或结算逻辑时，必须在代码或变更说明中写明保护了哪些不变量，
清单见根 AGENTS.md 和 docs/contracts/invariants.md。
