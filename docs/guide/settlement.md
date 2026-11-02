# 结算与履约

[文档导航](../README.md) · [业务规则](../business-rules.md) · [邮件流程](mail-flows.md) · [auction-domain](../../crates/auction-domain/README.md)

一场拍卖从“所有 lot 截止”到“钱货两清”经过四个阶段：**结算计算**（一次事务算出所有金额与义务）→ **履约**（把义务变成邮件并取得证据）→ **投递**（outbox 发信）→ **恢复**（付款超时、拒收等异常）。每一步都持久化、可重放，外部结果不确定时停下转人工，绝不猜测。

## 1. 结算计算

Worker 认领处于 Final 且所有 lot 有效截止都已过去的场次，在一个事务内：关闭活动代理、计算 `SettlementPlan`、写入场次与 lot 结算结果、补偿池流水与全部义务，发布 `auction.settled` 事件。同一场次的结算命令 ID 稳定派生，Worker 重启后继续使用原命令，不会产生第二份结算。

### 金额（HoldCurveV2，新场次默认）

1. **领先比例** `hold_ppm`：获胜者累计领先时长除以“计划开拍 → 计划结束”时长（百万分比，最高 1,000,000）。累计领先从竞价事实计算到 lot 的有效截止，包含软截止延长期；分母仍使用计划时窗。
2. **调整比例** `adjustment_ppm`：在锚点曲线上线性插值。默认锚点：

   | 领先比例 | 0% | 5% | 10% | 15% | 20% | 25% | 30% | 40% | ≥50% |
   | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
   | 调整 | +8% | +6.4% | +4.8% | +3.2% | +1.6% | 0 | −0.4% | −1.2% | −2% |

3. 调整为正：买方额外支付附加费 `floor(成交价 × 调整比例)`，计入税费并注入补偿池。调整为负：形成同额“优惠需求”。
4. **补偿分配**：可用额 = 期初补偿池余额 + 本场附加费合计；每件的补偿 = `floor(可用额 × 本件需求 / 总需求)`，不超过本件需求，尾差留在池中。
5. 结果：**卖方所得 = 成交价**；**买方实付 = 成交价 + 附加费 − 补偿**；期末池 = 可用额 − 已分配补偿。恒等式 `买方实付 + 补偿 = 卖方所得 + 税费` 在每件上成立。

流拍 lot 的所有金额为 0。历史场次若冻结的是 `HoldCurveV1`（历史卖方让利算法，仅供既有快照继续结算、读取与重放）或 `LegacyZeroV0`（无曲线、无补偿池），始终按原策略计算，不补算差额。

### 补偿池

两个世界共用一个补偿池（`compensation_pool` 单行），以 main Credits 计：isekai 成交的买方通过 main 载体 COD 付款，卖方货款也在 main 支付，附加费与补偿进出的都是 main Credits。各世界结算在同一行上串行，余额只由结算与下述冲正改变。

迁移 000071 之前每个世界各有一个池（`settlement_pools`）。合并时两池余额相加成为全局池的起点，`state_version` 从两池最大版本 + 1 起算，并写入审计 `auction.compensation.merge`（含各世界合并前余额与版本）；`settlement_pools` 随后由触发器冻结，作为合并前的期末证据。因此合并前结算的批次，期初与期末是所在世界池的余额。池的范围不属于场次冻结策略：公式与锚点不变，期初余额本就是结算时在锁内读取的运行时输入，所以合并时已创建但未结算的场次直接使用全局池，合并前结算的成交若被撤销，也冲正到全局池。

### 成交撤销冲正（规则 v1）

结算按权责发生制把附加费计入补偿池、把补偿从池中支出。成交随后被撤销时，这两笔金额都不会兑现，必须冲回。恢复记录的 `resolution` 由空首次落定即视为成交撤销：

| resolution | 场景 |
| --- | --- |
| `returned_after_three_cod_returns` | main 装备 COD 达到退回上限，装备退回卖家 |
| `reclaimed` | main 收回装备后确认重列 |
| `cancelled` | isekai 载体 COD 未付款被退回，确认取消并重列 |
| `equipment_recovered` | isekai 已付款但交付被退回，收回装备并向买方全额退款 |

在 `resolution` 落定的同一 Serializable 事务内，锁定补偿池并按冻结的 lot 结算金额冲正：

1. 先把该件的补偿全额退回池中：`可用 = 当前余额 + 补偿`；
2. 再冲回附加费：`冲回 = min(附加费, 可用)`，`期末 = 可用 − 冲回`。余额永不为负；冲回不足的部分（`附加费 − 冲回`）只在余额耗尽时出现，记录在冲正事实中并公开展示，不向后续场次追讨。

