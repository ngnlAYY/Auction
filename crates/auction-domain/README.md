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
| [src/equipment/taxonomy](crates/auction-domain/src/equipment/taxonomy) | 统一装备语义词典、来源修订、纯名称解析及词缀推荐关系 |
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

### 装备语义词典

`equipment::taxonomy` 是分类、品质、材质、部位及前后缀关系的唯一纯 Rust owner。词典以固定 Wiki 修订
`Equipment_Basics` 65026、`Detailed_Equip_Characteristics` 66094、`Obsolete_Items` 65259 为来源，
`TAXONOMY_REVISION` 标识本次审阅版本；每项携带 `Current`、`Legacy` 或 `BoundedGrammar` 状态与页面修订。
外层按稳定契约投影给 Admin，不在浏览器维护第二份规则。

确证清单包含七个规范大类、十个当前品质与历史 `Fine`/`Flimsy`、十二个当前护甲材质与历史 `Chainmail`，
以及 77 个完整种类（22 个武器/盾牌、50 个当前护甲、5 个历史 Chainmail）。整族标色的
`armor_material_category` 只识别原十二个当前材质，保留旧 scalar 精确匹配含义。Chain/Reactive 仅支持
精确材质 token 与 Heavy 八个公共部位 token 的有界兼容读取，不将这些组合列作已确证完整种类。

名称按“品质、可选前缀、装备种类、护甲部位、可选后缀”拆分，只有 Cloth、Light、Heavy 有部位词。
例如 `Legendary Radiant Phase Robe of Fenrir` 的种类为 `Phase`、部位词为 `Robe`、语义部位为 `Body`；
`Oak Staff`、`Force Shield` 的完整词组均属于种类，不拆出部位。类型不占名称字段，词典所属类型供详情 header
和 canonical JSON 交叉校验。Legendary 必须同时具有前缀和实义后缀，孤立 `of the` 视为缺少后缀；
Magnificent 及其他兼容品质保留可缺词缀的识别规则。

名称解析先按最长完整基础名匹配，保证 `Great Mace` 不被拆成自由前缀 `Great` 与种类 `Mace`，再分离护甲部位。
`EquipmentIdentity.equipment_type` 输出纯种类，`slot_name`、`slot` 分别保留名称部位词和语义部位；
`ResolvedKind.name` 继续保留完整基础名，由集成层映射到既有 canonical、快照和指纹字段。
自由前后缀保留原大小写与可选 `the`，只规范空白。未知种类返回安全闭集错误；
未知词缀继续保留，并区分推荐、历史和未分类状态。推荐关系保留 Wiki 总表与分项明确记载的兼容并集，
不以推荐列表限制历史装备或自由词缀。`suffix_matches` 统一标色与识别的可选 `the` 比较口径。

完整种类的名称匹配顺序仅初始化排序一次；同长度词条仍保留原词典顺序。名称解析中的词缀分类直接复用
已识别的种类与大类，借用静态推荐片段，不重新识别种类或分配推荐列表；对外候选 API 仍返回原顺序的 `Vec`。
字符串分类 API 只为确证完整种类、有限重甲语法或十二个可选材质建立上下文；未知种类的非空词缀统一标为
`Unclassified`，不再仅凭金属词条推定 `Legacy`。这些优化不改变名称字段、自由词缀文本、推荐内容或收件政策。

HTML 结构、等级与交易状态属于外部页面适配器；收件品质和交易资格属于应用准入，解析可识别历史品质或种类
不会修改这些政策。词典不读网络或配置，不回写既有快照，也不改变规范指纹的字段或编码。

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
