# auction-integrations

[文档导航](docs/README.md) · [架构](docs/design/architecture.md) · [契约](docs/contracts/README.md)

E-Hentai、MoogleMail、装备来源及密钥信封的外部适配器。实现应用层外部能力端口，只返回校验过的事实或安全错误：
外部响应必须经过应用命令或持久化 Worker 操作才能改状态，本地提交后的外部失败不能撤销已接受竞价；
错误、metadata 与普通投影不得包含 URL 中的 secret、Cookie、token 或原始响应。

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/clients/http.rs](crates/auction-integrations/src/clients/http.rs) | 统一 reqwest/rustls 客户端 |
| [src/clients/ehentai](crates/auction-integrations/src/clients/ehentai) | 论坛认证、受限请求、主题/固定帖协议 |
| [src/clients/ehentai/client/pm](crates/auction-integrations/src/clients/ehentai/client/pm) | PM 新鲜表单准备、单次发送与同账号 Sent 精确证明 |
| [src/clients/mooglemail](crates/auction-integrations/src/clients/mooglemail) | 邮件读取、动作/发送证明与加密诊断 |
| [src/equipment](crates/auction-integrations/src/equipment) | 装备来源策略、页面解析与安全链接解析 |
| [src/credentials](crates/auction-integrations/src/credentials) | Cookie 信封与 Admin 只读认证探测 |
| [src/cryptobox](crates/auction-integrations/src/cryptobox)、[src/login_key](crates/auction-integrations/src/login_key) | 装备/登录 key 加密与短时解密 |
| [src/forum](crates/auction-integrations/src/forum) | 纯本地论坛解析与 BBCode 渲染 |

## HTTP 与协议证明

各客户端固定 origin/endpoint、禁止自动重定向，限制响应类型、字节数、请求间隔和总预算；正文按块在追加前检查上限，
超时或截断不伪称取得完整响应，用户正文中的“登录”等文字不能作为凭据失效证明。HTML 解析使用 scraper 0.27 /
selectors 0.38，关闭默认 CLI feature 但保留 `errors`，仅使用库 API，不以手写解析器替代协议边界。

- MoogleMail 读取：列表/详情身份与资产一致，完整定位 cursor 后按旧到新返回连续详情前缀。持久轮询经 `fetch_claimed`
  先提交整窗 MID/列表身份，再发详情 GET；已有待读队列时优先直接读取 MID，恢复已读但尚未提交的无附件命令。
  journal 不可用时返回 `persistence_unavailable` 并停止详情读取，摘要或 world 无效时失败关闭；allowlist 不再允许的待读项保留待核对。
- ComposeSend：首次 POST 前取得 Sent baseline，之后独立扫描，只接受新增、唯一且 world、收件人、正文摘要、
  Credits/COD、装备精确匹配的 MID。
- Return / Take：写前核对 sender 与预期资产；Return 证明精确 MID 消失，Take 证明身份不变、资产归零及唯一回执。
- 论坛读取/写入：同 thread 规范分页与真实帖子/作者身份、完整页数；写入同源表单与精确 board/thread/post，
  回执始终核对，原始正文额外确认由 Worker 的 `verify_post_body` 开关控制。
- 论坛 PM：新鲜 MID 表单绑定当前姓名与 sender，单次 POST 保存 Sent、不 tracking；完整有界遍历自己的同标题 Sent
  后核验收件人 UID、完整正文、sender 与 Sent MSID。

Compose 成功页、空 compose 或本地 UUID 均不能替代 Sent 证明；资产不符、无匹配、多匹配或预算耗尽进入
`manual_reconcile`，不自动重寄。MoogleMail transport 在持久 sending fence 前 GET compose 并验证干净状态，
失败或超时只延后新任务；`prepare` 返回的消耗式 future 持有同一 credential session 独占 guard 直到写入结束或句柄
被丢弃，fence 后仍复核 compose 和原 Sent baseline，最终写入只执行一次。不自动清草稿，当前 compose 干净仅允许
新任务继续。[transport/delivery/authorization.rs](crates/auction-integrations/src/clients/mooglemail/transport/delivery/authorization.rs)
通过 `with_cash_evidence_gateway` 注入共享原 client/credential set/owner 的联合证据 gateway，在同一 session 复用身份
与 Main cash 读取，只读 Arc 租约不取得第二个 guard；准备绝对期限覆盖排队与全部读取，取消即同步撤销 owner。
旧 `prepare/send` 拒绝非零 Credits Send，新授权入口缺 gateway 时只返回 legacy 候选。回执、诊断与人工核对要求见
[邮件诊断](ops/mail/diagnostics.md)与[领域不变量](docs/contracts/invariants.md)。

