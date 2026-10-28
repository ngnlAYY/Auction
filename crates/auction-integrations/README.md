# auction-integrations

[文档导航](docs/README.md) · [架构](docs/design/architecture.md) · [契约](docs/contracts/README.md)

E-Hentai、MoogleMail、装备来源及密钥信封的外部适配器。实现应用层外部能力端口，只返回校验过的事实或安全错误：
外部响应必须经过应用命令或持久化 Worker 操作才能改状态，本地提交后的外部失败不能撤销已接受竞价；
错误、metadata 与普通投影不得包含 URL 中的 secret、Cookie、token 或原始响应。

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/clients/ehentai](crates/auction-integrations/src/clients/ehentai) | 论坛认证、受限请求、主题/固定帖协议 |
| [src/clients/ehentai/client/pm](crates/auction-integrations/src/clients/ehentai/client/pm) | PM 新鲜表单准备、单次发送与同账号 Sent 精确证明 |
| [src/clients/mooglemail](crates/auction-integrations/src/clients/mooglemail) | 邮件读取、动作/发送证明与加密诊断 |
| [src/equipment](crates/auction-integrations/src/equipment) | 装备来源策略、页面解析与安全链接解析 |
| [src/credentials](crates/auction-integrations/src/credentials) | Cookie 信封、Admin 论坛账号获取及只读认证探测 |
| [src/cryptobox](crates/auction-integrations/src/cryptobox)、[src/login_key](crates/auction-integrations/src/login_key) | 装备/登录 key 加密与短时解密 |
| [src/forum](crates/auction-integrations/src/forum) | 纯本地论坛解析与 BBCode 渲染 |

## HTTP 与协议证明

生产装配的论坛、MoogleMail 与装备来源客户端固定 origin/endpoint、禁止自动重定向，限制响应类型、字节数、
请求间隔和总预算；正文按块在追加前检查上限，超时或截断不伪称取得完整响应。论坛和 MoogleMail 安全客户端显式
关闭 reqwest 协议层自动重试，Cookie HeaderValue 标为 sensitive。兼容注入客户端的代理、重定向和重试策略仍由装配方保证。
论坛与 MoogleMail 的限速槽仅在等待完成后推进，
取消排队请求不会预占未来时间槽。用户正文中的“登录”等文字不能作为凭据失效证明；战斗状态也只识别真实
`battle_main` DOM 节点，不接受正文或脚本中的同名文字。HTML 解析使用 scraper 0.27 / selectors 0.38，
关闭默认 CLI feature 但保留 `errors`，仅使用库 API，不以手写解析器替代协议边界。
邮件和装备请求的登录页判断沿 DOM 深度维护隐藏状态，拒绝提示也沿同一原则提取可见文本；深层嵌套不再逐节点重扫祖先。
被 `script`、`style`、`noscript` 或 `textarea` 包围的内容仍不能作为登录失效或发送拒绝证据。
Sent 摘要、快照与维护查询的 `Debug` 隐藏收件人和主题；内部投递载荷不提供会输出正文的 `Debug`。
本地隔离端点的回环判断使用 URL 已解析的地址，支持带方括号的 IPv6 `::1`；论坛仍仅接受
`localhost`、`127.0.0.1` 与 `::1`，MoogleMail 保留原 IPv4 回环网段策略，不扩展生产 origin。

- MoogleMail 读取：完整发件人只采信详情，不以可能截断的列表显示名作身份比对、业务身份或 allowlist 判断。
  完整定位 cursor 后按旧到新返回连续详情前缀；每批先有界收集全部候选，发件人白名单在详情解析后精确校验。
  不匹配时保留待读项并失败关闭，不领取、退回、回复或越过该项；后续邮件等待调整白名单后继续。
  持久轮询经 `fetch_claimed` 先提交整窗 MID/列表快照，再发详情 GET；已有待读队列时优先直接读取 MID，
  恢复已读但尚未提交的无附件命令。旧摘要显示名仅保留快照兼容，不重写旧记录、不重置 cursor；
  journal 不可用时返回 `persistence_unavailable` 并停止详情读取，摘要结构或 world 无效时失败关闭。
  已登记 MID 不在 Inbox 时，通过 Read Mail 列表与原身份定位；历史已读邮件不成为新入站命令。
  恢复窗口只为本批 pending 建立借用索引，Inbox/Read Mail 各线性遍历一次；不为每个 MID 重扫整箱，也不丢弃两表均缺失的待读项。
  多附件的独立领取按钮不按数量拒绝；装备 key 只采信附件 ID 对应的严格证据，正文其它装备链接即使畸形也不参与校验，
  Unicode 正文不得触发字节切片 panic。脚本预筛选一次扫描附件 ID 和遮罩后的括号栈，只解析目标字段所属的最内层对象，
  避免外层 JavaScript 函数体被误当作 JSON；预筛选不替代严格 JSON、重复字段及正文/脚本一致性校验。
  `with_progress_observer` 只在持久 claim 路径通过应用窄端口报告真实详情 GET 目标，
  不以待读队首推断当前邮件；观测只进入 Worker 内存，不写入命令、cursor 或外部服务。
  调用方绝对截止只能收紧配置 fetch budget；末页同步解析完成后仍复核期限。
  金额/资产脚本的同名重复赋值、不明右值、复合改写或属性赋值均失败关闭，不沿用旧数值。
