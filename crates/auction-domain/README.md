# auction-domain

[文档导航](../../docs/README.md) · [业务规则](../../docs/business-rules.md) · [架构](../../docs/architecture.md)

纯领域模型：拍卖聚合、竞价与代理规划、结算计算、义务与资金规则、装备词典和跨层文本不变量。不做任何 I/O，不依赖数据库、HTTP、配置或时钟（时间由调用方传入），所有金额与版本运算溢出即失败。上层通过它获得“同样的输入一定得到同样的计划”的确定性，应用层负责把计划放进同一个数据库事务。

依赖只有 `chrono`、`serde`、`sha2`、`thiserror`、`uuid`，浏览器端不直接依赖本 crate。

## 模块地图

| 模块 | 内容 |
| --- | --- |
| [`crates/auction-domain/src/auction/`](src/auction) | `Auction` 聚合与 `AuctionStatus` 状态机 |
| [`crates/auction-domain/src/bidding/`](src/bidding) | `BidAmount`（金额网格）、`AuctionRules`（每场冻结的经济/时间规则）、`LotBidState`、`CompetitionPlan` 规划器与触发器 |
| [`crates/auction-domain/src/proxy/`](src/proxy) | 代理授权快照、追加式转换事实与生命周期 |
| [`crates/auction-domain/src/settlement/`](src/settlement) | 结算策略、持有时长曲线、`SettlementPlan`、义务闭集、恢复策略、资金分配与 Bot 内部履约选择 |
| [`crates/auction-domain/src/equipment/`](src/equipment) | 只在可信调用链短暂存在、自动清零的 `EquipmentKey`（固定十字节内联保存，清除后不可读取）；[`crates/auction-domain/src/equipment/taxonomy/`](src/equipment/taxonomy) 是装备名称、种类与词缀的唯一词典 |
| [`crates/auction-domain/src/identity/`](src/identity) | 各类强类型 UUID 标识与确定性派生 |
| [`crates/auction-domain/src/watch/`](src/watch) | 装备标记的提醒渠道（MM/PM）与连续领先周期 |
| [`crates/auction-domain/src/text/`](src/text) | 规范单行文本与正十进制字符串谓词，全仓复用 |
| [`crates/auction-domain/src/stable_name/`](src/stable_name) | 闭集枚举的稳定持久化名称 |

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
- **冻结版本**：新场次使用 `AuctionRules.bidding_version=2`，`finalize_plan` 对每笔待追加事实重新计算最低价；低于完整最低价的代理事实必须恰好等于锁内授权有效上限，并严格高于当前价。上限只放在临时草稿中核验，不进入公开事实或命令结果。v1 保留原计算，历史计划的 `validate` 与幂等重放不重算金额。数据库快照版本映射见[数据库契约](../../docs/contracts/database.md#拍卖与竞价)。
- **软截止**：一次被接受的出价若发生在 `effective_end_at` 前 `soft_close_window`（默认 15 分钟）内，候选截止为“接受时刻 + `soft_close_extension`（默认 15 分钟）”，只在晚于当前截止时延长。窗口为 0 时不延长。截止之后的出价被拒绝。
- 领先者不能对自己加价；已有有效代理的用户不能再普通出价。

## 代理竞价

- 代理授权含上限金额与模式：v2 的 `Fast` 立即按完整最低价与有效上限中的较小值反超；`Slow` 首次被超越后安排一次 `proxy_slow_interval`（默认 10 分钟）到期反超，后续出价不延后该调度，执行时重算最低价并截到上限。进入 Final 后不能新建 Slow 代理。
- v2 中只有上限出价仍无法严格反超时，授权才耗尽（`Exhausted` / `StrictlyDefeated`），清除慢速调度；普通出价仍成功，Slow 到期或 Final 路径只追加耗尽转换，不追加出价、不触发软截止。新建代理上限仍须达到完整最低价。竞争按上限及授权先后选择胜者，具体价格与既有 v1 例外统一见[业务规则](../../docs/business-rules.md#代理竞价)。
- 拍卖进入 Final 阶段（Worker 在最早的 lot 有效截止前 `final_phase_lead`、默认 15 分钟时推进；Admin 手动推进时把截止设为当前时刻加该提前量）时，尚未到期且持有人资格有效的 Slow 代理转为 `FinalFast`，已有 Fast 保持原模式，失去资格的活动代理冻结。授权状态闭集包括 `Active`、`FinalFast`、`Exhausted`、`Frozen`、`Closed`。
- 同一用户在同一 lot 累计设置代理的次数（被超越后可重新设置）受 `proxy_max_outbid_uses` 限制（默认 3，0 为不限）；重新设置时新上限必须高于上一份。
- 一次命令产生的公开出价、代理转换、代理快照与最终 lot 投影构成一个 `CompetitionPlan`，必须原子写入。普通出价最多两条草稿（本人出价与代理反超），创建代理、Slow 到期与 Final 反超最多一条，均在栈上准备；最终公开事实仍由计划持有。

## 结算计算

每场拍卖冻结一份 `SettlementPolicy`：

| 模式 | 用途 |
| --- | --- |
| `HoldCurveV2`（版本 2，新拍卖默认） | 卖方按成交价全额收款；买方优惠全部由补偿池承担 |
| `HoldCurveV1`（版本 1） | 历史卖方让利算法，仅供既有冻结快照继续结算、读取与重放；新拍卖拒绝 |
| `LegacyZeroV0`（版本 0） | 无曲线、无补偿池的历史模式 |

计算步骤（`SettlementPlan::calculate`）：

1. `hold_ppm` = 获胜者累计领先时长除以“计划开拍 → 计划结束”时长（百万分比，封顶 1,000,000）。存储层从竞价事实累加至 lot 有效截止，包含延长期；领域层接收累计时长，不自行读取竞价历史。
2. 在锚点曲线上线性插值得到 `adjustment_ppm`（四舍五入远离零）。默认曲线从 0 领先的 +8% 下降到 25% 领先的 0，再到 50% 以上领先的 −2%。
3. 正值：买方额外支付 `floor(成交价 × ppm)` 的附加费，计入税费并注入补偿池。负值：形成同额“优惠需求”。
4. 补偿池可用额 = 期初余额 + 本场附加费；各 lot 的优惠按 `floor(可用额 × 本件需求 / 总需求)` 分配且不超过本件需求，尾差留池。买方实付 = 成交价 + 附加费 − 补偿；卖方始终收到成交价（v2）。
5. 期末池 = 可用额 − 已分配补偿；`validate` 复核单件金额、冻结模式与池守恒，应用层再按锁内 lot 事实校验集合、统计总额及义务。一次计算只完整校验一次冻结曲线，结果自检复用已校验策略及同一可用池余额。

流拍 lot 的明细金额全部为 0。

成交被恢复流程撤销时，`CompensationReversal::calculate`（规则 v1）先把补偿退回池中，再冲回 `min(附加费, 当前余额 + 补偿)` 的附加费；余额永不为负，未冲回部分只在余额耗尽时出现。冲正是新事实，不改写原明细或冻结策略；改变冲正结果必须新增规则版本。

## 结算义务

`SettlementObligationKind` 是后续履约的闭集：

| 义务 | 世界 | 含义 |
| --- | --- | --- |
| `WinnerEquipmentCod` | main | 把装备以 COD（货到付款）寄给买方，金额为买方实付 |
| `WinnerPaymentCarrierCod` | isekai | 在 main 寄 1 Credit 载体；COD 面额为买方货款 +1，净收款为买方货款 |
| `WinnerEquipmentHandoff` | isekai | 收款后把装备交给买方（以付款义务为前置） |
| `SellerPayout` | 两者 | 向卖方支付货款（以收款为前置） |
| `BuyerRefund` | 两者 | 需要时向买方退款（`Contingent`） |
| `UnsoldReturn` | 两者 | 流拍装备退回卖方 |
| `WithdrawnReturn` | 两者 | 撤拍装备退回卖方 |

义务状态 `Ready`/`Blocked`/`Contingent` 只表达交给投递前的业务状态；`validate` 按种类约束世界、收款人（用户或游戏身份二选一）、金额与前置关系。

恢复策略 `RecoveryPolicy`（`RemindV1`）按版本固定付款超时：v1 为 7 天，v2（赛季前场次）为 24 小时；均为 1 次提醒，COD 被退回的次数上限默认 3、可配 1–10。

领域层只定义冻结付款时长，不按当前赛季重写既有策略；时区排期、赛季前计划结束加 24 小时必须严格早于赛季开始日本地 00:00 的准入，以及实际付款起点与到期推进，由应用层负责。v2 的 24 小时是固定经过时长，不是当地日历日；`pre_season` 保留原模板的提醒参数与 COD 退回上限。

## 资金与内部履约

- `FundingAccountKey`、`FundingSplit` 与 `funding_reconciliation_capacity` 描述 Bot 账号真实现金的用途分配：冻结的经济金额不重算，余额观察不等于自有资金。
- `InternalSettlementAction` 只在买方或卖方是 Bot 自身时选择义务的执行方式（不经邮件外发），不重算金额、不新增责任。
- `equipment_loss_funding` 只接受 record_only 及显式外部买方补偿责任声明，验证冻结 B+C=S+T；不把人工责任变成 COD 实收，不重算卖家金额。

## 维护约定

- 新增闭集值时同步更新 `stable_name` 与持久化映射，并为历史快照保留读取路径。
- 任何改变既有计算结果的修改都必须以新的策略版本表达，不能改写旧版本语义——已结算记录需要能被逐字重放。
