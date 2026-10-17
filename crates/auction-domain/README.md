# auction-domain

[文档导航](../../docs/README.md) · [架构](../../docs/architecture.md) · [契约](../../docs/contracts/README.md)

纯领域实体、值对象、状态机和不变量，是依赖方向最内层。

## 本页导航

- [源码入口](#源码入口)
- [职责与实现边界](#职责与实现边界)
- [模型与不变量](#模型与不变量)
- [依赖约束](#依赖约束)
- [新模板与历史策略](#新模板与历史策略)
- [维护与验证](#维护与验证)

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/auction](src/auction) | 拍卖聚合、生命周期与状态版本 |
| [src/bidding](src/bidding) | 金额、冻结政策与纯竞价计划 |
| [src/proxy](src/proxy) | Fast/Slow 授权与追加转换事实 |
| [src/watch](src/watch) | 逐条竞价的连续领先周期；账号 MM/PM 渠道闭集，旧标记 MM 占位独立校验 |
| [src/settlement](src/settlement) | 结算策略、金额守恒与履约义务 |
| [src/equipment](src/equipment) | 装备标识与分类 |
| [src/identity](src/identity)、[src/errors](src/errors) | 类型化 UUID 与领域错误 |

## 职责与实现边界

本 crate 计算和校验领域结果；应用层编排用例，存储层持锁并原子写入结果。
`lib.rs` 只声明模块和稳定导出，不拥有 I/O 或进程生命周期。

## 模型与不变量

- `Auction` 与代理授权先完成状态、版本和快照校验，再修改内存；失败不得留下部分变更。
- `bidding/competition/plan/` 统一普通竞价、Fast、SlowDue 和 Final 计划，并校验接受事实、最终领先者及代理转换链。
- 新出价、新代理上限和新自动竞价事实最高为 `MAX_BID_AMOUNT = 1_000_000_000`；历史金额读取、已接受命令重放及结算总额不截断。历史代理后续争价的有效上限仍受 1B 约束。
- 代理降额只能降低上限并推进版本，保持原模式和状态；`FinalFast → FinalFast` 不得退回普通活动状态。
- 结算明细校验金额守恒与税/补偿互斥；`WinnerPaymentCarrierCod` 的派发世界固定为 `main`。

完整状态、金额、历史兼容和原子写入要求见[领域不变量](../../docs/contracts/invariants.md)。

## 依赖约束

依赖限于领域安全的值类型、序列化、哈希和错误库，实际清单见 [Cargo.toml](Cargo.toml)。
不得导入 Axum、Dioxus、SQLx、reqwest、环境读取或日志初始化。修改状态机或计算规则时明确说明所保护的不变量。

## 新模板与历史策略

`SettlementPolicy::validate_for_new_auction` 要求新拍卖折扣严格小于 50%
（`adjustment_ppm >= -499999`），保证足额补偿后的 COD 付款仍为正；后继草稿同样适用。
历史读取继续按冻结策略校验，不能静默修正旧曲线或重算已结算金额；非法结果失败关闭。

## 维护与验证

```bash
cargo check --locked -p auction-domain
```

仅静态编译；默认检查与禁止默认运行的测试范围见[开发指南](../../docs/development.md)，
运行证据见[验证状态](../../docs/status.md)。修改公共行为时同步对应契约。