## 凭据装配与外部请求

Worker 配置数据库凭据能力后读取唯一 active Cookie 并解密，暂缺时保持 degraded，不退回另一来源；未配置该能力才
使用敏感 TOML 或 `.keyenv` key1（key2 仅供显式隔离验证）。每种 Cookie 全局最多一条 active，world 专属 scope 只服务
本世界、shared 可跨世界，失效后不自动换号。MoogleMail gateway 的列表、限速、正文与失效处理共用绝对 deadline，
详情预算耗尽可返回完整验证的最旧前缀，尚未定位 cursor/末尾时不得推进；发送全程持有凭据级 guard，
transport 并发上限为 1，读取不占用写 guard。

论坛完整主题额外最多四路分页读取，共享请求间隔与 32 MiB 聚合上限，作者身份只采信系统作者链接；目录续页增加顺序
POST、布局回读及可选正文校验成本，配置预算校验只是单目录页下界。大场次需提高既有 `item_timeout_seconds`/
`lease_duration_seconds` 并保持 lease 大于 item，超时仍走未知写入门禁。创建端口明确返回本轮 `Created` 或只读命中的
`Existing`，同标题旧主题不是本轮新正文的成功回执；论坛仅导入普通出价，代理行在账本前忽略。

`forum/render.rs` 按分类/编号排序，每帖最多 150 件、从 #1 连续分页，每页独立校验 65,531 字节上限，过长失败而不是
截断装备；续页重复分类标题，旧主题按持久化 `legacy_single` 保持一页。解析器只合并紧邻首帖、同作者且拍卖/页码
标记匹配的目录续页，随后按完整目录解析用户命令，不按正文跳过帖子；Worker 的 HTML 路径要求相同数字作者 ID，
只读纯文本预览仅比较可选作者标签。系统更新楼紧接最后一页，用户回帖从再下一楼开始。
装备名称标色消费 Worker 启动时冻结的应用窄规则：类型、种类、品质各自多选任一匹配，维度间同时满足，
按原规则顺序取首个前缀或后缀命中；空多选兼容旧单值，前后缀及 Peerless 彩虹规则保持原义。
渲染器不拆分多选规则，也不把 Admin 候选词表变成装备采集或品质接收政策。
新种类数组可选择十二种护甲材质族，由应用规则核对实际大类和完整种类的首词/部位边界；完整部位选项及旧种类单值精确匹配。
`forum/render/log.rs` 分类显示 pending、警告、忽略记录及总数，优先 pending 再取最近记录、最多 100 条且受正文预算
约束，超出时明确显示已展示/总条数。`forum/render/countdown.rs` 只在首帖顶部生成 `List Timer` 与 `Auction Timer` 的
`[img]` 链接，使用已配置 Public origin 下的[自托管动态 SVG](docs/contracts/routes.md#p22-自托管论坛倒计时)，
图片链接只含拍卖 UUID 和固定计时器类型，未配置公开 origin 时只保留 UTC 文字：List Timer 对应本场收件随正式开拍截止
（已有 `opened_at` 用事实时间，否则 `scheduled_open_at`），Auction Timer 用计划截止 `scheduled_end_at`，缺计划截止不生成
第二张图片。固定 #1 顶部保留 World、Bidding opens、
Opened、Bidding deadline，`Auction address` 使用原生 `[url=地址][b]地址[/b][/url]` 展示完整当期拍卖链接。
`About system` 链接固定指向论坛说明主题 `301946`。`forum/render/settlement.rs` 在成交行追加
`; winning for Xh / Yh; adjustment%`，仅消费已保存的结算投影，不以展示时长重算曲线，流拍无尾注；目录解析仅剥离
该结构化尾注，不放宽用户竞价语法（语义见[玩家指南](docs/guide/player-guide.md#成交价为什么不一定等于应付金额)）。

Admin A44 在 [credentials/check.rs](crates/auction-integrations/src/credentials/check.rs) 使用独立只读探测器，仅接受
数据库信封：401/明确登录失效为 invalid，挑战、战斗页、限流、超时或协议变化为 inconclusive，无写入、重试或状态回写。
每进程单并发、站点间隔 2 秒、每范围含等待预算 5 秒、正文上限 2 MiB，成功只证明当次登录态。

### PM 准备与已发送副本

`forum.enabled`、持久 sending fence、任务 lease 与 item timeout 由 Worker 装配/调度拥有，本适配器不自行启动或读取配置。
`EhentaiGateway` 实现冻结的 `ForumPmSender`，复用唯一 active ForumCookie、原 credential session、持久 owner 和共享
HTTP gate；安全 PM client 在构造时建立、随克隆共享连接池并独立强制禁止重定向，目标仅允许固定生产 HTTPS origin 或
显式配置的 HTTP loopback 隔离端点。`prepare` 只 GET `/index.php?act=Msg&CODE=04&MID=...`，从唯一 `REPLIER` 表单读取
当前 `entered_name`、令牌与系统导航 sender，不使用数据库旧 username；令牌、Cookie 和表单只留内存，
不实现 Debug/序列化。表单只允许已知控件与固定动作，强制 `add_sent=yes`、不发送 `add_tracking`，不 preview/save；
编辑器 `ffont/fsize` 仅接受默认首项为 0 的单选控件，无名 `type=button` 辅助按钮不发送，未知字段失败关闭。
只有调用方持久化 sender 并通过 sending fence 后才消费 prepared，一次消费只有一次 POST；POST 200 仅触发回读，
不是成功凭证；跳转、超时、读回不完整或无法证明唯一副本均为 `OutcomeUnknown`。标题精确为 `Outbid <notification UUID 的32位simple形式>`（39 ASCII 字节），正文上限 2048 字节、单行
ASCII 且拒绝 BBCode/HTML 括号；Sent 列表的同标题行证明 recipient 数字 UID，只请求该行的同源
`CODE=03&VID=sent&MSID=...` 详情并核验 sender、标题和 `.postcolor` 完整纯文本。每次核对最多 8 个列表页、8 个同标题
候选、8 MiB 聚合响应，分页只跟随实际校验链接、不猜 `st` 步长并去除 `s` 参数；发现首个副本不提前成功。
`reconcile` 只用当前凭据读取同 sender 的 Sent，换账号拒绝、未找到返回 `None`，绝不授权重寄。远端按姓名投递，
准备后改名可能误投且只能事后发现；历史浏览器样本不等价于本适配器验收，本轮无实发或行为测试。

## 装备来源与密钥适配

`equipment/` 分别拥有来源配置、无代理读取、页面/身份/属性解析和 canonical 事实；关闭业务 allowlist 不放宽固定
HTTPS origin、精确 world 路径、响应类型或身份校验。`equipment/game_link.rs` 将安全目标与密文 owner 绑定后短暂解密，
完整 URL 只移交受限 HTTP DTO 或论坛渲染。装备信封使用绑定装备 ID 的 AES-256-GCM，Cookie、装备 key 与用户登录 key
使用各自数据密钥和域分离；明文不进入持久化 payload、日志、审计或普通 DTO。`forum/` 只在内存解析预览和渲染，
远端请求归 `clients/ehentai`，回帖执行归 Worker。装备标记 delivery 使用独立 `lot_watch` 来源和冻结安全正文，
沿用原 ComposeSend/Sent 证明与 manual_reconcile；账号警告渠道由应用冻结，PM 使用独立 `ForumPmSender` 端口，
不伪造 MoogleMail delivery、不降级双发。

装备分类、品质、完整种类和名称识别统一来自
[domain taxonomy](crates/auction-domain/src/equipment/taxonomy)，依据
[Equipment Basics](https://ehwiki.org/wiki/Equipment_Basics)、
[Detailed Equip Characteristics](https://ehwiki.org/wiki/Detailed_Equip_Characteristics) 和
[旧装备说明](https://ehwiki.org/wiki/Obsolete_Items#Equipment) 保存当前及历史兼容元数据。
集成层不维护第二份品质、种类或 header 大类词表：`parser/html.rs` 提取 DOM 与属性，`parser/identity.rs` 适配领域名称识别，
`parser/header.rs` 解析等级/交易状态并复用领域大类映射，`parser/page.rs` 组合并交叉校验。
HTML 与 canonical JSON 使用同一个领域识别结果，完整长种类优先，来源大类或 JSON 声明不一致均失败关闭；
原名称、词缀文本、既有属性字段及 fingerprint 格式保持原义，不将领域兼容元数据加入历史摘要。

HTML 来源要求唯一 header、存在 `.eq` 属性区域且至少读取到一项属性；缺少必要结构或空属性不能成为成功快照。
主属性按 `Base:` 声明或带符号数字前导的文本识别，符号后须紧接数字或小数点，必须同时具有合法 Base 和名称/数值；其它普通说明的 `title`
不要求包含 Base。`.ex/.ep` 分组属性仍采用二至三子项及原单位协议，带 Base 或数值标记的残缺行、额外列、非法 Base
均明确拒绝；不带这些标记的零至一子项标题/说明保留可选语义。重复属性、数量及数值边界沿用原校验。
这些 DOM 完整性要求只约束真实 HTML 来源，不改变手工 canonical JSON 允许空统计集合的历史输入与摘要格式。
邮件诊断读取与普通来源读取共用 URL 校验，固定 origin/world 路径、ID/key 和业务 allowlist 不再由两条路径分别解释。

内部等级区分未声明、`Unassigned`、数字等级和合法 Soulbound Tier；只在现有 DTO/属性/fingerprint 边界投影为原
`Option<i32>`，未赋等级不伪造零级，数字范围与三个 Tier 值仍沿用原校验。领域可识别十档当前品质及 Flimsy/Fine
历史品质，交易资格及默认 Legendary/Peerless 收件门槛仍由应用策略决定，识别能力不授予收件资格。
解析失败使用九种闭集 `EquipmentSourceDetailCode`：Admin 的来源适配日志只含固定来源和原因码，HTTP 继续返回原
`InvalidSource`；MM 仅在 `Parse/ProtocolDrift` 组合携带来源码并写入既有 `detail_code`，其它阶段及旧空值保持原义。
原页面仍只进入既有敏感证据归档，原因码不包含原名称、header、页面、URL 或 key；不修改既有装备、编号或幂等结果。

## 凭据 owner 与取消传播

Worker 凭据 owner 随集合传给 gateway、克隆客户端、发送器和分页上下文，装备来源也共用 MoogleMail owner；
每个物理请求在限速后、发送前以有界检查核验原持锁 session，失锁永久停用，GET 返回不可用、POST 保守归为未知。
该边界不能撤回已发请求，也不能把本地检查与远端写入原子化。
[联合证据 gateway](crates/auction-integrations/src/clients/mooglemail/gateway/internal.rs) 从同一独占 session 读取
Character Equipment 的唯一 `let uid`、[论坛目标资料](crates/auction-integrations/src/clients/ehentai/client/profile.rs)
的完整名称和精确目标 MID，两站 Cookie 互不传递；论坛只读能力复用安全池、gate 与 owner，预期名称有值精确比较、
为 None 也读取。[Sent 组合读取](crates/auction-integrations/src/clients/mooglemail/gateway/sent_evidence.rs) 复用同
session 的既有列表/详情解析，精确匹配 MID/world/收件人/主题及可选可信 sent_at。Armory 仅接纳七类 organize 的完整
计数、同 world/MAIN_URL/action、正常非 disabled 且 id/value/行 handler 同一 EID 的正向行，还须同 session 干净 compose。
organize 表单省略或留空 `action` 时按当前固定 world/category 请求 URL 解析；显式地址仍须同源、同 world 且四项 query 完全匹配。
准备、限速、所有 GET/body 和提交共享不超过 30 秒的绝对预算并保留一秒提交余量；内部提交只接受数据库 active
ID/version 与持久 generation，配置 Cookie 或旧 owner 明确 Unsupported。
`record_only` 由应用从持久账户策略决定是否请求现金，transport 在原 session 中保留身份和发送授权，但不读取余额。
联合证据失败仅记录闭集阶段和错误类别；论坛目标资料另记录结构阶段及目标 MID 数量，不记录 UID、整页、URL 或凭据。

## 维护与验证

```bash
cargo check --locked -p auction-integrations
```

仅静态编译；默认检查与测试范围见[开发指南](docs/guide/development.md)，运行证据见[验证状态](docs/status.md)。
修改行为时同步对应契约，外部调用验证须由用户明确要求。
