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

流拍 lot 的所有金额为 0。历史场次若冻结的是 `HoldCurveV1`（历史卖方让利算法，仅供既有快照继续结算、读取与重放）或 `LegacyZeroV0`（无曲线、无补偿池），始终按原策略计算，不补算差额。补偿池每个世界独立，公开站点“补偿池”页展示各场流水。

### 义务

| 义务 | 来源世界 | 执行方式 | 前置 |
| --- | --- | --- | --- |
| `WinnerEquipmentCod` | main | 在 main 把装备以 COD（金额 = 买方实付）寄给买方 | — |
| `WinnerPaymentCarrierCod` | isekai | 在 main 寄 1 Credit 载体，COD 面额 = 买方实付 +1 Credit，买方净支付仍为实付金额 | — |
| `WinnerEquipmentHandoff` | isekai | 在 isekai 把装备寄给买方 | 载体 COD 已收款 |
| `SellerPayout` | 两者 | 在 main 向卖方支付成交价 | main：买方已付款（装备 COD 已收款）；isekai：装备已交付（`WinnerEquipmentHandoff` 已满足） |
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
- **结果未知时不自动重发**：发送阶段的超时、崩溃或远端响应异常无法证明寄出结果时，投递转为 `manual_reconcile`；过期的 `sending` 租约也先转入该状态。邮箱维护循环会尝试用原发送尝试冻结的完整期望在 Sent 中精确找回邮件。找不到不代表未发送；Admin 可记录失败或明确授权重试，但主动 Send 不允许仅凭手填远端引用执行“确认成功”，成功须由匹配的 Sent 证据恢复。
- 新的可发送投递写入时触发 `pg_notify`，投递循环立即被唤醒。

## 4. 恢复（付款超时与拒收）

每个已售 lot 在结算时建立恢复记录，冻结恢复策略（`RemindV1`，版本决定付款期限）：

| 规则 | 默认 |
| --- | --- |
| 付款期限 | 从结算建立恢复记录起算：v1 为 7 天；赛季前场次冻结 v2，为 24 小时。main COD 收回重寄后按冻结版本重新起算 |
| 逾期提醒 | 1 次（邮件提醒买方） |
| COD 退回上限 `cod_return_limit` | 3（可配 1–10） |

- main 装备 COD 在付款前退回：第 1 至 `limit − 1` 次退回时，系统取得收回装备的动作证明后重新寄出原 COD（金额与装备不变）；第 `limit` 次退回时，为卖家建立撤拍退回义务，原 COD 与卖方货款按失败收口，并拉黑买家。每封退件只计数一次。
- isekai 载体 COD 退回时先收回载体；已经付款但交付装备被退回时先取得装备收回证明。Admin 分别通过 `cancelled` 或 `equipment_recovered` 确认后重列到该世界的草稿接收场次，后者还激活买方退款；没有草稿目标时保持 `relist_pending`。main 的 `reclaimed` 也只在对应收回证明与义务状态允许时可确认。
- 赛季换季限制（参与早鸟期的世界）：赛季开始前的新赛季草稿不接收重列，保持 `relist_pending` 并按持久化退避重试；原成交场次开拍后已经历赛季开始的旧赛季装备永不重列，记录阻断码 `relist_season_closed`（报表显示为 `recovery_relist_unavailable`），停在 `relist_pending` 且不再自动重试，由 Admin 人工处理。
- 恢复并非单一路径：未付时为 `awaiting_payment`，到期转 `payment_overdue` 并创建一次提醒；main 收款后可完成，isekai 收款后先进入 `delivering_equipment`，交付满足后完成。退件可转 `reclaiming`，经收回、确认后进入 `relist_pending`、`refunding` 或 `completed`。Admin 确认必须携带原因、稳定命令 ID 与当前状态版本，所有动作写入审计。

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
