# 领域不变量与写入语义

[文档导航](docs/README.md) · [领域规则](crates/auction-domain/src) · [用例](crates/auction-application/src/services) · [事务适配](crates/auction-storage/src/database)

本页按稳定 I 编号登记正确性约束，是实现契约；运行证据另见[验证状态](docs/status.md)。默认只做静态检查、源码审计和文档核对，不执行行为测试。

## 本页导航

- [命令、幂等和标识](#命令幂等和标识)
- [事务、锁和原子写入](#事务锁和原子写入)
- [领域状态和金额](#领域状态和金额)
- [外部副作用、Worker 和缓存](#外部副作用worker-和缓存)
- [审计、凭据和事件](#审计凭据和事件)
- [能力档位和模块 owner](#能力档位和模块-owner)
- [验收边界](#验收边界)

## 命令、幂等和标识

### I-01 命令账本是权威幂等边界

每个可变命令必须有稳定 `command_id`。通用 `auction_commands` 账本保存 UUID 主键 `command_id`、非空 `command_kind`、聚合 `aggregate_id`、32 字节 `request_hash`、稳定 `result_code`、JSON object `result_payload` 和事务时间 `created_at`；专用账本按各自契约绑定同等幂等身份。相同 command_id 加相同请求摘要必须重放原结果，相同 command_id 搭配不同摘要必须返回冲突；重复请求不得再次写入竞价事实、审计记录、结算义务或外部命令。

创建拍卖也必须先查询账本，再校验当前时间，原命令成功后的重放不能因预览时间已过去而失败。浏览器处理未知写入结果时保留完整命令负载及版本，不能只保留 ID 后重新读取并拼装另一份请求。历史 `accepted + CompetitionPlan` 重放也必须核对 payload 与命令/lot 绑定；普通竞价首条事实核对请求金额、竞价者、Ordinary 机制和确定性 ID；慢速计划还核对授权、清除后的 generation 及反超/资格冻结结果形状，不得用内部自洽的其他命令结果替代原结果，也不能按当前状态重算旧命令。
普通命令存在第二条事实时，还必须绑定确定性第二条 bid ID、Proxy 机制和不同于首条的出价人；
合法同价反击继续允许，不以内部自洽替代命令归属证明。

Public 登录时长命令必须先在锁内复核当前会话仍有效，再查询/重放账本；不能把“原命令成功”解释为过期或撤销会话仍可授权。
摘要绑定账号、当前会话、时长和预期版本；重放返回原期限，不延长当前会话、不重复写审计。
合法删除凭据导致命令外键变为 NULL，不得破坏旧导入、改名和启停的安全结果重放；仍核对原类型、摘要及 payload，
不重取现行凭据或重新外呼。浏览器未决命令属于稳定身份/页面宿主，加载错误、分页或展示阶段变化不能清除原 UUID、
版本和负载；临时会话故障保留状态但禁写，累计未知不因之后的明确拒绝解除。

### I-02 可重试写入使用确定性事实标识

重试、进程恢复和 Worker 重放不得根据随机 UUID 产生重复事实：竞价事实、代理授权和其他可重试领域对象使用由稳定业务输入派生的确定性标识，或由唯一约束和命令账本共同保证只落一份。

## 事务、锁和原子写入

### I-03 竞价和生命周期写入不可拆分

竞价、代理出价、拍卖状态推进和影响结算的管理命令必须在同一数据库事务中完成：锁定受影响拍卖、标的和有效资格所需的权威行；校验命令幂等性、身份、资格、状态和版本；按领域规则计算新金额、结束时间和状态；写入领域事实、状态版本、审计事件及持久化事件/outbox；提交后才允许触发论坛、邮件、通知等外部写入。禁止先在一个事务读取当前价、再在另一个事务写入竞价；事务失败或冲突时整个写入集合回滚，不留下半个竞价、半个审计事件或无法解释的 outbox 记录。

开拍事务允许保留同场 `withdrawn` 拍品，但只激活有投稿来源的 `pending` 拍品；至少存在一件待拍品，且实际激活行数必须与锁内校验的待拍数量一致；已撤回拍品的状态、版本和返还记录不因开拍改变，其他非法状态仍必须使整个事务失败。PostgreSQL 结算领取、锁内截止复核和完成数量校验同样排除撤回拍品，禁止空集合结算；活动代理检查仍覆盖全部拍品，不能借撤回状态绕过未清理代理的阻断。

登记了 scheduled round 的 Preview、Open、Final 均在生命周期事务内原子写入对应完成 fence；人工操作不抢占有效
Worker 租约。后续 Worker complete 只能确认原轮次/拍卖/命令的已提交结果，不再用另一事务补写缺失生命周期事实。
手工场次要求 `preview < open < end`，两小时只属于自动邮箱暂停提前量，不是手工排期最小预览间隔。

规则发布在 PostgreSQL Serializable 事务中按 locale 做 revision CAS，原子写文档、`rules_admin_commands` 和审计；同命令重放原文档，摘要/locale 不同冲突。世界启停先锁 `worlds` 并校验 `policy_revision`，仅从启用变为关闭时检查非 `settled`/`cancelled` 拍卖与活动未完成轮次 blocker；状态变更通过 CAS 与 `auction_commands`、审计同事务提交，不能省略审计提供写入。
规则文档不存在时仅 `expected_revision=0` 可创建 revision 1；非零预期版本必须匹配现存文档，不能通过 UPSERT 跳号新建。
Admin 创建拍卖先按 world → intake target 取得原有锁，再读取数据库实时时钟校验预览时间；已接受命令仍先重放。
周轮次 bootstrap 同样使用锁后 `clock_timestamp()`，不能把事务开始时间当作锁等待后的当前时间。
Admin/MM 录入同样分离来源采集时间与本地业务时间：取得 world/target/auction 锁后读取数据库实时钟，
聚合更新时间不早于其创建/原更新时间。卖家按 Users 的大小写不敏感唯一名称定位并锁定，不能把大小写变化当新身份放行黑名单。

### I-04 PostgreSQL 并发机制

- PostgreSQL 竞价和关键生命周期写入（含结算 `final → settling` 领取）使用 Serializable 隔离并配合明确行锁；序列化冲突下最多尝试 3 次（含首次），超过后返回可识别的依赖/冲突错误。
- 普通/代理/慢速竞价的 world/auction 只读门禁使用 `FOR SHARE`，lot 与资格、代理、账本仍持有各自排他锁；按 world → auction → lot 取得锁；共享锁必须与启停/生命周期更新互斥，不能替换为普通读取或允许这些状态字段被同时更新的弱锁。
- PostgreSQL 结算事务提交只重试明确映射为序列化冲突或死锁的错误；连接中断、约束失败和其他提交错误必须保留原分类并立即失败，不能重放未知提交结果。
- PostgreSQL 的队列领取、游标推进等路径可以使用 Read Committed，但必须通过明确锁、唯一约束或条件更新保护领取和状态变化，不能把较低隔离级别扩展到竞价正确性路径。
- 非法状态迁移必须失败关闭，不能用“最后一次写入获胜”掩盖并发冲突。
- 人工及 Recovery 拉黑使用 lot → proxy → user → session 锁序，与偏好/登录的 user → session 保持一致；
  只重试明确的 `40001`/`40P01`，最多三次完整事务，原命令、密钥材料、摘要及历史时刻不重新生成。

### I-05 成功竞价的写入集合是原子的

成功竞价必须同时具备竞价事实、状态版本、追加式审计记录和持久化事件/outbox，任何一项写入失败都必须使整个命令失败；外部论坛或邮件调用不能放在本地事务内，也不能在本地提交失败后提前告诉用户成功。

纯竞价计划在写入和重放前也必须自洽：公开事实金额不得倒退，只有代理事实允许同价，最终价格、领先者及机制必须与最后一条事实一致；同一授权的转换必须连续衔接版本、金额、模式和状态，最终授权快照必须匹配最后一条转换，已提供的活动快照必须与活动授权标识一致。历史账本允许仅携带本命令的转换与局部快照，不能要求补齐未变更授权或重套新输入的 1B 限制，该局部校验不替代数据库锁内读取、资格校验和命令摘要核对。竞价计划新写入的代理授权创建/更新时间、追加转换时间及命令结果使用同一锁后数据库 `occurred_at`，不得另取事务开始时间或整秒时间造成历史倒序/精度分叉，已有历史不自动重写。

### I-05a Admin 装备录入的敏感边界和原子集合

Admin 装备录入读取外部 URL 或 canonical JSON 时，raw 装备 key 只能存在于短生命周期的应用对象中，不得进入日志、HTTP DTO、命令账本、审计 metadata、事件 payload 或普通装备字段；提交数据库前必须用配置中的数据密钥封装为带 nonce、key id 和 fingerprint 的密文信封，数据库只保存信封。

允许离开装备密文边界的公开数据路径是 P09/P11 lot JSON 及 P14 补偿流水中既有的 `game_equipment_url` 字段：服务端必须先核对当前可公开的 lot、装备 owner、world、无 key 投影和密文信封，再在短生命周期内解密并构造固定 HentaVerse origin 的完整 URL。该产品边界允许 10 位 key 出现在这三类响应体和浏览器 `href` 中，但不得进入日志、审计、事件、命令账本或持久化缓存；响应必须使用 `no-store` 与全局 `no-referrer`，不能返回看似可点击的 keyless URL。浏览器向 HentaVerse 发起的用户主动导航会按外部装备协议携带 key，这是显式交付边界。

PostgreSQL 完整档的录入必须在同一 Serializable 事务中完成命令账本占位、世界级目标锁、玩家资格检查、装备与密文、submission、pending lot、拍卖版本、审计记录和实时事件，任一项失败都必须回滚；外部来源读取发生在写事务之前，来源失败不能留下半个 lot；相同 command_id 只重放脱敏结果，不得再次抓取来源或重复写入事实，能力缺失必须明确报告，不能伪装成成功。唯一录入目标可以指向 Draft 或 Preview，但必须与拍卖的真实状态精确匹配；Preview 不是冻结目录：此阶段新增的 Admin/MM 装备仍属于本期，并必须在同一事务把新 lot 追加到 `auction_preview_items`、推进 `preview_revision` 后再发布目录重读事件。正常生命周期中只有 Preview 正式进入 Live 的事务才能创建下一期 Draft 并移动录入目标，唯一例外是 Preview 被取消时取消事务必须建立后继 Draft，避免该世界失去可用录入目标；与开拍或取消并发的录入要么完整进入原场次，要么在目标切换后完整进入下一期，不能跨两期拆分。

装备名称与分类的纯语义词典由 `auction-domain::equipment::taxonomy` 唯一拥有，`auction-integrations` 提取页面并完成来源交叉校验：
名称按品质、前缀、种类、护甲部位、后缀拆分；只有 Cloth、Light、Heavy 有独立部位，例如 `Phase` 与 `Robe` 分别表示种类和部位。
装备类型来自详情 header，不是名称中的词条；名称词典只提供所属类型以交叉校验，必须与页面或 canonical JSON 声明一致。
完整基础名优先于短名称匹配（例如 `Great Mace` 不得拆成 `Mace` 与伪前缀，`Oak Staff`、`Force Shield` 不拆部位）。
新来源名称的 Legendary 必须同时具有前缀和后缀，缺任一项返回既有名称结构错误；Magnificent 允许缺少词缀。
领域分开保存种类与部位；canonical `equipment_type`、快照 `attributes.type` 及指纹仍明确映射历史完整基础名，不重写既有记录或命令结果。
当前、历史与有限兼容语法携带来源版本；未知种类拒绝，未知前后缀在长度/文本边界内保留原文，不被推荐候选自动删除。
现行十档和历史两档品质的识别不授予收件资格。明确的 `Level Unassigned` 在来源模型中与数值等级、Tier 及缺失状态区分，
输出既有快照时仍映射为缺省等级，不能伪造为 0 或当前玩家等级；其它非数字等级仍拒绝。扩充 Wiki 装备种类及历史名称兼容只改变来源识别，
不跳过可交易性、品质允许列表、等级限制或录入资格，不修改既有装备快照与审计事实。精确词表及受限兼容语法见
[装备词典说明](crates/auction-domain/README.md)及[装备来源说明](crates/auction-integrations/README.md)。
HTML 来源必须具有唯一 header、必要属性区域和非空属性集合；已识别属性行的 Base、数值或列结构损坏时拒绝，
不能静默删掉该行再写入残缺快照，合法可选说明行保持兼容。手工 URL 与 MM 诊断读取复用同一来源 URL/allowlist 门禁，
在凭据访问与远端请求之前核验。手工 canonical JSON 的原字段及指纹协议保持不变。

### I-05b Admin 论坛预览是有界的纯本地操作

Admin 论坛预览请求必须在 HTML 与纯文本中恰好选择一个非空输入，输入体积不得超过 2 MiB；解析器只能在内存中生成 catalog、posts、warnings 和命令预览，解析失败必须返回明确的输入错误，不能产生数据库写入、远程抓取、回帖或“已发布”的成功信号；预览输出不得包含 Cookie、密钥或其他外部认证材料，远程发布仍由后续 Worker 的已提交事实和外部投递边界负责。

### I-05c 服务心跳是有界的可过期安全投影

服务心跳必须校验服务/职责闭集、实例与版本边界、正的 observation sequence、tracing 计数和 TTL，旧 sequence 不能覆盖新快照；数据库读取只返回未过期且通过安全投影校验的字段，details 原文不进入 dashboard。健康聚合缺少任一 required service 时必须降级；尚未迁移的 owner section 必须返回 `unknown`，不能用默认值伪造健康。

## 领域状态和金额

### I-06 状态机只允许声明过的迁移

拍卖状态迁移必须由领域层集中定义；非法迁移、未知标的、过期版本、无效身份和不满足资格的命令必须明确失败，HTTP 层、Worker 和存储适配器不能各自复制一份状态机或静默修正输入。

自动 bootstrap 的 Draft 只在策略快照和轮次保存计划结束时间，开拍事务才写入聚合运行期结束时间；新后继冻结启动时有效经济模板，SQL 规则列与 JSON 同源，前驱快照、既有排期来源和已接受命令不重写。

拍卖聚合的排期、开放、软截止延长、生命周期迁移及提前结算遵守失败原子性：版本溢出时状态、时间、版本和取消信息全部保持原值，业务校验和版本检查必须先于其他字段修改。代理授权的内存变更同样遵守失败原子性：版本溢出、无效快照或不匹配的终止原因返回错误时，金额、模式、状态和版本必须全部保持原值，不能依赖调用方丢弃半修改对象；创建转换不得携带终止原因，FinalFast 的终止转换必须保留其 Fast 模式。初始生命周期至少覆盖创建、预览、开放、进入最后阶段、取消、结算和恢复所需的边界；具体状态集合和迁移表以领域模块及本仓库冻结的版本化契约为准，新增状态必须同步更新事件、路由、数据库和运维文档。

PostgreSQL 完整档还必须在数据库边界保护拍卖聚合身份不可变、前序拍卖序号早于后继，以及每个世界至多一个处于 Draft/Preview 的可录入拍卖且 `auction_intake_targets` 恰好指向该聚合并保存相同状态。后两者是跨行不变量，使用跨状态唯一索引和延迟约束触发器，允许生命周期事务先推进状态、再同步或移动 target，但提交时必须一致；论坛同步入队不属于这些领域触发器，由 Worker 从已提交生命周期版本发现并持久化领取。

`worlds.enabled` 是新业务进入当前生命周期的权威门禁：关闭时必须拒绝存在 Draft/Preview/Live/Final/Settling 拍卖或活动未完成轮次的世界；成功关闭后，Public 当前目录、竞价写入、当前实时流和 Worker 新一周 bootstrap 都不得继续处理该世界。已经提交的 settlement、fulfillment、delivery 与 recovery 义务仍按持久队列排空，不能因关闭世界而被丢弃。Public 历史读取只为 `settled` 拍卖保留例外，即公开谓词为 `world.enabled || auction.status == settled`，该例外不能扩张到 cancelled、preview 或其它活动状态。

### I-07 出价计算必须遵守冻结的离散规则

基线规则包含以下不可被浮点或近似算法替代的语义：

- 外部普通出价、代理上限和规则默认金额使用 1000 为信用网格；历史 lot 可以使用任意正整数起拍价，并允许首次或固定增量自动代理事实保留同一网格余数；
- 新普通出价、新建/降低代理上限及自动代理的新竞价事实最高为 1B（1,000,000,000 Credits），包含恰好 1B；超额外部输入必须失败，不能自动缩为上限；既有金额值对象、历史授权/事实及幂等重放继续保留原值，不截断结算总额；自动代理的有效竞价上限不得超过 1B，到达上限时保留已有代理的同价优先级；下一口数学最低价可超过 1B 以保持读取契约中的严格抬价关系，但不得作为可提交金额，浏览器应显示上限状态并禁用新出价；
- 默认起始金额为 1,000；
- 默认使用 50,000 PPM 的经济增量，按信用网格向上取整且最低增加 1,000；
- PPM 明确设为 0 时使用冻结的最低增量（默认 30,000）；
- Fast、SlowDue、Final 及代理上限对抗的自动反超增量为 1000；普通用户的下一口仍使用冻结的 PPM/固定增量，不因上一笔来自代理而改为 1000；
- 新场次默认软截止窗口和延长时长均为 15 分钟（900 秒）；
- 取消截止边界为开始前 24 小时。

实际实现必须以 `auction-domain` 的领域常量、金额类型和边界条件为权威。所有金额都必须为正且不溢出；外部普通出价、代理上限、论坛命令和规则默认值额外校验信用网格。存量 lot 起拍价/当前价与自动代理事实不得被静默取整：固定增量直接保留原网格余数，PPM 分支先把当前价向上折算为信用网格单位，再把向上取整的整千增量加回原始当前价，同样保留余数。软截止按接受时刻加延长时长、与当前截止取较晚值，不是旧截止累加 900 秒，不采用 15/5/1 分段；只允许把结束时间向后延长，不得缩短已经确定的有效结束时间；程序默认值不覆盖显式模板或旧冻结快照。
软截止窗口为 0 时禁用延期，包括接受时刻恰好等于有效截止的情况；截止等值的出价资格保持原规则。
合法普通出价达到 `min(历史代理上限, 1B)` 时，Fast/Slow 均立即追加原代理同价事实；
应用预分配的事实数量必须与领域规划一致，不能等待 SlowDue/Final 在截止后补救，也不能截断历史授权原值。

未发布草稿只通过显式 `repair-draft-soft-close` 单目标修正：默认只读；execute 必须具有目标、版本、稳定 command_id、actor 与精确确认短语。应用规则及 storage 锁内门禁要求从未发布/开始、无竞价/授权/结算、无论坛或未知发布证据、无排期租约且行/快照一致为 60/60，失败关闭，不删除证据绕过。Serializable 按 world → scheduled round → auction 锁序，40001/40P01 最多三次；先重放原账本再校验当前状态，CAS 只修改两时间字段及对应快照、版本和更新时间，与原结果/追加审计同事务提交；不追溯进行中或已结算场次，不自动重开或批量修改。

### I-07a 结算策略与命令重放必须可从权威事实重建

`hold_curve_v1` 与 `hold_curve_v2` 必须启用补偿池，`legacy_zero_v0` 必须关闭补偿池；配置不能组合出会计算税额但不写资金池账本的第三种语义。当前格式的完成命令重放必须使用冻结策略、首次结算资金池和锁内 lot 事实重新计算计划及义务，并与结算摘要和账本 payload 全量相等；只满足 payload 内部加总守恒不足以证明结果合法。任何漂移都失败关闭，不现场修补或重复执行结算。
新拍卖默认 `hold_curve_v2/version=2`：每件成交的卖方应收必须等于成交价 `P`。设基础折扣为 `D`，
只保留一份基础优惠 `D`，不追加旧 v1 的第二份奖池优惠。令 `A=同一锁内世界奖池期初额+本场税费`，
有折扣需求时统一缩放系数为 `r=min(1,A/ΣD)`，每件实际优惠 `C=floor(D×r)=min(D,floor(A×D/ΣD))`，
卖方收 `S=P`，买方付 `B=P-C`，保持 `B+C=S+T`。每件向下取整，尾差留池，不按遍历顺序分发；
无折扣需求时 `C=0`，奖池为零则全部优惠为零，仍正常结算，不因优惠预算不足等待或让卖方承担差额。
加价及其入池规则不缩放：买方付 `P+税费`，卖方仍收 `P`；不能使奖池变负。
物理现金与资金用途仍受 I-09h 的独立证据及授权门禁保护，逻辑奖池不授予现金发送权限。
已有 v1 快照保持原 `S=P-D, B=P-D-额外优惠`，不改金额、命令重放、旧账本或补发差额；新建模板拒绝 v1。
v2 的 `listed_price=P-D` 只表达未缩放曲线的名义价，不是实际应付。v2 账单显式冻结
`pricing_mode=hold_curve_v2`，抵扣前价为 `P+T`，`pool_credit=C` 仅表示实分基础优惠；不能再预扣或叠加另一份折扣。
旧三/八变量的载荷、摘要与正文不变，不能把新模式隐式套入旧账单。

后续新拍卖显式冻结 recovery 的 `cod_return_limit`；旧快照缺字段仍按三次解释，不回写已冻结场次或恢复记录。

### I-07b 拍品展示编号与内部顺序必须分离

`lots.lot_number` 是场次内唯一、正数且不可变的内部全局顺序，用于锁序、预览物化、历史游标和审计关联，不得因为展示规则改变而重排。`category_number` 是与不可变 `category_key` 组合使用的对外编号，在每场拍卖的每个装备分类内从 1 独立递增，撤回或重挂不得复用旧号；数据库必须同时约束 `(auction_id, category_key, category_number)` 唯一并校验分类键与 equipment category 一致。所有写命令仍以 `lot_id` UUID 定位，不能把跨分类可重复的展示编号当作身份或 cursor。论坛主题一旦开始远端发布，其编号方案必须冻结：迁移前的主题继续解释原全局编号，新主题使用分类编号；不得把既有帖子或命令账本中的 lot code 静默重解释为另一件装备。

## 外部副作用、Worker 和缓存

### I-08 外部写入发生在本地提交之后

论坛发帖、邮件发送、通知广播和其他外部副作用只能消费已提交的本地事实；外部失败必须进入可重试、可观测的任务状态，不能撤销已接受的竞价，也不能把外部成功当成本地事务已经提交的证明。

论坛装备标色影响帖子名称展示，同一判定同时产生公开页的稀有/融合分组。类型、种类、品质的多选条件保持在单条规则内，各维度 OR、维度间 AND，空数组沿用旧单值、两者均空为任意；同维新旧选择同时非空或非法重复条目失败关闭。空数组省略序列化，保留旧 runtime 文档指纹与命令重放；规则仍按顺序首个前后缀命中，不改变装备采集接受政策、身份事实和经济计算。Public/Worker 启动冻结同源配置，新字段须由支持该契约的程序读取并重启生效。分组在应用层统一判定，Peerless 固定彩虹也归稀有；读取分组筛选必须先于分页，计数与页面保持同一快照。分组只作展示和查询，不写入装备身份、拍卖状态、审计或经济事实。
只有新 `equipment_types` 数组赋予十二种护甲材质族匹配语义：实际装备大类须与材质所属 Cloth/Light/Heavy 一致，完整种类以该材质首词、空格和非空部位组成；其他数组值与旧种类单值均精确匹配，不采用前缀子串或放宽采集接受规则。

论坛系统更新楼的拒绝提示只按持久错误码生成安全英文，区分已领先、金额过低/无效、截止、资格及状态等原因；原始有界诊断仍只留本地账本，不输出 SQL、凭据、代理上限或任意错误原文。旧 `command_rejected` 或空码只对完整白名单诊断格式做展示映射，金额诊断须含规范 i64，数值不回显；未知新码不得借旧详情重新解释，统一返回通用提示。展示映射不改历史记录、不重放拒绝出价，pending 仍表示将重试且阻止完成水位；正常镜像更新仍受活动主题、租约、写入证明及未知结果门禁约束。

主题标题采用 `[Auction] {Main|Isekai} Equipment Auction - Session {场次号，至少三位} ({真实拍卖状态})`，状态使用固定英文闭集：
Draft、Preview、Live、Final、Settling、Settled、Cancelled。本地校验上限为 80 字符，不截断场次身份；这不代表远端已验证接受该长度。
当前标题及最多二十二个旧/其他状态候选仅作精确对账；包括其余六种新状态、七个旧无前缀状态、七个旧短标题、合法长度的旧无状态完整标题和旧 HV# 摘要标题。
多个候选指向不同主题时失败关闭，不取第一个结果继续写入。状态变化不改主题身份，也不授权重复发帖。

论坛目录按分类/编号稳定排序，每帖最多 150 件，#1 起连续分页；系统更新楼为目录页数加一，竞价起点再加一。每个目录页独立受 65,531 字节上限约束，不能静默丢弃装备；历史已尝试/发布/未知主题冻结为单页并继续处理出价，不强行分页，只有从未尝试且无远端身份的队列项及新主题启用分页。已发布楼层身份冻结，抢楼、缺页、布局与冻结目标冲突失败关闭，不能为了插入目录页覆盖日志或用户回帖；全部正文证明同事务收口，续页与系统帖不参与用户竞价或格式报错。更新楼采用静态白名单提示，保留 post/line 定位，区分 pending、警告、忽略；格式错误要求新增回帖，pending 不要求重发。通知处理可能晚于竞价成功，不能把通知 pending 或失败解释为竞价未接受；日志最多展示 100 条且受正文预算限制，优先 pending、再展示最近警告，省略数量必须明确，不能以省略诊断推进真实处理水位。
普通回复的位置不是命令身份：已接受命令按稳定 post ID 与原业务负载重放，历史 post_number（含 NULL）不重新编号。
pending 边界按当前完整帖子集合映射，删除或未收口仍阻断；经固定布局、完整扫描和 claim/revision CAS 证明后，
`processed_through_post_number` 可随前序删帖下降，这是当前观察投影变化，不是事实 sequence 或命令账本回退。

结算终态只消费已保存的结算汇总/明细和最终领先事实，不按墙钟或舍入时长推断状态、金额或百分比。取消只刷新已有有效主题的状态，不创建新主题或导入新竞价。`000043` 的历史 settled 豁免保持不变；纳入的 Settled/Cancelled 主题停止新竞价导入，既有 pending 保留诊断并阻止完成。所有固定目录页、首帖元数据和日志确认后才能持久化所渲染的终态版本，存储在 auction 锁和 sync lease/fencing/revision CAS 下复核；扫描期间进入终态也不能跳过最终确认。扫描未完成时只更新实际变化页，不因终态反复 POST 未变化内容，完成后停止轮询。未知写入仍须只读对账，部分进度和未知结果不得提前写入完成版本，冻结分页及正文上限不变。

所有固定楼层的发布、编辑和未知写入对账始终核对真实 thread/post 身份、固定楼层及当前拍卖/页码标记，新 POST 仍须精确帖子回执。`worker.forum.verify_post_body` 属部署配置，默认 `false`，统一控制同步及回帖刷新：关闭时不额外回读/逐字比较原始 BBCode，正常成功 POST 后的本地正文仅为目标镜像，不证明远端正文、价格或排版与其逐字一致，Public 仍是权威事实源，正常内容更新仍按本地目标变化执行。创建端口必须区分本轮成功 POST 与精确标题命中旧帖，后者不是新正文发送证明；关闭正文校验时，无人工授权的既有固定帖不能被当作本轮目标完成，避免首次领取也漏掉未发送的变化。
创建主题的 HTTP 200 页面也必须包含唯一确认标题、唯一 meta refresh 和一致的同源主题证据；
普通错误页中的旧主题链接不构成创建成功，缺失或冲突证明保持 `write_unknown`，不自动重发 POST。
历史未知结果不能只凭现存帖子 ID 收敛，否则可能吞掉恢复期间尚未发送的新镜像；关闭时同步转人工授权，回帖刷新保留未知门禁，不更新正文基线或完成水位，人工对账不能用关闭开关替代。开启时，写前、写后及未知结果恢复统一经 `forum_post_bodies_match` 比较原始 BBCode，仅忽略末尾 CR/LF，内部换行、空格和 BBCode 必须逐字一致，不得宽泛 trim、折叠空白或以本地镜像替代回读；渲染器按论坛保存形状输出列表边界，完整拍卖链接用内层粗体防止长网址显示文字被截短。未知写入错误含 `reconcile_` 前缀时仍使用只读对账的短退避；启动首轮水位可重新领取历史未知结果一次，但必须保留 claim 中的原失败码，禁止自动发帖、补楼或升级旧布局。
主题未找到、缺固定楼层或开启校验时正文不符仍进入人工授权/未知结果门禁；关闭开关和重启均不自动清空已持久化的 `write_authorization_required`。
启用回读校验时，首帖同时核对编辑表单中的真实标题/说明和原始正文；正文相同不能证明标题已更新。
普通标题单独变化仍须提交元数据更新；未知写入中的标题/说明不匹配仍保持只读对账或人工门禁，不能自动重发。
创建、回复、编辑共用真实 POST 进度标记：明确尚未外写的读取超时可普通重试；已经写入或历史未知结果仍只读对账，
不因本轮没有新 POST 解除人工门禁。论坛及 PM 的 world 资格必须来自实际 credential scope，并在 LIMIT 前过滤不可用世界。

普通邮件入站的 take/return/notice 即使已经写入 delivery outbox，也必须等 `inbound_mail` 与 `mail_commands` 完成事实提交，并由入站决定中的精确 delivery ID 授权后才能领取或进入 sending；复用 MailIntake origin 的论坛通知通过同 command ID/world 的论坛 notify owner 证明，不能用任意模板名绕过门禁；投递命令摘要与邮件内容摘要属于不同命令，不能要求二者相等。拍卖收件必须在已接受装备的持久化边界保留可恢复的决定，后续 Take 入队失败时重试只能恢复已接受的入站结果，不能重新检查出重复装备后转为拒绝或退件；未知主题中的纯 Credits 也属于资产，不能因装备附件为空就归档为无资产邮件。Take/Return 入队与入库都在 Serializable 边界核对相反意图，历史冲突在领取及发送前继续阻断，并以 `mail_disposition_conflict` 保留原负载进入人工核对；该隔离只允许人工终结为失败，不能确认成功或重试，失败终结也不解除原资产互斥，后续处理必须另行核对权威库存与远端证据。

### I-09 Worker 进度必须持久化

Worker 的命令账本、outbox、消费游标、领取状态、重试次数和失败原因必须能在重启后恢复；内存定时器、内存游标或单进程广播不能作为唯一事实。重试必须幂等，超过策略上限的任务进入明确的失败/人工处理状态。

慢速代理的有界扫描在 LIMIT 前按数据库时间排除已截止 lot，避免 Final 未推进的过期授权占满批次；无锁筛选不替代执行端重读 generation、授权、资格和截止 fence；竞态中的过期候选可拒绝，但不得通过清空调度或伪造成功跳过生命周期所有权。Final 状态与对应调度完成 fence 必须原子提交；带冻结来源的后继周次按本地日历计算，不能以 UTC 时间直接加七天跨越夏令时；历史及没有时区输入的手工排期保持既有 UTC 周期，不猜测来源。进入 drain 后停止领取新任务，已经开始的任务仍按原租约和共享的有界关闭预算收口，单组错误不提前取消其他组；论坛临时存储错误必须阻止当前楼层及后续楼层推进，只有明确的永久身份拒绝才能作为该楼层的处理结果。

邮件读取预算内只返回已校验的连续详情前缀，列表未定位到 cursor 或完整末尾时不得推进；重读已取件邮件时先核验持久化完成证明，不能用已变化的远端资产重新解释已完成业务。既有事实和账本仍严格保留原摘要，缺失或损坏证明不能跳过，游标只越过已获证明或本轮成功提交的前缀；处理失败后缀不得抹掉完成前缀，前缀、游标与失败退避须在同一 token/version/lease fence 下原子提交。
邮件续租与失败退避必须先锁定邮箱，再用数据库 `clock_timestamp()` 复核租约；不能使用调用方时间或事务开始时间
延长已过期 claim。入站提交的最终 cursor CAS 同样检查实时有效期，过期时回滚同事务的事实、完成前缀及队列删除，
不复活租约、不释放未知外部写入，也不绕过原 token、owner、版本和暂停快照。
论坛内容/发布确认、告警批次、recovery action、履约失败和 PM 终态同样遵守先锁行、后复核实时租约。
在首次 UPDATE 的扫描谓词内使用 `clock_timestamp()` 仍可能先判断再等锁，不能冒充锁后有效性证明；
原 owner/token/fencing、业务锁序及未知外写门禁不变。
Worker 在邮件领取与续租发起前记录单调截止，回执只能收紧剩余预算，不能补回数据库或网络等待耗时；
旧租约到期、续租失败或回执已过新截止时取消陈旧处理。论坛发布和回帖导入的领取与处理共用同一单项截止，
超时不重置预算、不自动重寄可能已写出的外部请求。
生命周期、结算、履约与 recovery action 的本地排队也计入领取前记录的绝对截止；首次 poll 已过期则不执行命令。
Delivery 续租回执超过本次 finalize 或旧租约截止时必须拒绝，即使回执携带的新 TTL 尚未耗尽也不能恢复本地 owner。

诊断是非权威投影：完整装备响应先加密 prepare，失败才在 2 秒预算内 retain，成功即丢弃；每封邮件最多挂接首个失败 sidecar，不能进入 `decision_metadata`、command digest、completion proof 或 cursor，没有历史 trace 不补因、不重放。留证必须在 item/batch 业务定时器之外执行，实际耗时顺延后续 batch deadline；中间失败保留原 retryability，只有 terminal `OkRejected` 挂接 sidecar。留证关闭、失败或完成未知时仍输出安全关联日志，不伪称取得完整 body 或保证文件永久存在；阶段、字段闭集、HTTP 状态与计时范围、回执和调用顺序统一见[装备诊断契约](ops/mail/diagnostics.md#装备读取失败诊断)。

每个已配置世界的 Inbox、Read Mail 和 Sent/草稿维护并行且分别有界；维护自身的 fetch 总预算覆盖数据库、session 排队、逐封详情及 compose/discard，一条路径失败不撤销其他路径已提交的事实。
`000060` 的维护游标只在单条开始核对前持久推进，最多 200 条候选和 20 条通知/Take 边界不得跳过未处理后缀；
到尾稳定回绕，不把观察、轮转或超时条目记为业务完成。discard 前先持久登记一次性尝试，再于同一 session 重读并精确比较原草稿；
尝试提交未知、取消或外写未知都不自动再次 discard，更不授权重发 Send。

### I-09a Operations alert 的通知冷却不能早于入队证明

Operations alert 批次创建时即持有第一次 claim，`claim_fencing` 与 `attempt_count` 从 1 开始；lease 过期或 fencing 不匹配的 Worker 不得确认入队或释放 claim。普通 delivery Create 失败只把批次释放为错误码为空的 `pending` 并设置持久化退避，identity 不一致立即进入 `failed/dependency_mismatch`，第三次失败或第三次 lease 过期进入 `failed/retry_exhausted`。signal 的 `last_notified_at`、`next_notify_at` 和版本只能在批次、delivery 同一事务确认入队后推进，领取或 Create 失败不能提前消耗 cooldown；
delivery 的外部失败与人工复核只投影为稳定的 `delivery_failed` / `delivery_manual_reconcile`，不得把任意远端错误原文写入告警健康投影。scan、dispatch 和 reconcile 必须独立调度并各自受有界 deadline 约束，一个 owner 的读取、领取或写回失败不得阻塞另外两类 owner；默认一分钟 cadence、阈值、批次窗口、冷却、重试与正文边界属于当前运行契约，修改时必须显式评审告警灵敏度和通知频率。

### I-09b MoogleMail 破坏性动作必须有前后双重证明

return/take 只能在已重新读取的详情页上执行，动作前必须证明 sender 与当前 delivery 收件人一致；take 还必须精确比对装备 ID 集合、Credits、COD、附件条目数和 claimable 动作，只检查按钮存在不构成资产证明；前置证明失败时不得发起 POST，也不得将未知远端状态当作可自动重试的确定失败。

POST 后，return 必须取得唯一只读 `[RTS]` 回执，并与动作前 To/From/Subject、正文、装备 ID、Credits、COD 和附件数逐项绑定，精确证明收发件人反转、固定 MoogleMail 发件人、标题前缀、正文退回后缀、资产不变且 return/take 动作都已消失。详情 GET 会使邮件转为已读，目标 `mid` 不在未读 Inbox（包括完整末页）不能证明已退回，不再作为兼容成功证明。take 必须证明 To/From/Subject 不变、资产与 COD 归零、claimable 消失，且动作前正文是动作后正文的前缀，预期的 `CoD Paid` 和 `Removed attachment` 回执数量与金额精确一致。仅出现“成功”文字、`[RTS]` 标题或动作按钮消失都不足以证明完成；后置证明失败必须进入 `manual_reconcile`，不得盲目重发。
未知 Take 的维护恢复须先取得 `000063` 冻结的原 world/MID 与持久 credential ID/version，再解释同账号的严格邮件缺失证明或完整详情资产归零。
当前观察 generation 在恢复事务中锁验，原尝试按 sending_at/command_id/request_digest 定位，候选当前 fencing version 另作 CAS；
原 generation 不要求与重启后的当前 generation 相同。旧缺原绑定、配置 Cookie 无持久身份、换账号/版本或过期观察均保留人工核对。
仅原装备集合与金额完全相同才允许原有有界重排；部分、无关、额外或重复资产及金额漂移均不成功、不重排。
不能把“目标装备交集为空”或另一账号的“邮件不存在”解释为取件成功。重排清除旧 claimed_at 等执行时间，不生成新命令身份。

PostgreSQL 流拍的 `unsold_return` 与撤拍的 `withdrawn_return` 默认必须逐件 compose Send：使用原 lot/equipment 身份、原世界和冻结卖家，Credits/COD 均为零，暂存与 Sent 证据只能包含这一件原装备。收件人显示名命中 Admin runtime 的 `bot_players` 时，两者都直接内部留存并确认完成，不生成寄回投递。拍卖收件已 Take 的装备不能通过原邮件 Return 找回，也不能因一个 lot 撤拍而退整封多附件邮件。本地事务仅原子记录义务，外部寄送在提交后执行；稳定 delivery/prepare/activate 身份、租约和未知结果门禁保持不变，历史已冻结的 Return 请求不得自动改写为 Send 或重寄。

### I-09c MoogleMail 明确发送失败必须保留资产后果

send 只能从能证明为无收件人、正文、附件和 COD 残留的干净 compose 页开始，无法证明时必须进入 `preflight_unproven`
人工对账，不得覆盖或自动清理可能属于上一次结果未知的资产。明确拒绝只采信唯一 `#messagebox_inner` 的可见协议提示；
系统框外文本、script、style、noscript 和邮件 textarea 中的同名短语不能证明拒绝。
收件人不存在的拒绝仍须保留唯一、合法的退信 `mid`、`take_required` 和稳定 reason code，
退信资产取回前进入人工对账，不得直接终结为普通失败。
Admin 只对上述精确 `recipient_not_found/manual_reconcile` 形状接受退信 MID 作为外部引用，
它必须与 `returned_mail_id` 一致且保留取回责任、原执行时间链及安全 transport 元数据；退信引用不是 Sent 成功证明。

Worker 收到明确 `recipient_not_found` 后，必须只在当前领取轮次内暂时排除同一 `(world, recipient)` 的后续 delivery，并继续处理其他玩家；排除集合不得跨 world、不得批量改写未尝试 delivery，也不得形成永久黑名单，下一轮允许重新评估。首条失败 delivery 仍独立保留 `manual_reconcile`、退信引用和资产回收责任，禁止自动重发。明确的邮资拒绝只在 compose 中从未暂存 Credits、装备或 COD 时可以作为确定失败；任一资产暂存已经通过证明后，拒绝结果必须进入 `manual_reconcile`，禁止自动重发。失败 transport 元数据只能是有界的安全 object，storage owner 必须覆盖而不是信任其 `safe_detail` / `outcome_unknown`。普通日志、数据库和 DTO 不得保存原始响应、Cookie 或请求 payload，但业务拒绝、业务失败、处理超时以及 HTTP/解析异常可以按[邮件诊断](ops/mail/diagnostics.md)写入独立的加密异常证据，证据保存失败不得改变原有退回、投递、重试、游标或幂等结果。

所有主动 ComposeSend（通知、注册/重绑 key、告警、结算/返还）在任意 POST 前完成覆盖所有 MID 的 Sent baseline，POST 后独立有界扫描；只有新增且唯一的真实 MID 与 world、recipient、subject、canonical body digest、Credits、COD、装备 ID 精确匹配才可成功，零资产邮件同样需要证明；写后资产不符、秒领取、无匹配、多匹配、扫描不完整或超时均进入 `manual_reconcile`，不自动重寄；fence 前完整基线失败只释放未发送的有效 claimed。成功页、空 compose、本地 UUID 或 Admin 提交的裸 `external_reference` 均不能替代证明。成功 metadata、默认 180 秒 delivery timeout/300 秒 sending grace 及预算约束见[Sent 证明](ops/mail/diagnostics.md#主动-composesend-的-sent-证明)。storage defense gate 对 Worker 和 Admin 同样生效，Admin `retry`/`failed` 仍需显式命令、版本 fence 和审计；Return/Take 使用独立 action proof 和人工核对路径，不套用 ComposeSend 证明。

主动发送包含装备时，必须在既有 sending fence 和同一独占凭据会话内、暂存 Credits／装备／COD 前，
完成七类 Armory 库存的精确 EID 唯一定位和状态校验；原生显示名中的状态图标不能作为真实状态。
仅对本次冻结装备集合中已证明在仓库或受保护／锁定的条目执行显式出库／解除操作，不自动卸下已穿戴装备。
每次 Armory POST 后重新读取同分类，确认原目标身份、未穿戴及所需状态，再使用新 token 继续；
任一次外写后证据缺失、身份／状态不符、失效或超时均保留未知结果，禁止自动重试、回滚或跳过该装备寄送。
装备准备实际写入后还须重新证明 compose 干净；出库或解锁成功本身不构成邮件成功，最终附件与 Sent 证明不变。
具体字段、边界及人工处理见[附件装备状态准备](ops/mail/diagnostics.md#附件装备状态准备)。

Delivery claim 必须在进入 `sending` 前按 payload world 检查 scoped/shared 凭据：当前无凭据时只能用完整 lease token 和 fencing 从 `claimed` 释放回 `pending`，不能写成终态失败；非法 payload 仍必须越过 availability gate 进入确定失败路径，避免永久滞留；进入 `sending` 后发生的凭据竞态、网络、响应边界或协议依赖失败不得收口为普通失败。本轮必须跳过已确认无凭据的 world 并继续领取其他 world，不可用队头不得阻塞有凭据的投递。

MoogleMail Send 在 sending fence 前实际 GET compose 并完整扫描 Sent 至尾页；页数/时间耗尽分别为 `sent_scan_page_limit`/`sent_scan_timeout`，不得当空历史。持有同一 Cookie 独占 guard 进入消耗式 prepared future；fence 未明确成功不得 poll 写入。prepare 在原请求总预算内为 claimed 周期续租，首次准备前及 fence 前后均确认续租；数据库等待受 finalize_timeout 和已知单调租约截止双重限制，回执已耗尽新 TTL 不得继续。首次外部 poll 前直接复核单调截止，超时/租约/续租处理优先；未 poll 的失败先 drop prepared，再尽力释放，不调用外部 unknown 结果落库；已 sending 或 fence 提交未知不能退回 pending，仍沿持久隔离恢复边界处理。只读准备失败/超时释放有效 claimed，事务内撤销本次 attempt_count 并追加 release 审计，不写 unknown；仅暂排该 world 的 Send，不占发送批次，Take/Return 仍独立；下轮重新检查实际草稿，禁止永久历史错误 SQL gate；发送 fence 后仍严格单次，旧 sending/manual_reconcile/outcome_unknown 不因新草稿干净而重发或结清。

普通 Inbox/Read Mail/Sent 观察不取得 compose/资产操作 guard，但必须绑定当前凭据与有效 owner；跨凭据、世界、
读取代次或发送后时序屏障不得复用旧响应。完整列表窗口采集与可能改变已读状态的详情 GET 分阶段协调，
详情前仍须持久登记待读 MID；Read Mail 只定位已知恢复任务，不把历史已读邮件作为新业务。
共享观察只减少重复读取，不是资金/库存授权；内部证据仍遵守 I-09h 的原会话、generation 和绝对截止。
所有新 Send 在围栏前渲染一次，实际发送消费原正文；`000061` 将主题、正文摘要、资产矩阵与尝试身份同事务冻结，
不持久化登录 key 明文或含密钥正文。确定的非法载荷在有效 claimed 下失败收口，不能伪造 Sending 或永久重领坏任务。

预期 `player_name`、Cookie 可用或 Item Shop 余额均非自身身份证明，来源未证明时明确 `identity_unproven`；数据库旧预期名为 None 也必须远端联合核验，非空预期值精确匹配；内部履约与现金门见 I-09h，业务规则及原子写门禁见[应用履约](crates/auction-application/README.md#结算与恢复端口)。

可能超过初始 lease 的多步外部写必须周期续租，并在每次续租和最终写回中携带同一 lease token 与 fencing version。Worker 只能在有空闲执行槽时领取下一条，不能让批次内排队时间消耗新 claim 的租约；完整外部调用与终态写回必须分别有界，请求超时属于结果未知。续租失败后不得以 stale claimant 保存成功或确定失败，只有原 fence 仍有效时可以写入结果未知；过期 `sending` 必须在额外 sending grace 后进入 `manual_reconcile`，禁止重新领取并盲目重发。Delivery 普通轮询使用有界 `poll + [0, jitter]`；新超价 MM 提交后的空载荷提示可即时唤醒并优先领取，在途工作不抢占。提示不携带任务或发送授权，丢失/断线仍靠周期领取，准备失败释放不循环自唤醒；单轮 claim、持久化或外部处理错误必须由持久化状态保留重试边界并在下一轮继续，不能因此终止整个 Worker，循环任务异常退出仍必须失败关闭。

同类 Send 的最小开始间隔按七类、跨 Main/Isekai 共用，起点为持久 sending fence。`000059` 的分组高水位与 Sending/审计同事务推进，
准备失败或确定 Deferred 不消费间隔，发送失败/未知不退还；重启不清空。锁前缀为分组节奏 → 原 generation/world/资金及业务锁，
同事务再次按数据库实时钟复核，不依赖领取时快照或内存定时器。Take/Return 不参与分组节奏；0 不绕过原串行、限速、资格与证明。

### I-09d Delivery 状态事实与审计不能分叉

Delivery 的创建/准备/激活、claim、进入发送、释放、成功、失败、结果未知、过期发送隔离和 Admin 人工动作必须在同一 PostgreSQL 事务追加安全审计，审计失败时状态变化必须回滚。两阶段激活必须同时持久化独立的 activation command ID、请求摘要和激活时间，所有后续状态上的幂等重放都必须匹配该身份，幂等重放本身不得重复追加审计。续租不改变业务状态，不逐次生成审计噪声；续租只接受同 token/fencing 的有效 claimed/sending，先取得 delivery 行锁，再以数据库 clock_timestamp 及调用时刻核对旧截止仍有效、新截止仍在未来，且不得缩短原租约；过期、旧 unknown 或更换 owner 的 claim 不得复活。start_sending 保持 lot → delivery 锁序，续租/释放不倒取 lot 锁。

三页维护依据精确证明绑定恢复结果，也必须在同一事务追加 Delivery 审计；有持久资金分配的已证明 Send 同事务把
`unknown/in_flight` 收口为 `spent`，不能只修改 delivery。未影响任何行的幂等观察不追加重复审计，也不计作恢复成功。
新发送及冻结恢复还须原子建立唯一 world/MID 绑定；一个 MID 不能证明不同发送尝试成功。
`sent_recovered_frozen_v1` 必须关联原不可变摘要与尝试，不能仅凭 proof 标签进入履约或资金来源。
重复同内容候选、竞争尝试、缺页、立即领取后的空资产或旧凭据观察都不授权猜测绑定；历史缺证据不回填、不自动重寄。
冻结证明恢复付款提醒时，还须按 recovery→credentials→actions→delivery 的统一锁序恢复原 action 的未知结果投影，
使其重新领取并消费已经成功的原 delivery；不重排邮件，不覆盖其它动作或人工业务阻断，恢复和审计同事务。
已确认 COD 退件后的重排队，在清理旧结果前追加 `delivery.cod_return_resend` 审计，
保留原投递身份、fence、执行时间、真正已证明的引用及退件消费/次数关联；审计、清结果和 recovery 推进同事务提交。

### I-09e Fulfillment 与 Recovery 不得共享易失调度边界

PostgreSQL fulfillment 和 recovery 必须使用各自的持久化领取循环、批次、并发、lease、单项总时限、重试和 `poll + [0, jitter]` cadence；一个 owner 的领取、依赖或持久化失败只能影响自身下一轮，不能阻塞其它 owner 或终止整个 Worker，循环任务 panic 或意外退出仍必须失败关闭。Fulfillment 只能按并发波次领取，避免尚未开始的工作提前耗尽 lease；Recovery 必须按 `case → return → action → relist → refund` 依次尝试全部 phase，单个 phase 失败不能跳过后续 phase，action 仍由 fencing 和持久化重试保护。默认 60 秒轮询、5 秒抖动、100 条批次、8 并发、60 秒 lease/重试和 30 秒单项时限属于当前运行契约；等待买家支付 COD 或卖家领取货款时使用按绝对时间对齐的共享窗口（`worker.fulfillment.evidence_poll_seconds`，默认 300 秒）：同一窗口内到期的所有 COD/货款履约一起领取读取，窗口内新增的履约会等到下一个窗口，不各自建立五分钟计时；普通失败重试节奏不变。

已进入 `enqueued` 的投递以 prepare 时冻结的收件人及原 delivery/Sent 身份继续核对；账号之后改名不能重定向已发邮件，
也不能把已付款事实降为冲突。身份存在性使用稳定 user UUID，pending 新投递才可按当前名称解析实际收件人。

退件消费必须以入站邮件唯一身份追加持久事实，与退回计数、恢复阶段及后续 action 同事务提交；重寄清空当前
`return_mail_id` 不解除原邮件的消费。迁移只回填可证明已消费的来源，不自动修正既有计数。每次真实退件的 take
prepare 与 activate 必须拥有独立且同轮稳定的命令身份，不能随 claim、租约或重试次数改变；旧 prepared/enqueued
继续原身份，旧 prepare 已提交但未关联 action 时先完整核对原投递，不能为同一 MID 另建 take。
recovery v1 每次 COD 重寄沿用冻结的 604800 秒付款窗口，不从已经延期的 deadline 减最初 created_at 重新推导时长。

### I-09f Auction 生命周期与结算必须独立于集成轮询

Auction 生命周期和结算 owner 必须立即执行首轮，再使用自己的 `poll + [0, jitter]` cadence；邮件唤醒、论坛、realtime 或其它集成 tick 不得额外触发 Auction，也不得延迟它。生命周期与结算各自拥有完整 batch 上限并按共同并发波次领取，一类 claim 或单项失败仍必须尝试另一类，且只影响主循环下一轮。默认 60 秒轮询、5 秒抖动、每类 100 条、并发 8 和 30 秒单项总时限属于当前运行契约。

### I-09g 装备标记与超价提醒按领先周期去重

装备标记仅属于已认证用户，user UUID 来自服务端会话。标记/取消使用稳定 `command_id`、请求摘要、版本 CAS、追加审计和与竞价一致的 lot 锁序；相同命令重放原结果，不同负载冲突。取消保留版本墓碑，不同新命令即使设置相同布尔值也须推进版本 fence，阻止更早的未知请求晚到覆盖；旧命令重放不推进版本，不允许旧缓存、旧请求或 GET 恢复标记。服务器是唯一权威，本地缓存只作同账号、同拍卖的可丢弃展示副本。

普通竞价不会自动标记；新代理创建成功时同事务默认标记，降额、自动追价、刷新和重放旧命令不重新标记；追加迁移仅给已有有效代理补建标记，不补发过去的超价通知，不重写竞价历史。

通知只由已标记用户在本 lot 的真实“领先 → 被他人超越”事实触发，同一领先周期至多形成一个确定性通知身份；标记时已经落后、仅浏览收藏以及其他人继续加价不触发通知，重新领先后才开启下一周期。普通、论坛、快/慢代理共享该边界，必须处理一个事务内多条事实，不能只轮询最终领先者而漏记反超。通知事实及投递绑定与对应竞价同事务提交，外部发送不可用不得撤销已接受竞价，SQL 错误仍整体回滚，不吞持久失败。账号统一偏好默认 MM，可选择 PM，不同时双发；同 lot/user/cycle 的唯一身份不含渠道。偏好 PUT 只锁 user，Serializable 先重放账本再 CAS，同渠道新 ID 也推进独立正 i64 版本，最多三次尝试，保留 40001/40P01；竞价按既有 Serializable 快照冻结偏好，不在 facts 末尾追加旧 leader 的无序 user 锁。旧 watch.channel 永远 mm 兼容占位，切换偏好不改变旧 MM 或 PM 的来源证明，不重路由已排队通知；PM 收件人只来自稳定 forum_account_id，缺失/非法形成 recipient NULL 的 suppressed/recipient_missing 事实，不阻断竞价，不回退 MM。

取消标记或重新领先会使尚未开始发送的旧提醒失效，已经进入发送的邮件无法撤回。领取与发送前复核独立的标记/通知 owner 证明，不复用论坛 notify 的来源授权；稳定身份、唯一约束、持久状态和既有 delivery 租约共同防止重复入队，外部结果未知仍按 `manual_reconcile` 处理，不自动重寄，也不宣称第三方 exactly-once。邮件只包含公开竞价信息，不含代理秘密上限、装备 key 或凭据。失效且尚未进入 sending 的 pending/claimed 提醒由 delivery owner 有界收口为 failed，原因及失败码固定 `lot_watch_invalidated`，追加审计并保留来源证据，不触碰 sending 或 manual_reconcile；运维投影只对该 watch 专属原因接受无 sending 的 preflight 形状，并要求无租约/外部引用、零 COD、有序时间及明确 `outcome_unknown=false`/`preflight_blocked=true` 元数据。Admin release、retry 和人工 reconcile-retry 也先锁 lot，再锁 delivery 并复核同一独立 proof，不得恢复取消或旧领先周期的任务。

PM 不进入 MM deliveries/Sent/COD 链，独立队列只有 pending/claimed/sending/succeeded/suppressed/manual_reconcile。prepare 仅 GET；开始 sending 必须先锁 lot 再锁任务，复核冻结 payload/digest/recipient/channel、watch/version、永久失效标记、未重新领先、lease/fence 及准备得到的 sender，明确提交成功后才消费 prepared.send 一次。只有从未 sending 的领取可安全延期；任何 sending 永不回 pending/claimed/suppressed，sender 与 sending_at 不可改变。
超时、drain 取消、进程崩溃、过期 sending 都只核对原 sender 的 Sent 副本，找不到不是未发送证明，无自动/人工重寄；精确 receipt 必须绑定完整 message、recipient、sender、Sent MSID；取消不覆盖已发送/未知结果。CLI reconcile 只以稳定命令和审计申请 Worker 只读核对，未证明保持 manual_reconcile，不初始化第二个凭据客户端。PM runtime 共享唯一论坛网关/凭据 owner/HTTP 限速，不依赖 MM enabled，forum.enabled=false 不外呼；每轮最多 1 新发送+1 只读核对，串行，使用 delivery cadence/jitter 和 forum 单项总超时/lease，总预算严格小于租约；
未知 fence 提交不发送，预算到期停止工作并由持久租约恢复，而非将未知写入安全延期。

### I-09h Bot 内部履约与现金保护

账户以持久策略区分 `strict_cash` 与 `record_only`，缺失账户及旧账户默认前者。
下文的整号现金覆盖、专用账号及自有额度门禁适用于 `strict_cash`；共同的身份、装备、义务、精确投递、幂等及审计规则适用于两者。

- 身份来自同一 MM Cookie 认证 HV UID 与该 UID 的论坛目标资料名；论坛 Cookie 只提供读取权限，不取页头登录者。forum.enabled 关闭或证据缺失明确延期，不暗中启用论坛，不把 MM Cookie 发往论坛。
- 当前持有只接受批准七类 Armory organize 的精确 world/form/完整计数、非 disabled canonical eqids[] 及同 EID 行 handler，同 session compose 必须干净，且同 EID/world 历史出站、恢复及其他义务占用无冲突；详情可读不是库存证明。Isekai 独立读取且与 Main 同 UID，未知形状仅阻断相应分支，不复制 Main 库存。资产冲突依据真实 origin/payload 闭集 schema：已验证无装备通知不构成 EID 占用，attachment_ids 按外部 EID/world，secret 引用按实际装备 EID/world 核对；缺失/畸形/未知 schema、真实同资产占用仍失败关闭，不凭 succeeded 放行，不解除旧 unknown，保留精确 recovered_handoff 例外与来源关联检查。
- 内部提交要求数据库 active credential ID/version、全局 mail_cookie generation 与最长 30 秒的新鲜证据；原 operation guard 持有到事务确定结束，开始可能提交后取消或结果未知同步撤销 owner，先查原稳定命令，不转外发。所有取得 mail owner 的新版程序（含配置 Cookie）先推进 generation 并提交，配置/keyenv 不伪造持久凭据身份。
- BuyerRefund 的新 pending 和旧 prepared 在准备/激活时都必须取得同账号、同装备世界的 Armory 正向库存证明；
  opaque guard 持有到事务确定结束，原单调截止覆盖连接池等待、重试与 COMMIT，数据库时效另行复核。
  公开详情可读、无资金账户或历史 prepared 均不能替代库存证明，也不能把不确定提交伪装成已回滚。
- 数据库统一锁序为（Send 独有的分组节奏锁 →）generation → 排序 worlds → 资金 scope/account → auction → recovery → lot/equipment → credentials → users → 排序 obligations → recovery actions → 排序 deliveries → 排序 fulfillments → allocation/proof/ledger/audit；可省无关锁，不反锁；远端读取在业务事务外；SQLSTATE 40001/40P01 保留类型、最多三次整事务尝试，每次复核实时租约与证据。
- 仅从未 prepare/activate/delivery 的新 pending 可内部满足；原稳定命令重放、资金动作、独立证明、satisfied、清 claim/版本、审计与直接 blocked 依赖解除同事务；prepared/enqueued/unknown 不转换，contingent 退款不自动激活。保持 B+C=S+T、原义务 ID 和 payment→handoff→seller；历史 `write_unknown` 投递只允许在 Sent 中 reference 与收件人唯一匹配、金额/资产矩阵一致时绑定 `sent_recovered_reference_v1` 并重新排队，绝不重发邮件；内部 carrier 不派发、不虚构 1C、无伪 delivery 或 Sent。bot 流拍与撤拍均直接留装备；配置在 `bot_players` 中的买家按内部 Purchase 分配真实 B/C，卖方 S 转 retained 或正常寄出；新配置只影响尚未生成 delivery 的新结算。
- 资金账户固定认证 UID/Main/Credits，Cookie 换代不清余额或占用；普通 Balance/理论 pool 不是自有额度；preview 数据库零写、不取 owner、不计算已证明可用额度。apply 经真实现金及完整责任核对、版本/摘要和精确确认才开额度；旧无 UID 责任只保守隔离；新外部 COD 须同 session 精确 Sent-collected、原 exact-payload/冻结金额及唯一来源，且发送晚于 opening 边界、发行后占用不超过新鲜现金；历史/跨边界来源继续等待，不释放 quarantine。未资金化 Main 装备 COD/跨世界现金 carrier 已外发而收款事实未闭合，或关联 delivery 缺失，均阻断 preview/apply opening；不从本地未 satisfied 推断未收款，确定尚未外发的 pending/prepared 付款不误判为已收责任。补偿及隔离新增需求有零下界，既有 allocation 全计占用；collected 在满足/发行/解依赖之前，以已锁账户和同一有效 Main cash 检查既有占用，缺口确定提交冻结并返回 FundingDeferred，不满足义务、不发行、不解除依赖。
- retained、税与受限补偿不自动再投资；退款仅原恢复授权、同 UID 支付来源及装备恢复/卖方互斥责任闭合后转移原资金；原 internal 才恢复原 proprietary，外部 collected 内部退款转受限留存，不新增经济权利或内部取消退款。
- 旧 Bot 自寄仅通过单笔显式修复命令处理：原版本与完整摘要、真实同 UID 身份、干净 compose 和正向库存同事务核验，原投递、错误及全部指针快照形成只追加事实。流拍可留存；成交付款还要求明确自寄拒绝及原草稿清理证明，只恢复受持久 UID 约束的内部准备，不伪造付款、Sent 或收款，不降级为外发。旧实收的 Bot 卖家款须独立精确 Sent-collected 与当前现金证明，从已隔离责任逐标的转为留存，唯一目标防止双计，不重复发行旧 B 或增加自有额度。
- 现金发送的完整 Sent 基线先在原 Worker 准备总预算内完成；基线不是资金证据。其后首次读取联合资金证据前收紧到最长 30 秒，后续 prepared/交接/提交不重建期限，仍持有同一 session。
- 全部受保护现金 Send（含 carrier、generic、旧入口）按精确原义务/payload 预留；reserved→in_flight 与 sending fence 同事务，fence 成功及续租确认后才首次 poll。现金 fence 从 prepared 继承原单调绝对截止，等待/确认/存储预算取原证据、有效租约及 finalize 截止最小值，同一 commit guard 覆盖重试，不从交接重建 30 秒。数据库每次只用原剩余整毫秒与 UTC 证据窗口的交集设置 PostgreSQL 18 transaction_timeout，覆盖锁/审计/COMMIT；零预算拒绝，不以 0 禁用超时；旧有证据入口缺精确单调期限拒绝；过晚/错误/取消或提交未知撤销 owner，不 poll 发送。保留准备续租与慢续租公平性，无映射延期；余额缺口提交冻结且不发送。Sent 记 spent，失败/未知保留 unknown 占用；TTL、重启、人工 failed 不释放。无证据 legacy 只在同事务 scope 锁下排除所有保护账户和持久 allocation 后才可发送，缺 UID 不是其他账号证明。
- 有 allocation 的新 Release/RetryFailed/ReconcileRetry/ReconcileSucceeded/ReconcileFailed 均 Invalid，原成功命令先重放；无 allocation 的旧 retry 按 recovery scope→中间业务锁→actions→delivery→fulfillment/reset 预锁，不增加人工权限；本轮无资金未知重发或人工返还入口，旧 32 条未知投递与草稿仍须另行核对。

`record_only` 由显式命令以真实 UID、干净 compose、原责任预览、版本和摘要初始化，只记录拍卖资金责任，不采集或比较账号现金，允许账号所有人自行使用 Credits。
策略保存在 UID 账户并参与预览摘要；各进程从数据库读取，提交时在原 scope/account 锁下复核，不由 TOML 或命令默认值暗切换。
首次仅登记已知旧责任；内部购买及补偿按原标的冻结的 B/C 建立有限、唯一的记账来源，不要求人工补齐全局预算，保持 B+C=S+T。
真实跨世界外部付款的载体只按原 Isekai 义务及完整 canonical payload 登记每个 delivery 唯一的 1C 来源；内部付款仍不生成载体。
这些来源表示会计记录，不作为实际现金证明；旧实收认领仍需精确 Sent-collected，只转移原隔离责任，不重复记入收入。
个人用款或其他标的的现金未知结果不触发全局余额冻结；原标的未知占用、资产冲突、禁止无证据重发/返还及真实卖家投递继续保留。
本次仅提供缺失账户的显式初始化，已有 `strict_cash` 账户不通过该命令转换。

正式升级、账户策略及专用账号约束见[运维手册](ops/README.md#bot-内部履约与资金保护升级)。

### I-10 缓存不是权威数据源

缓存只能保存可重建数据；竞价金额、资格、幂等账本、锁、会话真值、审计证据和结算义务必须由数据库或明确的权威服务决定。缓存禁用、过期、丢失或不可用时，命令结果不能改变。

## 审计、凭据和事件

### I-11 审计与事件只允许追加

审计记录、领域事件和 outbox 记录不可更新或删除来“修正历史”，更正必须追加新的事实并说明关联关系；流序号不可复用，回放必须按序读取，检测到无法安全续读时要求读取权威快照。

Admin 审计读取只能消费追加式 `audit_records` 的安全投影：筛选和分页必须有界且保持时间/ID 的稳定排序，数字页的总数与记录应来自同一读取快照，keyset 页不得把未知总数伪造成精确值；metadata 必须是 JSON object，异常存储行应失败关闭，不得把原始数据库字段或潜在凭据直接发送到浏览器。

### I-12 敏感材料不得离开安全边界

会话签名密钥、论坛 Cookie、邮件 Cookie、数据库密码和其他凭据不得进入日志、HTTP DTO、事件 payload、快照、错误消息或提交的配置；错误只能返回可操作的分类和关联 ID，不能回显敏感 TOML 原文或连接串。

`mail_cookie` 与 `forum_cookie` 各自全局最多一条 active 记录，最终约束必须由 PostgreSQL 唯一索引保护，不能依赖可竞态的“先查后写”。Worker 必须按凭据类型持有进程生命周期 session advisory ownership，同类第二个 Worker 获取失败时应拒绝启动；远端确认失效后只允许幂等标记当前 Cookie 为 `invalid`，不得自动轮换到第二条。ownership 保持到 Worker 退出，因此替换 Cookie 后必须重启 Worker 完成交接；每次外部 HTTP 请求发送前必须有界检查原持锁 session，检查失败或取消后永久失效，不能换连接继续使用保留的 Cookie。同步可用性仅反映已知失效状态，不代替发送前检查；该检查不能撤销已到达远端的请求，多步写入失锁须保持结果未知，不能降为可盲重试失败。`.keyenv` 的 `hv-ck-key2` 只供显式隔离验证，不能成为运行时后备。

Users/MoogleMail 登录 key 信封的 AAD 必须绑定最终持久化用户 UUID，不能把候选新用户的信封写到既有用户；注册身份预读不授予写入权限，事务须重新核对 UUID，身份变化回滚后重新签发。撤销 key 不能授权新的 MM 换绑，但不改写此前完成命令的幂等账本结果。Admin 撤销/重签和 MoogleMail 重签必须在凭据变更事务内永久撤销旧 Public 会话，恢复 Key 不能复活旧 token；登录创建会话时须在数据库锁内复核认证时的用户版本，阻止换钥竞态。MM 注册/换绑的 Users 时间取本地处理时钟，不能以远端接收时间抬高；用户、论坛身份和 key 信封快照的更新时间不早于其创建时间及已存更新时间，历史偏快创建时间不猜时区重写，更新以旧值为下界，命令/审计事实时间保持原义。`000044` 仅对 MM 注册与论坛同步双重证据匹配的倒退用户追加更正审计并推进版本，无法证明的异常阻断迁移，读取不静默修正。

### I-12a Public WebSocket 不能成为无界资源或第二个命令入口

Public WebSocket 只推送已提交公开事件，客户端 Text/Binary 不能成为业务命令。必须限制帧大小、存活/写入时间、进程/用户连接及 discovery 槽，具体数值和关闭 code/reason 由[事件契约](docs/contracts/events.md#websocket)统一登记；配额先于缓冲区分配、随订阅归还，淘汰慢消费者不能提前释放，资源计数不参与竞价正确性。连接尝试限流与 mutation 分账，公开预检读取权威状态；settled 拍卖只允许既有连接接收终态事件。

首次推送前及后续有界周期只读复核原会话、账号/凭据状态及过期时间，不延长会话，Pong 不续命；失效、依赖错误或复核超时均失败关闭，不承诺跨实例同步踢线。浏览器不能把业务空闲当作断线，也不能引入未登记的应用心跳帧。

### I-12b Public 永久登录不绕过撤销与授权

Public 永久会话仅以成对 NULL 截止表示，仍要求未撤销、账号 active、Key active 并受原有并发上限约束。
有限时长固定从登录或明确保存时起计算，普通活动不续期；旧截止不回填为永久，过期或撤销令牌不能通过设置或重放复活。
保存按 user → session → 命令账本取锁，使用 Serializable 与最多三次尝试；锁后重新读取数据库实时时钟，写入谓词再次核对有效期，
在同一事务提交账号偏好及独立版本、当前会话截止、命令结果和追加审计，不修改其他会话或用户认证版本。
命令历史响应与当前真实截止可能不同，HTTP 必须另读当前会话生成 Cookie，不能用旧重放结果覆盖后续期限。
浏览器 Cookie 持久期不等于服务器授权期，数据库和认证边界仍为真值，Admin 会话策略独立不变。
普通会话 GET 永不写入或清除 Cookie。浏览器登录、退出、时长保存/重放和显式期限同步共用同源原生独占锁，
持锁后重新取当前身份；不支持锁时明确拒绝本次 Cookie 写操作。同步只影响浏览器持久期，不改变数据库期限，
不以迟到响应恢复旧身份，协议与同版升级要求见[P06](docs/contracts/routes.md#p06-会话读取与-cookie-同步)。

## 能力档位和模块 owner

当前仅支持 PostgreSQL；未装配或不可用的能力必须按 HTTP 契约明确报告，不能返回伪成功。已发布迁移及历史事实的保留边界见[数据库契约](docs/contracts/schema.md)。写入责任按以下边界分配：

| 责任 | owner |
| --- | --- |
| 领域规则、金额和状态机 | auction-domain |
| 用例、事务编排、命令端口 | auction-application |
| 稳定 DTO、事件信封和错误契约 | auction-contracts |
| SQL、事务、锁、迁移和命令账本 | auction-storage |
| HTTP/WS 映射 | auction-http |
| 重试、游标和外部副作用编排 | auction-jobs |
| 外部论坛、邮件和网关认证 | auction-integrations |

任何新增表、事件、写入查询或路由都必须只有一个 owner，并在架构文档中登记能力范围。

## 验收边界

格式、编译、静态分析和契约审计只能证明结构没有明显漂移，不能据此宣称生产行为已经验收。除非用户明确要求，不新增、不运行单元测试、集成测试、功能测试、端到端测试或浏览器测试；真实外部服务、并发、非空生产数据和切换前验证需另行授权后执行。
