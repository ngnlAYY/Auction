# auction-integrations

[文档导航](../../docs/README.md) · [架构](../../docs/architecture.md) · [契约](../../docs/contracts/README.md)

E-Hentai、MoogleMail、装备来源及密钥信封的外部适配器。

## 本页导航

- [源码入口](#源码入口)
- [职责与实现边界](#职责与实现边界)
- [HTTP 与协议解析](#http-与协议解析)
- [凭据装配与外部请求](#凭据装配与外部请求)
- [装备来源与密钥适配](#装备来源与密钥适配)
- [边界约束](#边界约束)
- [凭据 owner 与取消传播](#凭据-owner-与取消传播)
- [维护与验证](#维护与验证)

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/clients/http.rs](src/clients/http.rs) | 统一 reqwest/rustls 客户端 |
| [src/clients/ehentai](src/clients/ehentai) | 论坛认证、受限请求、主题/固定帖协议 |
| [src/clients/ehentai/client/pm](src/clients/ehentai/client/pm) | PM 新鲜表单准备、单次发送与同账号 Sent 精确证明 |
| [src/clients/mooglemail](src/clients/mooglemail) | 邮件读取、动作/发送证明与加密诊断 |
| [src/equipment](src/equipment) | 装备来源策略、页面解析与安全链接解析 |
| [src/credentials](src/credentials) | Cookie 信封与 Admin 只读认证探测 |
| [src/cryptobox](src/cryptobox)、[src/login_key](src/login_key) | 装备/登录 key 加密与短时解密 |
| [src/forum](src/forum) | 纯本地论坛解析与 BBCode 渲染 |

## 职责与实现边界

实现应用层外部能力端口，只返回校验过的事实或安全错误。
外部响应必须经过应用命令或持久化 Worker 操作，不能直接修改拍卖状态；本地提交后的外部失败不能撤销已接受竞价。

装备标记 delivery 使用独立 `lot_watch` 来源和冻结安全正文，沿用原 ComposeSend/Sent 精确证明及未知结果 manual_reconcile，
不新增部署模板键或配置开关。账号警告渠道由应用冻结；PM 使用独立 `ForumPmSender` 端口，不伪造 MoogleMail delivery/proof，不失败降级双发。论坛 `notify mm` 是一次性通知，不再宣称订阅超价提醒。

## HTTP 与协议解析

各客户端固定 origin/endpoint、禁止自动重定向，限制响应类型、字节数、请求间隔和总预算。
正文按块在追加前检查上限，超时或截断不伪称取得完整响应。用户正文中的“登录”等文字不能作为凭据失效证明。
HTML 解析使用 scraper 0.27 / selectors 0.38，移除旧 fxhash 停维依赖；关闭默认 CLI feature，保留 `errors`。
仅使用库 API，不以手写解析器替代协议边界。
依赖升级的编译和公告检查不证明真实论坛、邮件或装备页面已完成回归。

| 协议 | 必须保留的证明 |
| --- | --- |
| MoogleMail 读取 | 列表/详情身份与资产一致；完整定位 cursor 后按旧到新返回连续详情前缀 |
| ComposeSend | 首次 POST 前取得 Sent baseline，之后独立扫描；只接受新增、唯一且 world、收件人、正文摘要、Credits/COD、装备精确匹配的 MID |
| Return / Take | 写前核对 sender 与预期资产；Return 证明精确 MID 消失，Take 证明身份不变、资产归零及唯一回执 |
| 论坛读取 | 同 thread、规范分页、真实帖子/作者身份与完整页数；不能把部分主题作为完整命令源 |
| 论坛写入 | 同源表单与精确 board/thread/post；回执始终核对，原始正文额外确认由 Worker 的 `verify_post_body` 开关控制 |
| 论坛 PM | 新鲜 MID 表单绑定当前姓名与 sender，单次 POST 保存 Sent、不 tracking；完整有界遍历自己的同标题 Sent 后核验收件人 UID、完整正文、sender 与 Sent MSID |

Compose 成功页、空 compose 或本地 UUID 均不能替代 Sent 证明；资产不符、无匹配、多匹配或预算耗尽进入 `manual_reconcile`，
不自动重寄。发送前 compose 必须为空，避免覆盖未知写入遗留资产；收件人不存在的退信在资产取回前也保留人工对账。
流拍/撤拍按原世界、原卖家逐件零 Credits/COD Send 原装备，历史冻结 Return 在远端写入前隔离。
完整回执、诊断和人工核对要求见[邮件诊断](../../ops/mail-diagnostics.md)与[领域不变量](../../docs/contracts/invariants.md)。

## 凭据装配与外部请求

Worker 配置数据库凭据能力后读取唯一 active Cookie 并解密，暂缺时保持 degraded，不退回另一来源。
未配置该能力才使用敏感 TOML 或 `.keyenv` key1；key2 仅供显式隔离验证。每种 Cookie 全局最多一条 active，
world 专属 scope 只服务本世界，shared 可跨世界；失效后不自动换号。

MoogleMail gateway 的列表、限速、正文与失效处理共用绝对 deadline，详情预算耗尽可返回完整验证的最旧前缀；
尚未定位 cursor/末尾时不得推进。发送动作全程持有凭据级 guard，transport 并发上限为 1，读取不占用写 guard。

论坛完整主题额外最多四路分页读取，共享请求间隔与 32 MiB 聚合上限；作者身份只采信系统作者链接。
目录续页增加顺序 POST、布局回读及可选正文校验的成本；配置中的预算校验只是单目录页下界。
大场次需提高既有 `item_timeout_seconds`/`lease_duration_seconds`，保持 lease 大于 item；超时仍走未知写入门禁，不绕过限速。
创建端口明确返回本轮 `Created` 或只读命中的 `Existing`；同标题旧主题不是本轮新正文的成功回执。
论坛仅导入普通出价，代理行在账本前忽略。`forum/render.rs` 按分类/编号排序，每帖最多 150 件，从 #1 连续分页；
每页独立校验 65,531 字节上限，过长失败而不是截断装备。续页重复分类标题，保持编号及当前价来源不变。
旧主题按持久化 `legacy_single` 保持一页，不能从当前件数猜测或更换旧布局。
解析器只合并紧邻首帖、同作者且拍卖/页码标记匹配的目录续页，随后按完整目录解析用户命令；不按正文跳过帖子。
Worker 的 HTML 路径要求相同数字作者 ID；只读纯文本预览仅比较可选作者标签，不得用于远端命令导入。
系统更新楼紧接最后一页，用户回帖从再下一楼开始，不能猜测身份或进度。
`forum/render/log.rs` 分类显示 pending、警告、忽略记录及总数，优先 pending、再取最近记录，最多展示 100 条且受最终正文预算约束；
超出时明确显示已展示/总条数，不因报错累积阻断价格同步，也不把省略当作解决。
`forum/render/countdown.rs` 仅在首帖顶部生成 `List Timer` 与 `Auction Timer` 的 `[img]` 链接，使用已配置 Public origin
下的[自托管动态 SVG](../../docs/contracts/routes.md#p22-自托管论坛倒计时)，不请求或引用第三方服务。
图片链接只含拍卖 UUID 和固定计时器类型，不含凭据、任意时间参数或装备 key；未配置公开 origin 时只保留 UTC 文字。
List Timer 对应本场收件随正式开拍截止，已有 `opened_at` 时使用事实时间，否则使用 `scheduled_open_at`；
Auction Timer 使用计划截止 `scheduled_end_at`，不代替单件软截止。缺计划截止不生成第二张图片，主题备注保留 `Not scheduled`，不推算日期。
图片下不再重复输出计时器说明段；UTC 日期保留在图片内及首帖开拍/截止信息列表，规则与页尾保留官网权威状态提示。
已打开图片须刷新才能取得改期，不改变正文精确证明、分页长度或未知写入门禁。
固定 #1 顶部保留 World、Bidding opens、Opened、Bidding deadline；`Auction address` 使用原生 `[url=地址][b]地址[/b][/url]`
展示完整当期拍卖链接。内层粗体防止论坛将长网址文字截短后写回 BBCode，不能靠忽略省略号通过正文校验。
origin 由[Worker 部署配置](../../ops/config/README.md#论坛与邮件)提供，拍卖 UUID/world 路径不丢失。
`About system` 链接固定指向论坛说明主题 `301946`。列表边界直接生成论坛保存后的稳定换行形状，不靠放宽正文比较消除差异。
`forum/render/settlement.rs` 在成交行追加 `; winning for Xh / Yh; adjustment%`，仅消费已保存的结算投影；
X 为最终连续领先到有效截止，Y 为计划开拍至计划截止，小时整数舍入一位小数；adjustment_ppm 每 10,000 为 1%，最多四位小数、去尾零、正税负折扣、零为 0%。
不以展示时长重算曲线、不含补偿池额外抵扣；流拍无尾注。目录解析仅剥离该结构化尾注，再解析真实来源，不放宽用户竞价语法。
所有页使用同一渲染路径、冻结页数及原正文上限，缺失权威明细失败关闭。完整语义见[玩家指南](../../docs/player-guide.md#成交价为什么不一定等于应付金额)。
目录固定文案使用英文；当前领先来源显示 ` (proxy)` 或 ` (#N)`，解析器仍兼容历史中文/全角标记，用户名称保持原文。

Admin A44 在 [credentials/check.rs](src/credentials/check.rs) 使用独立只读探测器，仅接受数据库信封。
401/明确登录失效为 invalid，挑战、战斗页、限流、超时或协议变化为 inconclusive；无写入、重试、留证或状态回写。
每进程单并发、站点间隔 2 秒、每范围含等待预算 5 秒、正文上限 2 MiB；成功只证明当次登录态。

### PM 准备与已发送副本

`EhentaiGateway` 实现冻结的 `ForumPmSender`，复用唯一 active ForumCookie、原 credential session、持久 owner 和共享 HTTP gate。
安全 PM HTTP client 在构造时建立、随客户端克隆共享连接池，独立强制禁止重定向，不信任兼容构造器传入的 redirect 策略；
不改变既有主题/编辑客户端。目标仅允许固定生产 HTTPS origin，或显式配置的 HTTP loopback 隔离端点；不暗改配置为生产地址。
`forum.enabled`、持久 sending fence、任务 lease 与 item timeout 由 Worker 装配/调度拥有；此适配器不自行启动或读取配置。

`prepare` 只 GET `/index.php?act=Msg&CODE=04&MID=...`，从唯一 `REPLIER` 表单读取当前 `entered_name`、令牌与系统导航 sender。
不使用数据库旧 username；令牌、Cookie 和表单只留内存，不实现 Debug/序列化，不进入错误或普通 DTO。
表单仅允许已知控件与固定动作，强制 `add_sent=yes`、不发送 `add_tracking`，不 preview/save。
编辑器 `ffont/fsize` 仅接受默认首项为 0 的单选控件，识别但不复制到 POST；无名 `type=button` 辅助按钮亦不发送。
未知隐藏字段、提交动作或异常编辑器默认选择仍失败关闭。
只有调用方持久化 sender 并通过 sending fence 后才消费 prepared；一次消费只有一次 POST，不换 Cookie、不重试写入。
POST 200 仅触发回读，不是成功凭证；跳转、超时、读回不完整或无法证明唯一副本均为 `OutcomeUnknown`。

标题精确为 `Outbid <notification UUID 的32位simple形式>`（39 ASCII 字节）；正文上限 2048 字节、单行 ASCII，包含完整带连字符 notification UUID，
拒绝 BBCode/HTML 括号。正文不得携带代理上限、凭据等敏感数据。Sent 列表的同标题行证明 recipient 数字 UID，
只请求该行的同源 `CODE=03&VID=sent&MSID=...` 详情，并核验系统导航 sender、标题和 `.postcolor` 完整纯文本（仅文本/br及ASCII空白规范化）。
不读取收件人 Inbox、不扫描其他标题的详情。每次核对最多 8 个列表页（同时受既有更小页数配置限制）、8 个同标题候选、8 MiB 聚合响应，
单响应仍遵守已有配置；分页只跟随实际校验链接、不猜 `st` 步长，去除 `s` 会话参数。
发现首个副本不提前成功，其他已发现候选或分页未闭合时失败关闭；重复精确副本、未知 HTML/操作/链接格式不得产生回执。
`reconcile` 仅使用当前凭据读取同 sender 的 Sent；换账号拒绝、未找到返回 `None`，绝不授权重寄。

远端实际按姓名投递，MID 预读与回读 UID 证明不能提供原子改名保证：准备后改名可能误投且只能在事后发现，不能承诺消除此协议竞态。
历史浏览器的一封授权样本仅说明当时网页协议，不等于本 Rust 适配器/Worker 验收；本轮没有实发、读取真实 Cookie 或行为测试。
长度、限流、满箱、拒收及异常页面仍未行为验证，未知格式允许安全阻断或保留人工对账。

## 装备来源与密钥适配

`equipment/` 分别拥有来源配置、无代理读取、页面/身份/属性解析和 canonical 事实。
关闭业务 allowlist 不放宽固定 HTTPS origin、精确 world 路径、响应类型或身份校验。
`equipment/game_link.rs` 将安全目标与密文 owner 绑定后短暂解密，完整 URL 只移交受限 HTTP DTO 或论坛渲染。

装备信封使用绑定装备 ID 的 AES-256-GCM；Cookie、装备 key 与用户登录 key 使用各自数据密钥和域分离。
明文不进入持久化 payload、日志、审计或普通 DTO，Cookie 导入和登录 key 的授权输入/输出边界由应用与 HTTP 契约限定。
`forum/` 只在内存解析预览和渲染，远端请求归 `clients/ehentai`，回帖执行归 Worker。

## 边界约束

外部写入只在本地事务提交后执行，使用有界请求与持久化身份/结果证明；未知结果不能按普通暂时错误重试。
错误、metadata 与普通投影不得包含 URL 中的 secret、Cookie、token 或原始响应。
HTTP/解析异常及失败邮件只能通过独立加密诊断留证；装备成功读取丢弃预备密文，失败留证不改变原业务结果或 cursor。

## 凭据 owner 与取消传播

Worker 凭据 owner 随集合传给 gateway、克隆客户端、发送器和分页上下文；装备来源也共用 MoogleMail owner。
每个物理请求在限速后、发送前以有界检查核验原持锁 session；失锁永久停用，GET 返回不可用，POST 保守归为未知。
该边界不能撤回已发请求，也不能把本地检查与远端写入原子化；无 owner 的显式本地构造保持原行为。

## 维护与验证

```bash
cargo check --locked -p auction-integrations
```

仅静态编译；默认检查与测试范围见[开发指南](../../docs/development.md)，运行证据见[验证状态](../../docs/status.md)。
修改行为时同步对应契约，外部调用验证须由用户明确要求。