冲正写入只追加的 `compensation_pool_reversals`（每个恢复记录、每个 lot 最多一条），同时推进池的 `state_version` 并写审计 `auction.compensation.reverse`。原 `lot_settlements`、`auction_settlements`、`compensation_ledger` 与冻结策略都不改写；这三张表与冲正表由触发器禁止 UPDATE/DELETE。附加费与补偿都为 0（含 `LegacyZeroV0`）时不写冲正。升级到迁移 000070 之前已经撤销的成交不补冲，公开页把它们标为“已撤销（未冲正）”。

### 公开透明

公开站点“补偿池”页（[路由契约](../contracts/routes.md#目录与内容)）展示：

- 补偿池的当前余额与更新时间；
- 各场批次：期初、税收、折扣需求、补偿支出、实际兑付率（补偿支出 ÷ 折扣需求）、期末与本场冲正净额（有冲正时显示）；
- 展开场次后的逐件明细：成交价、领先比例与时长、调整比例、税收、名义折扣需求、实得补偿、买家实付、卖家所得与冲正（含未冲回金额）。玩家可据此复算上面的公式。

`/api/v1/compensation-pool/{auction_id}/movements` 继续提供逐笔账本（含 `balance_after` 余额链），供第三方核对。

### 义务

| 义务 | 来源世界 | 执行方式 | 前置 |
| --- | --- | --- | --- |
| `WinnerEquipmentCod` | main | 在 main 把装备以 COD（金额 = 买方实付）寄给买方 | — |
| `WinnerPaymentCarrierCod` | isekai | 在 main 寄 1 Credit 载体，COD 面额 = 买方实付 +1 Credit，买方净支付仍为实付金额 | — |
| `WinnerEquipmentHandoff` | isekai | 在 isekai 把装备寄给买方 | 载体 COD 已收款 |
| `SellerPayout` | 两者 | 在 main 向卖方支付原冻结货款 | main：买方已付款（装备 COD 已收款），或通过下述装备损失人工处置承担付款责任；isekai：装备已交付（`WinnerEquipmentHandoff` 已满足） |
| `BuyerRefund` | 两者 | 需要时向买方退款（`Contingent`，默认不执行） | 恢复流程激活 |
| `UnsoldReturn` | 两者 | 流拍装备寄回卖方（在装备所在世界） | — |
| `WithdrawnReturn` | 两者 | 撤拍装备寄回卖方 | — |

义务业务状态：`ready`（可执行）、`blocked`（等待前置）、`contingent`（只有恢复流程激活后才执行）。

## 2. 履约

Worker 的“结算履约”循环领取可执行义务，按 `pending → prepared → enqueued → satisfied` 推进：

| 阶段 | 内容 |
| --- | --- |
| prepare | 校验收件人当前身份与资格、装备持有、金额与世界；冻结邮件意图（主题、正文、收件人、附件、COD），生成 outbox 投递（prepared） |
| activate | 激活投递，交给投递循环发送（enqueued） |
| reconcile | 读取 Bot 账号的 Sent 记录（必要时还有余额）取得证据：COD 类义务要求 Sent 中该邮件已被收取（collected）才算满足；普通寄送要求精确匹配的 Sent 记录 |

满足（`satisfied`）后解除依赖它的义务（例如 main 买方付款后、isekai 装备交付完成后，`SellerPayout` 变为 ready）。证据不一致或结果未知时转 `manual_reconcile`，在 Admin“结算报表”中处理。

结算相关邮件主题带义务 ID，例如 `auction equipment delivery [<义务 UUID>]`、`auction payment request [<义务 UUID>]`。买方拒收或 COD 过期被退回时，退件回到 Bot 收件箱，邮件处理器据主题识别为对应义务的退件证据（见[邮件流程](mail-flows.md#结算退件)）。

新建的 main 装备 COD 邮件正文包含价格明细（成交价、附加费或补偿、应付金额）；isekai 载体付款邮件展示含载体 Credit 的 COD 面额。各类邮件正文模板可在 `[worker.mooglemail.mail_templates]`（或 runtime 文件）中定制，已冻结的旧投递保留原载荷。

## 3. 投递（outbox）

所有出站邮件——结算、恢复动作、登录 key、投稿回执、超价提醒、运维告警——都先写入 `deliveries`，再由投递循环发送：

```text
prepared → pending → claimed → sending → succeeded
                                     ├─► failed
                                     └─► manual_reconcile（结果未知）
```

- 领取带租约与 fencing 版本；进入 `sending` 前写入发送尝试摘要，发送后把远端邮件 ID 绑定到投递（唯一）。
- 同类邮件按 `[worker.mooglemail.send_intervals]` 控制发送起点的最小间隔；同一账号的远端写入串行。
- **结果未知时不自动重发**：发送阶段的超时、崩溃或远端响应异常无法证明寄出结果时，投递转为 `manual_reconcile`；过期的 `sending` 租约也先转入该状态。邮箱维护循环会尝试用原发送尝试冻结的完整期望在 Sent 中精确找回邮件；Sent 列表时间只到分钟，开始围栏按原 `sending_at` 所在分钟的起点比较，该分钟内任何同内容、同凭据的其他未绑定尝试都使找回失败关闭。找不到不代表未发送；Admin 可记录失败或明确授权重试，但主动 Send 不允许仅凭手填远端引用执行“确认成功”，成功须由匹配的 Sent 证据恢复。
- **Sent 目击门**：结算发送停在 `manual_reconcile` 且冻结匹配未能找回时，邮箱维护在完整 Sent 列表中不限时间查找同一义务主题（主题带唯一 reference），读取详情确认后在投递上记录 `sent_reference_sighting`（MID 已属其他投递时不记录）。目击不是发送证明，只使 Admin 对该投递的重试与判失败返回 `delivery_sent_sighted`，防止重复付款或把已寄出误判为失败；应在游戏中核对后改用下文 Sent 人工证据确认。Admin 判失败的原因码为闭集 `verified_not_sent`、`recipient_invalid`、`no_longer_needed`，历史失败码保持原样。
- **修复后恢复投递**：先核对原发送尝试、归档响应及后续处置，不能只凭 `write_started=false` 或 Sent 未命中就批量重发。对仍为 `manual_reconcile` 且已明确授权重试的记录，Admin 对账使用最新 `fencing_version` 与稳定命令 ID 执行 `retry`；同一事务把原投递恢复为 `pending`，把关联失败/人工核对履约恢复为 `enqueued`，并恢复相关恢复流程投影，不另建义务或直接修改数据库。重试命令结果未知时原样重放。
- 新的可发送投递写入时触发 `pg_notify`，投递循环立即被唤醒。

## 4. 恢复（付款超时与拒收）

每个已售 lot 在结算时依据场次已冻结的恢复策略建立恢复记录（`RemindV1`，版本决定付款期限）；升级不改写既有场次或恢复记录：

| 规则 | 默认 |
| --- | --- |
| 付款期限 | 从结算建立恢复记录起算：v1 为 7 天；自动排期的赛季前场次冻结 v2，为 24 小时。main COD 收回重寄后按冻结版本重新起算 |
| 逾期提醒 | 1 次（邮件提醒买方） |
| COD 退回上限 `cod_return_limit` | 3（可配 1–10） |

付款期限到达只创建提醒并进入 `payment_overdue`，不会仅凭逾期自动取消成交、退回装备或拉黑买家。实际截止以恢复报告的 `payment_deadline_at` 为准；赛季前场次的配置时序校验使用计划结束时间，软截止或延迟结算后仍从实际建立恢复记录起算完整 24 小时。

- main 装备 COD 在付款前退回：第 1 至 `limit − 1` 次退回时，系统取得收回装备的动作证明后重新寄出原 COD（金额与装备不变）；第 `limit` 次退回时，为卖家建立撤拍退回义务，原 COD 与卖方货款按失败收口，并拉黑买家。每封退件只计数一次。
- isekai 载体 COD 退回时先收回载体；已经付款但交付装备被退回时先取得装备收回证明。Admin 分别通过 `cancelled` 或 `equipment_recovered` 确认后重列到该世界的草稿接收场次，后者还激活买方退款；没有草稿目标时保持 `relist_pending`。main 的 `reclaimed` 也只在对应收回证明与义务状态允许时可确认。
- 赛季换季限制（参与早鸟期的世界）：赛季开始前的新赛季草稿不接收重列，保持 `relist_pending` 并按持久化退避重试；原始投稿收件或成交场次开拍后已经历赛季开始的旧赛季装备永不重列，记录阻断码 `relist_season_closed`（报表显示为 `recovery_relist_unavailable`），停在 `relist_pending` 且不再自动重试，由 Admin 人工处理。判断沿用不可变的原始投稿收件事实，修改场次排期不会把旧装备变为新赛季资产；只使用已冻结且非 NULL 的赛季边界，不回填历史场次。
- 上述任一路径首次落定 `resolution`（成交撤销）时，同事务按[成交撤销冲正](#成交撤销冲正规则-v1)回滚补偿池。
- 恢复并非单一路径：未付时为 `awaiting_payment`，到期转 `payment_overdue` 并创建一次提醒；main 收款后可完成，isekai 收款后先进入 `delivering_equipment`，交付满足后完成。退件可转 `reclaiming`，经收回、确认后进入 `relist_pending`、`refunding` 或 `completed`。Admin 确认必须携带原因、稳定命令 ID 与当前状态版本，所有动作写入审计。

### Sent 人工证据确认

当原生 Sent 已证明邮件发出，但附件已领取、历史尝试关联不完整、原生分钟时间不足以通过自动恢复，或该投递已被 Admin 误判为失败时，运维可使用 `auction-cli settlement confirm-sent <DELIVERY_UUID> --evidence <JSON>` 只读预览；执行须提供稳定命令 ID、当前 fencing 版本、操作者、原因和逐字确认，并按预览要求显式接受已领取资产或旧尝试关联责任。完整证据字段和参数见 [auction-cli](../../apps/auction-cli/README.md#sent-人工证据确认)。命令只读本地证据与数据库，不发起游戏请求，也不重发邮件。

此入口限定 main 结算的单件装备 COD、流拍退回、撤拍退回，以及已有真实收款分配的卖方货款。应用端口匹配原冻结正文、身份、原生发送分钟、金额和附件；装备未领取时资产必须精确匹配，已领取时要求全零当前资产和匹配冻结名称/COD 的原生回执，由操作者显式承担无法再次观察原 EID 的责任。现金接受完整正额 Credits、零 COD 与无装备：未领取时原额仍在、回执为空且正文未变；已领取时剩余为零并带原生 `Removed attachment: <千分位金额> Credits` 回执，须显式接受已领取责任；必须与原冻结卖方金额、原收件人及当前 UID 的原 lot/义务/投递 `seller` 分配完全一致，分配须为 `unknown`，lot 来源须为 `collected` 且用途仍为 `seller`，原预留命令与资金分录必须存在且匹配。历史尝试不删除或改写，旧尝试关联的人工接受独立记录。被 Admin 对账为失败、且其后再无变化的投递也可用同一入口更正（预览 `corrects_admin_failure=true`）：Sent 分钟须在投递激活后、失败结论前，早于当前尝试时须接受旧尝试关联；原失败命令与审计保留，履约从 failed 恢复正常对账。活跃领取、退休损失投递、已有成功、已返还资金或付款提醒以外的恢复动作均拒绝。

提交追加不可变人工确认，使用独立 `sent_operator_confirmed_v1`，不冒充 `sent_exact_payload_v1` 或设置自动 `sent_verified`。持久证明必须与冻结尝试、MID 绑定及当前投递一致，单独 metadata 标签不能赋予证明能力。Admin 仅填写远端引用仍不能确认主动 Send 成功。原义务与结算金额保持：装备确认后原履约恢复，由 Worker 继续取得真实收取证明、经资金 owner 记账并放行原冻结卖方货款；现金确认则与自动恢复共用资金 owner，在同一事务把原未知支出分配变为 `spent` 并追加一次原投递的支出分录，再恢复正常履约对账，不创建新付款、资金来源或收款凭据。人工确认不证明收件人已领取；现金金额仍完整附在原已发邮件中。

预览、提交和重放分别输出 `preview`、`committed`、`replayed`。结果未知时保持原命令 ID、证据和全部参数重放核对，不能换 ID 或混入最新预览版本。

### 装备损失人工处置

main 已结算成交的装备在实际寄出前损失时，运维可通过 `auction-cli settlement equipment-loss <LOT_UUID>` 只读预览，再以 `--execute`、稳定命令 ID、操作者、原因、资金账户、三项预期版本、冻结卖方金额与逐字确认记录人工处置。操作步骤与完整参数见 [auction-cli](../../apps/auction-cli/README.md#装备损失人工收口)。资格由结算 owner 在同一 Serializable 事务内校验：原装备 COD 必须停在库存读取失败的人工核对状态，全部尝试都明确未开始外写，且没有既有收款、活跃领取或冲突恢复/资产操作。Sent 中未找到邮件或单条 `write_started=false` 不能替代这些完整约束。

操作者明确声明装备已损失，并承担买方线下补偿责任；这项声明不证明补偿已支付。系统追加独立人工处置事实，以 `operator_equipment_loss_v1` 证明收口原付款履约，不冒充 Sent 邮件或真实 COD 收款。此入口只支持已就绪的 `record_only` 资金账户，拒绝 `strict_cash`；操作者承担的买方金额与既有补偿按原冻结结算分配，不记为真实 `collected_payment`，不改写原结算金额、策略或补偿流水。

命令在同一事务内终结原投递为 `failed/equipment_destroyed`、收口买方退款执行责任与恢复记录，并解除原 `SellerPayout` 的阻塞。恢复 `stage=completed`、`resolution=NULL`：不再催款、退款或重列，不拉黑买家，也不触发成交撤销冲正。原损失投递永久退休，Admin 对账/重试不能将它复活。卖方仍按原冻结金额通过正常履约工作项与真实邮件支付，人工处置成功并不表示卖方已经收款。

默认 JSON 的 `outcome=preview` 只是只读投影；实际提交返回 `committed`，原 ID 同参数重放返回 `replayed`。结果未知时保持命令 ID、原因、账户、版本和金额等全部原参数重放，由命令账本判定；不能换 ID 或将新预览的版本混入原请求。

## Bot 内部履约与资金账户

当义务的收件人就是 Bot 自己（例如 Bot 账号作为卖家寄售、或作为买家竞得）时，给自己寄信没有意义，也无法从 Sent 取得收款证据。此时履约走**内部分支**：在一个事务内直接满足原义务，同时记录资金用途、审计与依赖解除，不产生 outbox 投递。

| 原义务 | 内部动作 | 需要装备在库 | 需要资金账户 |
| --- | --- | --- | --- |
| `UnsoldReturn`、`WithdrawnReturn` | 直接留存（KeepUnsold） | 是 | 否 |
| `WinnerEquipmentCod`、`WinnerPaymentCarrierCod` | 购买（Purchase） | 是 | 是 |
| `WinnerEquipmentHandoff` | 交接（Handoff） | 是 | 否 |
| `SellerPayout` | 卖方货款留存（RetainSeller） | 否 | 是 |
| `BuyerRefund` | 退款（Refund） | 是 | 是 |

内部身份分支使用当前远端账号 UID、玩家名称与联合证据，并核对配置的 Bot 名称；配置名称不能替代最终的同账号证明。联合证据需要 MoogleMail 与论坛目标资料读取能力，当前 Worker 仅在启用论坛时装配后者。内部动作只选择原义务的执行方式，不重算冻结金额，也不创建新的责任；装备持有在付款前与 isekai 交接时各自重新证明。

### 资金账户

需要资金的内部动作要求全局存在一个 Bot 资金账户（固定于 Main 世界的 Bot UID，Credits 计）。账户有两种模式，建立后不能切换：

| 模式 | 含义 | 建立方式 |
| --- | --- | --- |
| `record_only`（只记账） | 记录结算责任与真实收款，不读取 Bot 余额，不以 Bot 现金限制个人消费 | `auction-cli funds open-record-only` |
| `strict_cash`（真实现金） | 每次支出前观察 Bot 余额，只能动用对账时授权的自有额度与补偿额度；出现账实缺口时新增支出冻结 | `auction-cli funds reconcile` 对账（需专用账号声明） |

账户未建立、未对账、已冻结或资金不足时，相关内部履约保持等待，不会退化为外部邮件。

建立与对账步骤（只读 `preview` 不访问远端；执行开户或对账写入前必须停止 Worker，写入命令会取得与 Worker 相同的邮件/论坛凭据所有权）：

1. `auction-cli funds preview --uid <UID>`：只读查看账户状态、责任投影、阻断项、`preview_digest` 与确认短语模板。
2. 选择模式：
   - 只记账：`funds open-record-only`，携带 `--expected-version 0`、`--preview-digest` 与 preview 给出的确认短语。
   - 真实现金：`funds reconcile`，声明 `--proprietary-credits`、`--compensation-credits`、`--dedicated-account` 与确认短语。
3. 输出 `committed` / `replayed` 才算完成；`outcome_unknown` 时用同一命令 ID 与参数重新执行以核对，不要换 ID。

参数细节见 [auction-cli](../../apps/auction-cli/README.md#bot-资金)。

## 运维入口

| 位置 | 用途 |
| --- | --- |
| Admin “结算报表” | 按世界与期号查看结算报告（金额、义务、履约证据）与恢复报告，确认恢复动作 |
| Admin “投递队列” | 释放、重试、人工对账投递 |
| Admin 仪表盘 | 履约、投递、恢复积压与告警 |
| `auction-cli mail inspect-sent` | 只读核对撤拍退回邮件在 Sent 中的状态 |
| `auction-cli funds …` | Bot 资金账户 |
| `auction-cli settlement equipment-loss` | 装备实际寄出前损失的只读预览与受审计人工处置 |
| `auction-cli settlement confirm-sent` | 原生 Sent 人工证据的只读预览与受审计确认 |