- 普通 Inbox/Read Mail/Sent 读取通过共享协调器合并同一有效观察窗口的请求，不获取 compose 操作锁；
  完整列表与改变已读状态的详情读取分阶段协调，发送及资产写入使用新鲜屏障。凭据失效、owner 撤销、
  世界或读取代次变化不得复用旧响应；有界观察仅用于减少重复读取，不构成内部资金/库存提交授权。
  协调器最多保留 8 个凭据/world/generation scope、8 个在途共享请求、32 MiB 已接收页面及列表数据；
  每代最多 512 个请求，普通复用窗口 30 秒、读取代次绝对寿命 120 秒。上限耗尽明确失败，不能伪装为空邮箱；
  在途网络响应仍受原单响应上限约束，上述 retained 上限不是整个进程 RSS 承诺。
  失败结果只交给当次等待者，不作为后续重试的缓存；重新派发仍累计占用同一代次的请求次数，不能绕过总上限。
  已知 MID 的 Sent 履约读取按页定位，命中合法列表行后即核对原详情；只有到尾页仍未命中才能判缺失，页数耗尽仍是
  不完整错误。履约使用不早于 claim 的独立新鲜只读轮次，不复用旧观察，也不使其他读取者失效；全局并发、内存、
  owner 与写屏障校验不变。新发送基线、未知候选搜索和写后新增证明仍要求完整扫描，资金证据期限不变。
- ComposeSend：完整 Sent baseline 在持久 sending fence 前、同一独占 session 内取得，之后一次消费且写后独立扫描，只接受新增、唯一且 world、收件人、正文摘要、
  Credits/COD、装备精确匹配的 MID。发送与失败诊断借用同一输入，分步附件证明使用原 ID 前缀，避免复制整封邮件；
  compose 正文只提取一次，资产集合排序使用借用，表单编码直接移交 HTTP body，原有 compose 暂存顺序及最终证明条件不变。
  任何 compose 资产暂存成功后，后续写前失败也保留已外写状态；暂存证明解析失败保留原加密诊断引用，仍按未知写入处理。
  最终发送响应的拒绝分类与成功提示共用一次 DOM，仍分别留证，Sent 与空 compose 才构成成功证明。
