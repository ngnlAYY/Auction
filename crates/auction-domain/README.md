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
| [src/auction](src/auction) | 聚合与生命周期 |
| [src/bidding](src/bidding) | 金额、竞价政策和锁内纯计划 |
| [src/proxy](src/proxy) | 代理授权及追加转换事实 |
| [src/settlement](src/settlement) | 策略、金额守恒、结算计划与义务 |
| [src/equipment](src/equipment) | 装备标识与分类 |
| [src/identity](src/identity) | 带类型的身份 |

## 职责与实现边界

领域层：实体、值对象、状态机和不变量。这是依赖方向的最内层。

## 模型与不变量

- [src/auction/mod.rs](src/auction/mod.rs)：`Auction` 聚合与 `AuctionStatus` 生命周期状态机，包含状态版本、结束时间
  和取消/软截止边界。
- [src/bidding/amount.rs](src/bidding/amount.rs)：正竞价金额值对象；外部命令构造保持信用网格，持久化防腐入口保留
  旧版任意正起拍价、当前价和自动代理事实。新普通出价、新建/降低代理上限及新自动竞价事实
  最高为 `MAX_BID_AMOUNT = 1_000_000_000`（1B）；外部超额输入直接拒绝，历史读取、幂等重放和
  结算总额不截断。历史代理后续自动争价的有效上限取原授权与 1B 的较小值。
- [src/bidding/policy.rs](src/bidding/policy.rs)：旧论坛兼容的分段加价、代理反超、软截止和取消截止规则。
- [src/bidding/bid.rs](src/bidding/bid.rs)：不可变的公开竞价事实和产生机制。
- [src/bidding/competition/](src/bidding/competition/)：锁内纯竞价 owner；`trigger.rs` 维护触发器模型，`plan/` 按
  `mod.rs` 规划入口、`validation.rs` 结果/输入校验、`ordinary.rs` 普通竞价、`proxy.rs`
  代理生命周期和 `state.rs` 软截止/状态收口拆分职责，`competition/mod.rs` 只负责稳定导出。
- [src/proxy/](src/proxy/)：`model.rs` 定义 Fast/Slow 代理授权和转换事实，`authorization.rs` 负责授权
  创建/状态变更，`transition.rs` 负责追加式转换事实校验；`mod.rs` 只保留稳定导出。
  降额允许 `Active → Active` 和 `FinalFast → FinalFast`；后者保持 Fast 模式，所有降额都保持
  状态与模式不变、上限严格下降、版本推进及空终止原因。
- [src/identity/mod.rs](src/identity/mod.rs)：带类型的 UUID 包装，避免不同标识符互相混用。
- [src/errors/mod.rs](src/errors/mod.rs)：领域错误分类。
- [src/settlement/](src/settlement/)：结算领域 owner；`policy.rs` 维护策略/恢复参数，`amount.rs` 维护非负金额
  值对象，`facts.rs` 维护 lot 事实与结算明细，`obligations.rs` 维护后续业务义务，`plan.rs`
  维护结算曲线计算与最终计划；其中 `WinnerPaymentCarrierCod` 的义务路由固定为 `main`；
  `SettlementLine` 在存储/重放边界同时校验金额守恒、税与补偿互斥、税时卖方金额等于成交价，
  以及补偿时卖方金额严格低于成交价；`mod.rs` 只负责稳定导出。

## 依赖约束


允许依赖：serde、UUID、时间类型、`thiserror`。

**不得导入** Axum、Dioxus、SQLx、reqwest、环境变量读取或日志初始化。领域层不应知道自己
通过什么协议交付或持久化。

修改竞价写入、状态迁移或结算逻辑时，必须在代码或变更说明中写明保护了哪些不变量，
清单见根 AGENTS.md 和 docs/contracts/invariants.md。

## 新模板与历史策略


新拍卖模板通过 `SettlementPolicy::validate_for_new_auction` 要求折扣严格小于 50%
（最小 `adjustment_ppm` 为 -499999），保证足额补偿后 COD 付款仍为正。历史策略仍使用
原结构校验，已结算金额和重放不因模板限制改变；已有冻结曲线若产生非正付款，仍由原公式与义务校验失败关闭，
不得静默调整经济快照。

## 维护与验证

修改本模块时同步对应[契约](../../docs/contracts/README.md)与源码入口；跨模块理由按[决策记录规则](../../docs/documentation.md)维护。

```bash
cargo check --locked -p auction-domain
```

该命令只做静态编译；完整门禁见[开发指南](../../docs/development.md)，运行证据边界见[验证状态](../../docs/status.md)。
