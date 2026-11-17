# auction-domain

[docs/README.md](../../docs/README.md) · [docs/business-rules.md](../../docs/business-rules.md) · [docs/architecture.md](../../docs/architecture.md)

纯领域模型：拍卖聚合、竞价与代理规划、结算计算、义务与资金规则、装备词典与跨层文本不变量。不做任何 I/O，不依赖数据库、HTTP、配置或时钟（时间由调用方传入），所有金额与版本运算溢出即失败。依赖只有 `auction-time`、`jiff`（内嵌 IANA 时区数据）、`serde`、`sha2`、`thiserror`、`uuid`，浏览器端不直接依赖本 crate。

## 模块地图

| 模块 | 内容 |
| --- | --- |
| [crates/auction-domain/src/auction](src/auction) | `Auction` 聚合与 `AuctionStatus` 状态机 |
| [crates/auction-domain/src/bidding](src/bidding) | `BidAmount`（金额网格）、`AuctionRules`（每场冻结规则）、`LotBidState`、`CompetitionPlan` 规划器 |
| [crates/auction-domain/src/proxy](src/proxy) | 代理授权快照、追加式转换事实与生命周期 |
| [crates/auction-domain/src/settlement](src/settlement) | 结算策略、持有时长曲线、`SettlementPlan`、义务闭集、恢复策略、人工 Sent/装备损失准入 |
| [crates/auction-domain/src/scheduling](src/scheduling) | 排期纯规则：IANA 时区与 DST 归一化、周场次、赛季早鸟窗口 |
| [crates/auction-domain/src/equipment](src/equipment) | 自动清零的 `EquipmentKey`；[crates/auction-domain/src/equipment/taxonomy](src/equipment/taxonomy) 是装备名称与词缀的唯一词典 |
| [crates/auction-domain/src/identity](src/identity) · [watch](src/watch) · [delivery](src/delivery) | 强类型标识与确定性派生；提醒渠道与连续领先周期；投递处置闭集与复核码 |
| [crates/auction-domain/src/keys](src/keys) · [errors](src/errors) · [text](src/text) · [stable_name](src/stable_name) | 父密钥用途闭集；`DomainError` 闭集；全仓复用的文本谓词；闭集枚举的稳定持久化名称 |

## 核心规则

**拍卖生命周期**：`Draft → Preview → Live → Final → Settling → Settled`，开拍前可 `Cancelled`（须记录原因与操作者）。`Auction` 聚合只提供到 `Final` 为止的迁移；`Settling`/`Settled` 由结算 Worker 在存储事务的状态与版本守卫下推进。`open` 把开拍与结束时间物化合并为一次版本推进；`request_early_settlement` 只关闭竞价入口，真正结算由 Worker 认领后推进。每次状态变化先推进 `state_version` 再改字段，版本溢出时聚合保持不变。详见[docs/business-rules.md](../../docs/business-rules.md)。

**竞价规则**：`AuctionRules` 冻结每场的竞价版本、起拍价、最低加价、软截止、取消截止、Final 阶段提前量与代理参数（金额网格 `CREDIT_GRID` 为全局常量），`CompetitionPlan::plan` 在锁内快照上一次算出命令的全部公开事实与代理转换。`soft_close_window` 只判定出价是否落入截止窗口，延长后的候选截止为接受时刻加 `soft_close_extension`，只延后不提前。v2 对每笔事实重算最低价，v1 保留原计算、历史计划不重算；最低价与 lot 材质无关。金额网格、最低价公式与出价资格见[docs/business-rules.md#竞价](../../docs/business-rules.md#竞价)。

**代理竞价**：创建、降额、慢速到期、进入 Final 的转换，代理动作前发现持有人失格时的冻结，以及结算时的关闭，都经同一规划器计算，输出只追加的转换事实与更新后的授权快照；拉黑用户时则由存储事务直接冻结其生效代理，不经规划器。规则测试见 [crates/auction-domain/tests/proxy_rules.rs](tests/proxy_rules.rs)；价格、排名、降额下限、失格冻结与 v1 例外见[docs/business-rules.md#代理竞价](../../docs/business-rules.md#代理竞价)。

**结算计算**：每场冻结一份策略——`HoldCurveV2`（版本 2，新拍卖默认：卖方全额收款，买方优惠由补偿池承担）、`HoldCurveV1`（仅历史快照）、`LegacyZeroV0`（无曲线历史模式）。`SettlementPlan::calculate` 按领先时长计算 `hold_ppm`、在锚点曲线插值 `adjustment_ppm`、按需求比例分配补偿池并复核对账；流拍 lot 明细全为 0。成交被撤销时 `CompensationReversal` 先退补偿再冲回附加费，余额永不为负，冲正是新事实。

**结算义务**：`SettlementObligationKind` 闭集——`WinnerEquipmentCod`（main，COD 寄装备）、`WinnerPaymentCarrierCod`（isekai，1 Credit 载体）、`WinnerEquipmentHandoff`、`SellerPayout`、`BuyerRefund`、`UnsoldReturn`、`WithdrawnReturn`。状态 `Ready`/`Blocked`/`Contingent` 只表达投递前的业务状态；`validate` 按种类约束世界、收款人、金额与前置关系。恢复策略 `RemindV1` 冻结付款超时（v1 7 天、v2 赛季前场次 24 小时）与 COD 退回上限。

**资金与内部履约**：`FundingAccountKey`/`FundingSplit` 描述 Bot 真实现金的用途分配，冻结金额不重算；`InternalSettlementAction` 只在买方或卖方为 Bot 时选择不经邮件外发的执行方式；`equipment_loss_funding` 只接受 record_only 与显式外部买方补偿责任，不把人工责任变成 COD 实收。

## 维护约定

- 新增闭集值时同步更新 `stable_name` 与持久化映射，并为历史快照保留读取路径。
- 任何改变既有计算结果的修改都必须以新的策略版本表达，不能改写旧版本语义——已结算记录需要能被逐字重放。

## 数据密钥域

`DataKeyDomain` 是父密钥用途闭集（`equipment`、`user_login`、`credential`、`mail_diagnostics`），字符串同时用于敏感 TOML 前缀、CLI `--domain` 与 MoogleMail 诊断归档头部；新增域必须同步扩展配置装配与归档格式校验。
