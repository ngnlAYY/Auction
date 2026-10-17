# auction-domain

[文档导航](docs/README.md) · [架构](docs/design/architecture.md) · [契约](docs/contracts/README.md)

纯领域实体、值对象、状态机和不变量，是依赖方向最内层：本 crate 只计算和校验领域结果，
用例编排归应用层，持锁与原子写入归存储层；`lib.rs` 只声明模块和稳定导出，不拥有 I/O 或进程生命周期。

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/auction](crates/auction-domain/src/auction) | 拍卖聚合、生命周期与状态版本 |
| [src/bidding](crates/auction-domain/src/bidding) | 金额、冻结政策与纯竞价计划 |
| [src/proxy](crates/auction-domain/src/proxy) | Fast/Slow 授权与追加转换事实 |
| [src/watch](crates/auction-domain/src/watch) | 逐条竞价的连续领先周期；账号 MM/PM 渠道闭集，旧标记 MM 占位独立校验 |
| [src/settlement](crates/auction-domain/src/settlement) | 结算策略、金额守恒与履约义务 |
| [src/equipment](crates/auction-domain/src/equipment) | 装备标识与分类 |
| [src/identity](crates/auction-domain/src/identity)、[src/errors](crates/auction-domain/src/errors) | 类型化 UUID 与领域错误 |

## 职责与实现边界

状态、金额、历史兼容和原子写入的完整要求见[领域不变量](docs/contracts/invariants.md)。本 crate 不得导入 Axum、
Dioxus、SQLx、reqwest、环境读取或日志初始化，依赖限于领域安全的值类型、序列化、哈希与错误库（见
[Cargo.toml](crates/auction-domain/Cargo.toml)）；修改状态机或计算规则时必须说明所保护的不变量。

## 模型与不变量

- `Auction` 与代理授权先完成状态、版本和快照校验再修改内存，失败不得留下部分变更。
- `bidding/competition/plan/` 统一普通竞价、Fast、SlowDue 和 Final 计划，并校验接受事实、最终领先者及代理转换链。
- 新出价、新代理上限和新自动竞价事实最高为 `MAX_BID_AMOUNT = 1_000_000_000`；历史金额读取、已接受命令重放及
  结算总额不截断，历史代理后续争价仍受 1B 约束。
- 代理降额只能降低上限并推进版本，保持原模式和状态；`FinalFast → FinalFast` 不得退回普通活动状态。
- 结算明细校验金额守恒与税/补偿互斥；`WinnerPaymentCarrierCod` 的派发世界固定为 `main`。

### 真实现金用途规则

[src/settlement/funding](crates/auction-domain/src/settlement/funding) 校验稳定认证 UID/Main/Credits 账户键、
冻结 `B+C=S+T`，以及真实现金扣除已有分配、旧责任隔离和明确用途授权后的容量；不把补偿池或一次 Balance 当自有金额。

### 内部履约动作

[src/settlement/internal](crates/auction-domain/src/settlement/internal) 只从原义务选择留存、付款、交接、卖方留存
或合法退款动作，不改冻结 `B+C=S+T`、原义务 ID 或 payment→handoff→seller 顺序，Isekai 内部 carrier 不产生 1C 收支；
WithdrawnReturn 没有内部授权，库存、来源和现金用途由外层证明与事务门禁决定。

## 新模板与历史策略

`AuctionRules` 新模板软截止窗口和延长统一为 900 秒：命中窗口的有效出价以接受时刻加 900 秒作为候选截止并与已有
截止取较晚者，资格、金额、最终阶段和代理规则不变，不使用分段 15/5/1；既有场次沿用冻结策略，未发布旧草稿只能
通过[显式修正命令](apps/auctionctl/README.md#草稿软截止显式修正)处理。`SettlementPolicy::validate_for_new_auction`
要求新拍卖折扣严格小于 50%（`adjustment_ppm >= -499999`），历史读取继续按冻结策略校验，非法结果失败关闭。

## 维护与验证

```bash
cargo check --locked -p auction-domain
```

仅静态编译；默认检查与禁止默认运行的测试范围见[开发指南](docs/guide/development.md)，运行证据见[验证状态](docs/status.md)。
