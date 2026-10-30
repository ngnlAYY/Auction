# auction-domain

[文档导航](docs/README.md) · [业务规则](docs/business-rules.md) · [架构](docs/architecture.md)

纯领域模型：拍卖聚合、竞价与代理规划、结算计算、义务与资金规则、装备词典和跨层文本不变量。不做任何 I/O，不依赖数据库、HTTP、配置或时钟（时间由调用方传入），所有金额与版本运算溢出即失败。上层通过它获得“同样的输入一定得到同样的计划”的确定性，应用层负责把计划放进同一个数据库事务。

依赖只有 `chrono`、`serde`、`sha2`、`thiserror`、`uuid`，浏览器端不直接依赖本 crate。

## 模块地图

| 模块 | 内容 |
| --- | --- |
| [`crates/auction-domain/src/auction/`](crates/auction-domain/src/auction) | `Auction` 聚合与 `AuctionStatus` 状态机 |
| [`crates/auction-domain/src/bidding/`](crates/auction-domain/src/bidding) | `BidAmount`（金额网格）、`AuctionRules`（每场冻结的经济/时间规则）、`LotBidState`、`CompetitionPlan` 规划器与触发器 |
| [`crates/auction-domain/src/proxy/`](crates/auction-domain/src/proxy) | 代理授权快照、追加式转换事实与生命周期 |
| [`crates/auction-domain/src/settlement/`](crates/auction-domain/src/settlement) | 结算策略、持有时长曲线、`SettlementPlan`、义务闭集、恢复策略、资金分配与 Bot 内部履约选择 |
| [`crates/auction-domain/src/equipment/`](crates/auction-domain/src/equipment) | 只在可信调用链短暂存在、自动清零的 `EquipmentKey`；[`crates/auction-domain/src/equipment/taxonomy/`](crates/auction-domain/src/equipment/taxonomy) 是装备名称、种类与词缀的唯一词典 |
| [`crates/auction-domain/src/identity/`](crates/auction-domain/src/identity) | 各类强类型 UUID 标识与确定性派生 |
| [`crates/auction-domain/src/watch/`](crates/auction-domain/src/watch) | 装备标记的提醒渠道（MM/PM）与连续领先周期 |
| [`crates/auction-domain/src/text/`](crates/auction-domain/src/text) | 规范单行文本与正十进制字符串谓词，全仓复用 |
| [`crates/auction-domain/src/stable_name/`](crates/auction-domain/src/stable_name) | 闭集枚举的稳定持久化名称 |

## 拍卖生命周期

```text
Draft ──► Preview ──► Live ──► Final ──► Settling ──► Settled
  │          │
  └──────────┴──► Cancelled（只允许开拍前取消，须记录原因与操作者）
```

- `open` 把开拍与结束时间物化合并为一次版本推进：只能从 Preview 进入，`scheduled_end_at` 必须晚于开拍时刻，`effective_end_at` 初始等于计划结束。
- `request_early_settlement` 只关闭竞价入口（Live→Final 或 Final 内推进版本），真正的结算由 Worker 认领后推进。
- 每次状态变化先推进 `state_version` 再改字段，版本溢出时聚合保持不变。

## 竞价规则

- **金额网格**：外部出价与代理上限必须是 1000 Credits（`CREDIT_GRID`）的正整数倍，且不超过 10 亿（`MAX_BID_AMOUNT`）。历史持久化的非整千金额只能经 `from_persisted_positive` 进入。
- **最低加价**：无出价时最低为起拍价；之后按 `bid_increment_ppm`（默认 5%）计算：当前价先向上折算为千位单位，再对增量单位向上取整，增量至少 1000，最后加回原当前价（例：当前 1111 时下一口为 2111）。`bid_increment_ppm=0` 时使用 `minimum_increment`。
- **软截止**：一次被接受的出价若发生在 `effective_end_at` 前 `soft_close_window`（默认 15 分钟）内，候选截止为“接受时刻 + `soft_close_extension`（默认 15 分钟）”，只在晚于当前截止时延长。窗口为 0 时不延长。截止之后的出价被拒绝。
- 领先者不能对自己加价；已有有效代理的用户不能再普通出价。

## 代理竞价