- 装备发送准备：`sender/equipment.rs` 在 sending fence 后、暂存任何资产前，按完整七类 Armory 库存核对本次装备，
  按需移出仓库并解除保护／锁定；`parser/reads/inventory/` 绑定原生 EID、显示名、状态及短期 token，
  与内部履约的正向库存读取共用页面证明。外写后必须回读，不自动卸装、回滚或重试；
  每行 checkbox ID 与 handler 直接借用剥离固定前后缀后精确比较，不为全库存逐行构造期望字符串。
  协议字段与故障处理见[附件装备状态准备](ops/mail/diagnostics.md#附件装备状态准备)。
- Return / Take：写前核对 sender 与预期资产；Return 只接受身份、正文及全部资产精确匹配的 `[RTS]` 回执，
  不再把未读 Inbox 中目标 MID 缺席当作成功（详情 GET 本身会标记已读）；缺少回执保持 `outcome_unknown`，不自动重寄。
  Return 精确比较借用原正文与装备 ID，不构造包含原文的完整预期副本。
  Take 证明身份不变、资产归零及唯一回执。Take 授权准备先固定原独占凭据 session 并核验 owner；
  `DeliveryTakeExpectation` 仅冻结 world/MID 与原 credential ID/version/generation，资产仍来自不可变 claim。
  原 session 同时覆盖持久 sending fence 的撤销 guard 和一次性动作 future；详情 GET、精确资产校验及 POST
  留在该 future 内，fence 后不重新取得当前 Cookie。Return 不扩展为 Take 冻结或自动恢复。
  维护读取复用完整详情解析，明确缺失也返回当次观察的 credential binding；应用核对原账号/版本，
  存储再锁验当前 generation、原尝试与候选 fence。新旧账号的同 MID 缺失不能混用。
  旧无 Take 冻结身份或配置 Cookie 无持久 binding 时可即时证明动作，但未知后不自动确认或重排；
  严格缺失提示、全空资产与精确原资产重排的区别见[维护契约](ops/mail/diagnostics.md#定期三页维护)。
- 论坛读取/写入：同 thread 规范分页与真实帖子/作者身份、完整页数；写入同源表单与精确 board/thread/post，
  回执始终核对；正常正文额外确认由 Worker 的 `verify_post_body` 开关控制，回帖未知写入始终只读核对，
  首帖同时精确核对编辑表单中的真实标题与描述。正文调试输出始终脱敏。
  创建主题还要求表单 action 为同源 `/index.php`，可选 `act`/`CODE` 与新主题动作一致，且不携带已有 thread/post；
  短时 `s` 与既有同值重复隐藏字段仍按原协议保留，冲突路由在 POST 前拒绝。
  HTTP 200 的创建回执与固定帖回执共用唯一 `Please stand by...` 标题、唯一 meta refresh 和一致同源链接证明；
  错误页中的普通主题链接不得产生 `Created`，证据缺失或冲突保留 `write_unknown`，不授权再次 POST。
  标题使用 `[Auction] {Main|Isekai} Equipment Auction - Session {sequence:03} ({state})`，状态直接来自同源数据库快照；
  保留至少三位期数，本地最多 80 字符，不截断身份；远端是否接受长标题须另行验证，原回执/回读及未知写入门禁不变。
  精确对账覆盖其余六种新状态标题、七个旧无前缀状态标题、七个旧短标题、旧无状态完整标题和旧 `HV#` 摘要标题，
  历史形式仍遵守当时的长度边界；当前标题与候选合并为最多二十三个精确标题，一次版面读取完成核对。
  同名重复主题或候选指向不同主题一律失败关闭，不模糊匹配，也不因状态变化自动新建主题。
- 论坛 PM：新鲜 MID 表单绑定当前姓名与 sender，单次 POST 保存 Sent、不 tracking；完整有界遍历自己的同标题 Sent
  后核验收件人 UID、完整正文、sender 与 Sent MSID。

Compose 成功页、空 compose 或本地 UUID 均不能替代 Sent 证明；写后资产不符、无匹配、多匹配或预算耗尽进入
`manual_reconcile`，不自动重寄。MoogleMail transport 在持久 sending fence 前 GET compose 并验证干净状态，再完整读取 Sent 基线，
失败或超时只延后新任务；`prepare` 返回的消耗式 future 持有同一 credential session 独占 guard 直到写入结束或句柄
被丢弃，fence 后仍复核 compose，消费原完整基线并执行独立新增证明，最终写入只执行一次。基线页数耗尽与超时分别为
`sent_scan_page_limit`/`sent_scan_timeout`；准备失败只延后新任务，写后同类错误仍未知，历史人工核对记录不改写。不自动清草稿，当前 compose 干净仅允许
新任务继续。[transport/delivery/authorization.rs](crates/auction-integrations/src/clients/mooglemail/transport/delivery/authorization.rs)
通过 `with_cash_evidence_gateway` 注入共享原 client/credential set/owner 的联合证据 gateway，在同一 session 复用身份
与 Main cash 读取，只读 Arc 租约不取得第二个 guard；Worker 原准备绝对期限覆盖排队与全部读取。
完整基线完成后、首次资金证据读取前收紧到原 30 秒证据预算；后续交接/事务只收紧该截止，取消提交即同步撤销 owner。
实际 Send 在准备阶段渲染一次，后续 future 消费同一请求；`DeliverySendExpectation` 只保存安全的主题、
正文摘要及资产矩阵，随 Sending 原子落库。所有来源的冻结恢复复用原尝试，不从当前模板或登录 key 重建旧邮件。
旧 `prepare/send` 拒绝非零 Credits Send，新授权入口缺 gateway 时只返回 legacy 候选。回执、诊断与人工核对要求见
[邮件诊断](ops/mail/diagnostics.md)与[领域不变量](docs/contracts/invariants.md)。
旧无冻结摘要的 Send 只保留可证明的历史 reference/无密钥通知恢复；不能用当前登录 key 猜测原正文。
歧义候选、竞争发送尝试、缺页、凭据不符或资产已变化仍保留人工核对，不授权重寄。
明确发送拒绝只采信唯一系统提示区的可见文本，附件/正文诱饵不能跳过 Sent 核对。
写后 Sent 或最终 compose 核对的明确登录失效保留原类别、原加密证据并撤销当前凭据，不能被通用 `write_unknown`
掩盖；发送 POST 响应仍独立留证。已明确失效时不再追加诊断 GET；诊断 GET 首次发现失效则同步撤销，并由原 session
回写失效状态，不覆盖原投递结果、不换号、不重发。

入站 envelope 摘要与加密诊断恢复都按附件 ID 排序借用后直接散列，不复制 key 或逐件重新查找；
保留原 `mooglemail-envelope-v1` 字段顺序、长度编码及 key 字节，不改变合法历史证据认证结果。
Sent 正文摘要复用换行规范化的分配后截短尾部空白，不再生成额外的正文副本。
脚本正文直接追加各文本片段，不为每个 `<script>` 另建中间字符串；遮罩、重复赋值与 JSON 严格校验不变。
读写页面共用空白、附件 ID、Credits 与正文校验辅助；空白规范化直接写输出，不建立按词数增长的切片数组，
唯一回执匹配最多检查两个命中。

[transport/outcomes/rejection.rs](crates/auction-integrations/src/clients/mooglemail/transport/outcomes/rejection.rs)
负责将闭集拒收码映射为安全英文原因和处理建议；`auction_equipment_rejected` 新增 `{reason_text}`、`{next_steps}` 与
`{mail_reference}`（`world:external_mail_id`），保留旧 `{reason_code}`。正文不包含原始来源错误，整封邮件退回由独立 Return
投递及其远端证明负责。模板键仍为 14 项；买家 COD 价格明细只使用应用层冻结的结算事实，金额与幂等身份保持原义。
完整占位符与升级方式见[邮件正文模板](ops/config/README.md#邮件正文模板)。
模板按展开后的 UTF-8 字节数，在每段追加前执行发送正文上限；重复占位符不会先构造超大正文再拒绝，
超限不截断、不发送部分内容。Worker 启动快照 `MoogleMailNotificationTemplates::from_config` 直接持有应用层模板结构，
闭集、默认正文与占位符校验复用 `AdminRuntimeMailTemplatesConfig`，本 crate 只拥有渲染与发送上限。
`MoogleMailWorld` 与领域 `SettlementWorld` 通过 `From` 双向转换，网关与装配根不再手写世界映射。

[transport/payload/settlement/pricing.rs](crates/auction-integrations/src/clients/mooglemail/transport/payload/settlement/pricing.rs)
校验买家装备 COD 的完整价格变量并格式化千分位金额、带符号百分比与适用的奖池抵扣；旧三变量载荷仅省略价格明细，
不补零或套用现行曲线。只接受精确旧三/八变量或显式 `pricing_mode=hold_curve_v2` 的九变量；
v2 正文区分未缩放曲线与实分总优惠（含零），旧模式正文保持原字节。字段集合与金额关系见
[交付载荷契约](docs/contracts/schema.md#结算恢复与交付)。

## 凭据装配与外部请求

Worker 配置数据库凭据能力后读取唯一 active Cookie 并解密，暂缺时保持 degraded，不退回另一来源；未配置该能力才
使用敏感 TOML 或 `.keyenv` key1（key2 仅供显式隔离验证）。每种 Cookie 全局最多一条 active，world 专属 scope 只服务
本世界、shared 可跨世界，失效后不自动换号。MoogleMail gateway 的列表、限速、正文与失效处理共用绝对 deadline，
详情预算耗尽可返回完整验证的最旧前缀，尚未定位 cursor/末尾时不得推进。普通邮箱及装备读取使用可撤销的读取会话，
不等待 compose 独占 guard；发送、Take/Return、discard 与内部资金/库存证据仍保留写入互斥。
内部证据借用原 session 并持有至事务确定结束，既有绝对读取预算包含排队等待，不从共享观察生成授权。
论坛数据库凭据同样保留真实 scope，不以部署选择器覆盖它；世界相关主题和 PM 在 session 开始时核对 scope，
每次物理请求仍检查 owner。无世界目标的账号资料读取不伪造世界限制，所有路径复用同一 Cookie、连接池和限速器。
论坛及装备读取直接消费唯一候选；失效回写后结束当前操作，不保留无效轮换循环或为重试复制完整正文。
普通邮箱、维护及只读 Sent lookup 都只尝试当前唯一 Cookie，明确失效后撤销并返回，不维护轮换或重复尝试循环。
Inbox 的完成 owner 复核、已知 MID Sent 与余额读取的会话取得/失效回写均受同一配置绝对截止约束；
同步解析、缓存立即就绪及列表投影完成后再核对截止，不能仅靠 `timeout_at` 拒绝逾期结果。维护 Sent/Read Mail 扫描、
附件检查及单封 Sent 核对也遵守原验证预算。Sent 预算无法表示为截止时返回 `Configuration`，不因时间加法 panic。

论坛完整主题额外最多四路分页读取，共享请求间隔与 32 MiB 聚合上限，作者身份只采信系统作者链接；目录续页增加顺序
POST、布局回读及可选正文校验成本，配置预算校验只是单目录页下界。大场次需提高既有 `item_timeout_seconds`/
`lease_duration_seconds` 并保持 lease 大于 item；真实 POST 进度区分纯预读超时与写后未知。
创建端口明确返回本轮 `Created` 或只读命中的 `Existing`，同标题旧主题不是本轮新正文的成功回执；
所有创建、回复及编辑统一传递写入进度，历史未知不因本轮未写而解除；论坛仅导入普通出价，代理行在账本前忽略。
系统作者检查完整祖先链；正文与分页导航按深度优先顺序维护继承状态，每个元素只入栈、出栈一次。
正文保留引用排除、最近块元素、块间换行与空白规则；导航继承正文/签名标记，不再逐链接重扫祖先，
深层嵌套不会因固定层数上限而被误认作系统导航。
论坛解析内部帖子和页面的 `Debug` 仅显示长度、数量及标志，不输出原文、用户输入或包含短时参数的导航 URL。
完整目录只建立一次借用 code 索引；规范 post ID 的严格递增同时证明唯一，不另存重复身份集合。
分页身份索引借用 post ID，query 唯一性复用既有检查；同值重复参数仍拒绝，空 `st` 的编辑链接兼容保留。
装备 DOM 基数检查使用有界迭代，attributes JSON 只构造一次，严格结构与字段校验不变。
装备 URL 借用拆分路径，不建立路径片段集合或重复复制 ID；手工 URL 与邮件诊断共用快照投影。
canonical 指纹分块散列原域与原 JSON 字节，不再复制整份 JSON 拼接前缀，历史摘要保持一致。

`forum/render.rs` 按分类/编号排序，每帖最多 150 件、从 #1 连续分页，每页独立校验 65,531 字节上限，过长失败而不是
截断装备；续页重复分类标题，旧主题按持久化 `legacy_single` 保持一页。解析器只合并紧邻首帖、同作者且拍卖/页码
标记匹配的目录续页，随后按完整目录解析用户命令，不按正文跳过帖子；Worker 的 HTML 路径要求相同数字作者 ID，
只读纯文本预览仅比较可选作者标签。系统更新楼紧接最后一页，用户回帖从再下一楼开始。
分类编号保持持久化原值，格式为三字母前缀加至少两位十进制正 `i32` 编号：`One01`、`One100` 均可渲染和解析，
大编号按数值顺序排列；不限制每分类 99 件，也不重排或复用已发出的编号。
目录起拍价和 Current 都作为事实解析，可保留合法非整千金额；实际用户出价仍经过独立信用网格校验，
不能把可展示的历史金额误当成新的合法输入，或把它静默删成缺价。
装备名称标色消费 Worker 启动时冻结的应用窄规则：类型、种类、品质各自多选任一匹配，维度间同时满足，
按原规则顺序取首个前缀或后缀命中；空多选兼容旧单值，前后缀及 Peerless 彩虹规则保持原义。
渲染器不拆分多选规则，也不把 Admin 候选词表变成装备采集或品质接收政策。
渲染器与公开页使用应用层 `equipment_color_decision` 的同一命中结果；名称片段缺失的纯文本回退、首条规则优先级及
Peerless 彩虹共同决定公开页稀有/融合分组。BBCode 上色只在论坛执行，公开页消费分组字段。
分类排序直接执行借用的 ASCII 大小写比较；BBCode 规范化和括号转义直接写目标缓冲，颜色片段保留原空白，
不改变排序键、页数、字节上限或标色结果。
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

Admin A32 的 [credentials/acquire](crates/auction-integrations/src/credentials/acquire) 独占账号密码登录与 Cookie 字段映射：
新鲜表单、一次 POST、共享认证字段的控制台正向证明；固定 HTTPS 目标，无代理、重定向、自动重试或远端业务写入。
游戏只保留 `ipb_member_id`/`ipb_pass_hash`，论坛保留校验后的完整 header；密码仅在短时内存，命令摘要使用独立域 HMAC。
进程单并发、取消后仍冷却、响应大小及绝对预算见 [A32 契约](docs/contracts/routes.md#外部凭据)；
论坛账号获取只连接固定生产 endpoint，不提供回环或可配置目标入口。自有敏感缓冲尽力清零，不承诺清理 reqwest/DOM 内部副本。
全新论坛会话的表单 action 与认证导航可能携带短时 `s`：提交保留唯一合法 session 参数，
控制台个人链接允许唯一 `showuser` 与可选合法 `s`，仍拒绝重复或额外参数；不记录这些短时 URL。

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
POST 或写后 Sent 明确返回 401/登录失效时，同样撤销当前 Cookie；发送结果仍为 `OutcomeUnknown`，
不因失效判定未发送、不自动换号或重发，普通 403 不构成凭据失效证明。
`reconcile` 只用当前凭据读取同 sender 的 Sent，换账号拒绝、未找到返回 `None`，绝不授权重寄。远端按姓名投递，
准备后改名可能误投且只能事后发现；历史浏览器样本不等价于本适配器验收，本轮无实发或行为测试。

## 装备来源与密钥适配

`equipment/` 分别拥有来源配置、无代理读取、页面/身份/属性解析和 canonical 事实；关闭业务 allowlist 不放宽固定
HTTPS origin、精确 world 路径、响应类型或身份校验。`equipment/game_link.rs` 将安全目标与密文 owner 绑定后短暂解密，
完整 URL 移交受限 HTTP DTO 或论坛渲染。装备信封使用绑定装备 ID 的 AES-256-GCM，Cookie、装备 key 与用户登录 key
使用各自数据密钥和域分离；按用户选择，论坛目录、P15 和认证后的 A46 可以返回完整装备 URL，具体边界见
[I-05a](docs/contracts/invariants.md#i-05a-admin-装备录入的敏感边界和原子集合)。
论坛首帖和续页的数据库正文镜像可包含公开游戏装备直链；固定楼层身份及未知写入的对账仍遵守论坛同步契约。
`forum/` 只在内存解析预览和渲染，远端请求归 `clients/ehentai`，回帖执行归 Worker。装备标记 delivery 使用独立 `lot_watch` 来源和冻结安全正文，
沿用原 ComposeSend/Sent 证明与 manual_reconcile；账号警告渠道由应用冻结，PM 使用独立 `ForumPmSender` 端口，
不伪造 MoogleMail delivery、不降级双发。

Cookie、装备 key 和用户登录 key 的指纹采用既有域字节分块 HMAC，不构造额外的明文拼接缓冲。Cookie 与登录 key
解密后直接移交原认证缓冲，截短前清理尾部；随机源、加解密或指纹失败路径尽力清理已有敏感缓冲。
装备、Cookie、用户登录 key 与邮件诊断归档的加密缓冲一次预留认证标签容量，避免追加标签时再次扩容复制密文。
AES-256-GCM、AAD、nonce、指纹域及十六进制编码保持原格式，不迁移或重写历史信封。
装备信封的只读字段改为借用，nonce 转为栈数组；仅原地解密复制必要的可变密文缓冲，原信封不变。
运行时的 `EhentaiCookie` 与 `MoogleMailCookie` 克隆共享明文缓冲和不可逆撤销位，最后一个引用释放时尽力清理；
`MoogleMailToken` 与 `MoogleMailEquipmentKey` 销毁时同样清理当前缓冲。MoogleMail 表单逐值覆写，而非仅缩短
`String` 长度。Cookie 构造不无条件另建裁剪副本，确需裁剪时清理原值；附件 key 从解析表直接移交。
这不承诺清理 HTTP/DOM 库内部或分配器曾产生的副本，也不改变凭据失效、owner 或未知写入策略。

装备分类、品质、完整种类和名称识别统一来自
[domain taxonomy](crates/auction-domain/src/equipment/taxonomy)，依据
[Equipment Basics](https://ehwiki.org/wiki/Equipment_Basics)、
[Detailed Equip Characteristics](https://ehwiki.org/wiki/Detailed_Equip_Characteristics) 和
[旧装备说明](https://ehwiki.org/wiki/Obsolete_Items#Equipment) 保存当前及历史兼容元数据。
集成层不维护第二份品质、种类或 header 大类词表：`parser/html.rs` 提取 DOM 与属性，`parser/identity.rs` 适配领域名称识别，
`parser/header.rs` 解析等级/交易状态并复用领域大类映射，`parser/page.rs` 组合并交叉校验。
HTML 与 canonical JSON 使用同一个领域识别结果，完整基础名优先匹配后拆分种类与护甲部位，来源大类或 JSON 声明不一致均失败关闭。
名称只有 Cloth、Light、Heavy 携带独立部位词，类型读取详情 header；Legendary 缺任一前后缀或只有 `of the` 时拒绝，
Magnificent 及其他兼容品质允许缺词缀。领域拆分如 `Phase`/`Robe`，集成层内部 `base_name` 明确保留 `Phase Robe`，
用于 canonical JSON 的历史 `equipment_type`、快照 `attributes.type` 及 `manual-json-v1` 指纹，保持原字段与编码。
原名称、词缀文本保持原义，不将拆分字段或领域兼容元数据加入历史摘要，也不回写已有快照。

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
论坛凭据在持久失效回写前同步撤销所有克隆；已准备 PM、分页和发帖会话在 owner 检查前后及构造 Cookie header 时
拒绝该凭据，普通 GET/PM GET 完成后也复核撤销位。撤销不能撤回已发送请求，既有写后未知边界不变。
[联合证据 gateway](crates/auction-integrations/src/clients/mooglemail/gateway/internal.rs) 从同一独占 session 读取
Character Equipment 的唯一 `let uid`、[论坛目标资料](crates/auction-integrations/src/clients/ehentai/client/profile.rs)
的完整名称和精确目标 MID，两站 Cookie 互不传递；论坛只读能力复用安全池、gate 与 owner，预期名称有值精确比较、
为 None 也读取。[Sent 组合读取](crates/auction-integrations/src/clients/mooglemail/gateway/sent_evidence.rs) 复用同
session 的既有列表/详情解析，精确匹配 MID/world/收件人/主题及可选可信 sent_at。Armory 仅接纳七类 organize 的完整
计数、同 world/MAIN_URL/action、正常非 disabled 且 id/value/行 handler 同一 EID 的正向行，还须同 session 干净 compose。
organize 表单省略或留空 `action` 时按当前固定 world/category 请求 URL 解析；显式地址仍须同源、同 world 且四项 query 完全匹配。
准备、限速、所有 GET/body 和提交共享不超过 30 秒的绝对预算并保留一秒提交余量；内部提交只接受数据库 active
ID/version 与持久 generation，配置 Cookie 或旧 owner 明确 Unsupported。
内部租约的 owner 检查前后均复核原绝对截止；即使检查 future 立即完成，也不能接受已过期租约。
`record_only` 由应用从持久账户策略决定是否请求现金，transport 在原 session 中保留身份和发送授权，但不读取余额。
联合证据失败仅记录闭集阶段和错误类别；论坛目标资料另记录结构阶段及目标 MID 数量，不记录 UID、整页、URL 或凭据。

## 维护与验证

```bash
cargo check --locked -p auction-integrations
```

仅静态编译；默认检查与测试范围见[开发指南](docs/guide/development.md)，运行证据见[验证状态](docs/status.md)。
修改行为时同步对应契约，外部调用验证须由用户明确要求。