- 代理授权含上限金额与模式：`Fast` 立即以 +1000 反超；`Slow` 在 `proxy_slow_interval`（默认 10 分钟）后到期再反超，进入 Final 阶段后不能新建 Slow 代理。
- 普通出价超过代理有效上限时代理耗尽（`Exhausted`）；恰好等于上限时代理以同价守位（同价事实只允许代理内部产生）；低于上限时 Fast 立即反超、Slow 安排一次到期。
- 拍卖进入 Final 阶段（Worker 在最早的 lot 有效截止前 `final_phase_lead`、默认 15 分钟时推进；Admin 手动推进时把截止设为当前时刻加该提前量）时，活动代理按规划转为 `FinalFast`。状态闭集：`Active → FinalFast/Exhausted/Frozen/Closed`，`FinalFast → Exhausted/Frozen/Closed`。
- 同一用户在同一 lot 累计设置代理的次数（被超越后可重新设置）受 `proxy_max_outbid_uses` 限制（默认 3，0 为不限）；重新设置时新上限必须高于上一份。
- 一次命令产生的公开出价、代理转换、代理快照与最终 lot 投影构成一个 `CompetitionPlan`，必须原子写入。

## 结算计算

每场拍卖冻结一份 `SettlementPolicy`：

| 模式 | 用途 |
| --- | --- |
| `HoldCurveV2`（版本 2，新拍卖默认） | 卖方按成交价全额收款；买方优惠全部由补偿池承担 |
| `HoldCurveV1`（版本 1） | 历史快照，仅供读取与重放，新拍卖拒绝 |
| `LegacyZeroV0`（版本 0） | 无曲线、无补偿池的历史模式 |

计算步骤（`SettlementPlan::calculate`）：

1. `hold_ppm` = 获胜者在“开拍 → 计划结束”时长中累计领先的比例（百万分比，封顶 1,000,000）。
2. 在锚点曲线上线性插值得到 `adjustment_ppm`（四舍五入远离零）。默认曲线从 0 领先的 +8% 下降到 25% 领先的 0，再到 50% 以上领先的 −2%。
3. 正值：买方额外支付 `floor(成交价 × ppm)` 的附加费，计入税费并注入补偿池。负值：形成同额“优惠需求”。
4. 补偿池可用额 = 期初余额 + 本场附加费；各 lot 的优惠按 `floor(可用额 × 本件需求 / 总需求)` 分配且不超过本件需求，尾差留池。买方实付 = 成交价 + 附加费 − 补偿；卖方始终收到成交价（v2）。
5. 期末池 = 可用额 − 已分配补偿；明细与池守恒由 `validate` 复核。

流拍 lot 的明细金额全部为 0。

## 结算义务

`SettlementObligationKind` 是后续履约的闭集：

| 义务 | 世界 | 含义 |
| --- | --- | --- |
| `WinnerEquipmentCod` | main | 把装备以 COD（货到付款）寄给买方，金额为买方实付 |
| `WinnerPaymentCarrierCod` | isekai | 寄 1 Credit 载体并以 COD 收取买方货款 |
| `WinnerEquipmentHandoff` | isekai | 收款后把装备交给买方（以付款义务为前置） |
| `SellerPayout` | 两者 | 向卖方支付货款（以收款为前置） |
| `BuyerRefund` | 两者 | 需要时向买方退款（`Contingent`） |
| `UnsoldReturn` | 两者 | 流拍装备退回卖方 |
| `WithdrawnReturn` | 两者 | 撤拍装备退回卖方 |

义务状态 `Ready`/`Blocked`/`Contingent` 只表达交给投递前的业务状态；`validate` 按种类约束世界、收款人（用户或游戏身份二选一）、金额与前置关系。

恢复策略 `RecoveryPolicy`（`RemindV1`）固定 7 天付款超时、1 次提醒，COD 被退回的次数上限默认 3、可配 1–10。

## 资金与内部履约

- `FundingAccountKey`、`FundingSplit` 与 `funding_reconciliation_capacity` 描述 Bot 账号真实现金的用途分配：冻结的经济金额不重算，余额观察不等于自有资金。
- `InternalSettlementAction` 只在买方或卖方是 Bot 自身时选择义务的执行方式（不经邮件外发），不重算金额、不新增责任。

## 维护约定

- 新增闭集值时同步更新 `stable_name` 与持久化映射，并为历史快照保留读取路径。
- 任何改变既有计算结果的修改都必须以新的策略版本表达，不能改写旧版本语义——已结算记录需要能被逐字重放。
