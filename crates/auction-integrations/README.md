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
| [src/clients](src/clients) | 受限 HTTP、论坛与邮件传输/解析 |
| [src/equipment](src/equipment) | 装备来源策略、读取与解析 |
| [src/credentials](src/credentials) | 外部 Cookie 加密 |
| [src/cryptobox](src/cryptobox) | 数据密钥与装备 key 信封 |
| [src/forum](src/forum) | 本地论坛解析与 BBCode 渲染 |
| [src/login_key](src/login_key) | 用户登录 key 签发/读取 |

## 职责与实现边界

外部服务网关：E-Hentai 论坛、MoogleMail，以及 Admin 装备来源。

## HTTP 与协议解析

- [src/clients/http.rs](src/clients/http.rs)：`build_http_client()` 构建统一使用 rustls/ring 的 `reqwest::Client`。
  外部适配器应复用该客户端，避免每个网关各自维护连接池或选择不同 TLS provider。
- [src/clients/ehentai/](src/clients/ehentai/) 与 [src/clients/mooglemail/](src/clients/mooglemail/)：各自的传输与认证边界。E-Hentai
  目录由 `mod.rs` 负责公开导出，`error.rs`/`credentials.rs` 持有错误和单 Cookie 会话，
  `client/mod.rs` 负责客户端装配与公共入口，`client/request.rs` 持有固定 origin、限速、
  响应边界和请求安全校验，`client/pagination.rs` 持有同 origin/thread 的主题分页发现与页数上限，
  `client/topic.rs` 持有主题/回复/编辑表单与发布回执解析；`gateway.rs` 持有凭据失效与 Forum 端口，
  `forms.rs` 持有隐藏表单、URL 和响应安全校验。MoogleMail
  gateway 已进一步按 `gateway/{credentials,model,reader}.rs` 拆分单 Cookie 会话、读取结果模型和
  inbox/Sent/Balance 读取流程，`gateway.rs` 只保留稳定导出与安全错误辅助。
  其中 `parser/` 按类型、列表读取、详情动作、脚本/装备和公共 helper 拆分页面事实解析与证明；
  `parser/reads/{mod,inbox,balance}.rs` 与 `parser/reads/sent/{mod,detail}.rs` 进一步隔离 inbox、
  Sent 摘要、Sent 详情和 Item Shop 余额读取；
  `transport/` 按 payload、delivery、结果证明和类型快照拆分外部 HTTP/凭据会话与请求边界；
  其中 `transport/payload/{mod,mail,settlement}.rs` 分别负责 payload 装配、普通邮件 delivery
  校验以及结算 payload/模板渲染；
  其中 `transport/types/{mod,delivery,settlement,templates,transport}.rs` 分别负责稳定导出、
  普通/结算载荷、模板快照和 transport 装配状态；
  其中 `transport/delivery/{mod,messages,credentials,actions}.rs` 分别负责入口分派、通知/结算消息、
  单 Cookie 写入隔离和 return/take 动作证明；
  `client/mod.rs` 负责 typed target、配置和装配，`client/request.rs` 负责有界 GET/POST 及结果分类，
  `client/security.rs` 负责固定 endpoint、响应类型、session 判定、限速和敏感载荷清零；
  `protocol/`、`forms.rs`、`writer/` 和 `sender.rs` 分别拥有
  协议值对象、表单、写入状态/compose 证明与发送编排；`protocol/{mod,model,builder,validation}.rs`
  分别负责协议出口、值对象、安全 URL 构造和输入/endpoint 校验；`writer.rs` 只负责稳定导出，
  `writer/{model,common,compose,response}.rs` 按状态模型、共用安全解析、compose 证明和最终响应分类拆分。
  已实现固定 endpoint、无重定向 GET、响应大小/内容类型/限速边界、session 失效分类、
  inbox 分页与 cursor 扫描、摘要/详情一致性校验、正文/附件/Credits/COD/动作状态解析，以及
  单 Cookie 失效收口。GET/POST 正文按块累计并在追加前检查大小，无 Content-Length 的响应也受同一上限约束；
  超时、截断或超限不留存半截正文。邮件、装备、余额及战斗页面中的用户文案不构成登录失效证明，
  仅受限认证重定向或排除用户输入区后的登录页面证据允许作废 Cookie。装备链接中的固定长度 key
  使用安全 UTF-8 切片；通知和结算模板正文共用表单正文校验，允许配置支持的换行和制表符。
  写入侧已实现 compose/detail typed target、表单闭集、无重定向单次 POST、write_unknown 不确定性分类、
  发送成功页解析，以及 return/take 的动作前身份/资产
  fence 和动作后结构化证明；解析结果不携带原始 HTML，token 只在短生命周期状态对象中
  受控引用，Cookie 不进入返回值。`auction-application`
  已定义窄的 `DeliveryTransport` 端口；Worker 现已装配 MoogleMail delivery transport，严格解码
  旧版 schema v1 payload 后执行静态通知、结算 send/return、return 和 take；装备 key 只在实际发送前
  按装备 ID 从密文 owner 读取并解密，不进入 intent、payload、日志或回执。return 必须在写入前核对
  sender，写入后证明收件箱中精确 `mid` 消失；take 还必须精确核对装备 ID、Credits、COD 和
  附件计数，并以资产归零、邮件身份不变与唯一回执证明收口。无脚本 Take 终态页只从脚本和
  可执行属性保守匹配原始协议标记，包括其中的字符串、注释和不完整赋值；URL 按浏览器
  空白规则归一化。用户正文、textarea 与 HTML 注释不参与判定。
  结算 send 的 compose 成功页不会
  直接满足履约，还需要 Sent 明细与余额证据；POST 后
  无法证明的结果进入 `manual_reconcile`，不会自动重发。`WinnerPaymentCarrierCod` 的结算
  payload 按旧版契约固定使用 `main` world。
  send 前置要求 compose 页能够证明为无收件人/正文/附件/COD 的干净状态，否则以
  `preflight_unproven` 进入人工对账，不会覆盖可能属于上一次未知写入的资产。最终 send 响应只从
  排除 script/style/noscript/textarea 后的可见文字识别旧版闭集失败：
  “收件人不存在”会保留唯一退信 `mid`、`take_required` 和稳定 reason code；虽然拒绝本身已有
  协议证明，退信资产在取回前仍进入 `manual_reconcile`。runner 会在当前轮次内暂时排除同一
  `(world, recipient)` 的后续 delivery，其他玩家继续处理；下一轮重新评估，不形成永久黑名单；
  “无法支付邮资”在未暂存资产时是确定失败，暂存过 Credits/装备/COD 时则进入
  `manual_reconcile`，不会盲目重试并重复处理资产。
  同一凭据的 compose/send 与 return/take 会从首次前置读取到最终结果证明全程持有凭据级异步
  guard；不同 delivery 不会再交错修改同一个远端 compose，普通 inbox/Sent/Balance 读取不占用该
  写入 guard。Worker 启动时还按 Cookie 类型取得不透明的 PostgreSQL session 所有权，防止多个
  Worker 共享同一种 Cookie；transport 失败 metadata 只记录闭集 `stage`、`operation`、`category` 和
  `write_started`，普通 metadata 不保存 URL、Cookie、token 或原始响应。HTTP/解析异常的实际响应字节，以及业务拒绝、
  失败和超时的实际邮件字段，只能进入独立加密异常证据；普通 transport metadata、数据库和 DTO 仍不含未脱敏的
  原始数据。Admin 的 `message_display` 是应用层从完整 `MailMessage` 生成的受限脱敏审查投影，不是 transport
  原始字段，边界见 [`docs/operations/mail-diagnostics.md`](../../docs/operations/mail-diagnostics.md)。
  MoogleMail transport 同时向 delivery runner 声明进程内并发上限为 1，使背压发生在领取下一条
  claim 之前；凭据锁仍作为 direct caller 和未来装配路径的最后一道写入隔离。
  transport 还会在 delivery 进入 `sending` 前按 payload world 检查 scoped/shared 凭据；无凭据时
  由 Worker 释放 claim。进入 `sending` 后的凭据、网络或协议依赖失败不会伪装成普通终态失败。
  `MoogleMailSender` 提供了可复用的高层发送编排：GET compose 后要求干净状态，逐步附加
  Credits/装备/CoD，并在每次 POST 后证明状态，最后证明发送结果；它只接受已解析的远端装备
  ID，不负责把内部装备 UUID 或密钥信封转换成远端附件身份。
  所有主动 ComposeSend 统一在任意 POST 前读取覆盖所有 MID 的 Sent baseline，POST 后独立扫描 Sent；只有新增且
  唯一、同 world/recipient/subject/canonical body digest、Credits/COD/装备 ID 精确匹配的 MID 才成功，0 资产
  通知也必须有 0 资产 Sent 证明。资产不符、秒领取、无匹配、多匹配、未扫完或超时均为未知并进入
  `manual_reconcile`，不自动重寄，不接受成功页、空 compose 或本地 UUID fallback。成功 metadata 为
  `sent_verified=true`、`external_reference_proven=true`、`proof=sent_exact_payload_v1`，真实引用为
  `mooglemail:world:send:mid`。Return/Take 保持独立 action proof；
  [sender/proof.rs](src/clients/mooglemail/sender/proof.rs) 编排 baseline 与新增 Sent 核验，内容证明由
  [parser/reads/sent/detail.rs](src/clients/mooglemail/parser/reads/sent/detail.rs) 的
  `MoogleMailParser::parse_sent_content_proof` 提供。
  装备来源适配器对每个完整响应先加密 prepare，只有读取/解析/身份/snapshot/filter 失败才在 2 秒内 retain；
  成功读取丢弃密文。装备失败诊断只保存首个低基数 category/detail_code 和安全关联，网络/超时/截断/超大响应
  只能标记 `body_unavailable`，不伪称完整响应。
  诊断 prepare/retain 不由 integration 自行 await 或创建 tracing 出口；Worker 通过同步 callback 注入日志和 raw
  recorder。HTTP body timeout 只保留 `body_unavailable` 与最后实际观测 status，不伪称 partial body；session expired
  在凭据耗尽或失效写回失败时保留最后已取得的 body/status/elapsed。

## 凭据装配与外部请求

调用顺序是：用 `MoogleMailClient::production()` 建立受限客户端；Worker 优先从 PostgreSQL
读取 A32 唯一 active Cookie 并用独立数据密钥解密；暂时没有 active 行时保留 degraded 运行态，实际
操作返回 credential unavailable；未配置该能力时再使用
`.keyenv` 的 `hv-ck-key1` 或敏感配置 getter 建立单 Cookie 集合；`hv-ck-key2` 只供显式隔离验证，
不进入 Worker 候选。数据库凭据保留 `main`/`isekai`/`shared` scope，world 专属 Cookie 只服务
对应 world，shared Cookie 才能跨 world 复用。数据库与兼容来源都在 Worker 启动时按凭据类型
取得进程生命周期所有权；再以明确的 sender allowlist
构造 `MoogleMailGateway`；Worker 通过 `fetch_with_deadline` 传入每次尝试的绝对截止时刻，独立
`fetch` 调用使用 gateway 配置的默认读取预算。完整完成摘要扫描和 allowlist 筛选后，详情按旧到新
读取，临近截止时返回已通过全部一致性校验的连续前缀；应用层负责幂等入库和业务分类。收件箱从新到旧扫描时，
有界窗口保留 cursor 之后最旧的 `limit` 封目标邮件，再按旧到新读取详情，防止积压量超过批量
上限时跳过旧邮件。cursor 对应最后返回邮件的 `external_mail_id`，Worker 可据此安全提交完成前缀。
列表扫描、共享限速、请求/正文读取及凭据失效处理使用同一绝对 deadline；详情阶段预留最多
100 毫秒完成返回，避免外层超时先取消已解析前缀。空窗口保持原 cursor 并正常成功，有候选但
没有完成详情才报告预算失败；列表未完整定位 cursor 或末尾时不返回可推进窗口。
当前首条业务链已完成拍卖邮件的
 Credits/COD 拒绝、无附件通知、装备来源读取、起拍价指令解析、拍卖入站、用户注册/重绑和
 take/return 编排；用户登录 key 通过独立数据密钥生成密文信封，只有 DeliveryTransport 发送前
 短暂解密。结算邮件来源退信和后台来源装备交接通知已接入；Sent/Balance 回执严格解析、exact
remote ID 查找、proof 持久化和人工复核边界也已由 PostgreSQL Worker 接入。E-Hentai 论坛网关已
实现固定 origin、受限响应读取、单 Cookie 失效收口、精确标题查找/主题创建、固定 #2 回复、固定 post 编辑和主题读取；
主题读取按真实 `postcolor`、`post-{id}`、`showuser={id}` 和结构化 `.edit` 锚点把每页 HTML
单次构建为受限页面事实，分页身份证明与帖子/回复语义解析共享该结果，不再重复构建 DOM；它会从第一页发现规范 `st` 水位，按 20 帖分页补齐全部页面，并用每个已读页面复核该水位没有扩张；额外页面最多四路并发读取且仍
共享请求起始间隔；每页响应继续受配置上限约束，全部主题 HTML 另有 32 MiB 聚合硬上限。每个响应
都要用非用户正文区的同 thread permalink 绑定该页真实 post ID；任一 `postcolor` 缺少规范数字 ID、
非末页不是完整 20 帖、页数超过配置上限、页码不规范或合并后 post ID 重复/逆序都会失败关闭，
不会把错误主题或前几页结果当作完整出价。
写入会加载真实表单并核对同源 `/index.php` action、POST 方法及 exact board/thread/post，回复 action
不得携带 post 目标；固定帖 POST 只接受同源重定向中的唯一 thread/post 身份证明，或真实论坛
`Please stand by...` 200 回执中完全一致的 meta refresh 与链接双重证明，无法证明时返回
`write_unknown`。`ForumThreadRenderer` 负责参考旧帖结构生成有界、紧凑且适合论坛窄栏的 #1 BBCode
和确定性 #2 处理日志；#1 保持分类标题和单行拍品目录，每行按可用字段展示
`(Lv.440, SDA, Ave: 56.7%) (seller: name) (start: 30k)`；Primary Attributes 缩写固定按
Strength、Dexterity、Agility、Endurance、Intelligence、Wisdom 顺序选取已有属性的首字母。
#1 的领先者继续使用玩家名称；可证明的论坛竞价紧邻显示可点击的 `（#楼层）`，Web 代理只显示
`（代理）`，论坛回读会剥离该展示后缀，不把它写入玩家名称。
#2 只按公开处理事实展示处理水位、真实告警/
人工忽略计数、红色告警标题和紧凑单行记录，并用解析所得真实
post ID 为楼层号生成 E-Hentai findpost 链接；它还显示本轮能够连续证明已处理到的楼层，待重试命令
会阻止水位越过对应楼层，不按 post ID 或末楼伪造进度；固定帖 ownership 标记只放在正文末尾供机器
校验并以透明文本渲染，不作为用户可见日志内容。它不按楼层位置猜测远端身份，也不伪造成功竞价数或正文外身份，
按分类列出拍品并把装备名称链接到解析后的真实 HentaVerse URL；回帖命令仍先
从同一个论坛生成的 `.bigusername` 作者链接读取 `showuser` 数字账号键和链接文本；数字键作为
稳定唯一的论坛账号 ID 交给 Users owner，链接文本作为可变的 MoogleMail 收件人/页面显示名称。
用户正文、签名和泛化 data 属性都不能提供作者身份，缺少任一身份分量或同一帖子行出现冲突链接时
失败关闭，数字键不用于页面展示或 MM
收件人，适配器本身不直接写竞价状态。论坛会话只在 HTTP 401 或
正文明确出现登录表单/失效文案时作废；Cloudflare 前置挑战同样可能返回 403，不能据此误删有效
Cookie，而应保留凭据并以协议/访问失败收口。论坛回复只执行普通出价；代理创建/降低是 Public Web
专属能力，解析器会在账本前静默丢弃论坛代理行，且不会把它们写入固定 #2。

## 装备来源与密钥适配

- [src/equipment/](src/equipment/)：`config.rs` 冻结固定 HTTPS origin 外的 host/path allowlist、响应上限、共享
  请求间隔和完整请求超时，`reader.rs` 负责无代理/无重定向读取与应用端口适配，`parser/` 按
  `url.rs`、`page.rs`、`identity.rs`、`stats.rs` 和 `common.rs` 分别解析来源 URL、旧装备页、
  闭集属性、统计值和共用边界，`parser/mod.rs` 只保留稳定出口；`model.rs` 负责 canonical JSON 事实与稳定指纹。关闭业务
  allowlist 也不会放宽固定 origin、精确 world 路径、响应类型、content encoding 和最终响应
  origin 校验。装备页解析器把 `.eq` 直接子项与 `.ex`/`.ep` 分组属性分开处理，避免嵌套 `title`
  被重复解释为主属性；同时兼容旧页的 `Tier A/B/C Soulbound` 头部，但仍严格校验阶段值、类别和
  页面结构。`game_link.rs` 为 P09/P11 已读取的 Public lot 批量绑定密文 owner，核对 world、
  外部 ID 和无 key 投影后短暂解密，构造固定 HentaVerse origin 的完整 URL；该值只移交最终 Public
  HTTP DTO 或论坛正文渲染边界，不进入日志、审计、事件或持久化缓存。[src/cryptobox/mod.rs](src/cryptobox/mod.rs) 只在适配器边界使用敏感数据密钥，以 AES-256-GCM 生成绑定
  装备 ID 的密文信封，明文 key 不进入应用写端口。
- [src/credentials/mod.rs](src/credentials/mod.rs)：A32 外部凭据加密适配器。MoogleMail 与 E-Hentai Cookie 使用独立
  的 AAD/HMAC 域和 `credential_data_key`，密文信封只保存 nonce、key id 和指纹；Admin 导入
  与 Worker 解密都不返回 secret，密钥不进入日志、DTO 或审计 metadata。凭据管理只由
  PostgreSQL 完整档提供，SQLite 不返回伪造的 Lite 能力。
- [src/forum/mod.rs](src/forum/mod.rs)：只负责 ForumPreviewAdapter 和渲染/解析出口；[src/forum/parser/](src/forum/parser/) 按
  单次 DOM 页面事实抽取、纯文本切分、目录、命令/金额、内部模型和结果辅助拆分有界的 catalog、posts、warnings
  和命令预览。解析器只在内存中工作。远程主题读写由 [src/clients/ehentai/](src/clients/ehentai/) 拥有，回帖导入仍由
  `auction-jobs` 通过应用端口执行。

## 边界约束

本 crate 只负责外部传输和认证。外部响应**不得直接修改拍卖状态**，必须经过应用命令或持久化
Worker 操作。

外部写入发生在本地事务提交之后：论坛发帖、邮件和通知失败不能抹掉已接受的竞价。每个外部写入
都要有 timeout、重试上限、幂等键和审计日志。

兼容旧系统时建立防腐层或映射，不把旧 DTO 或旧命名未经审计地传播到新领域。

外部 Cookie 只能从 A32 的密文信封或 `auction-config` 的显式 getter 读取，不得进入日志、DTO
或快照。`mail_cookie` 与 `forum_cookie` 各自全局最多一条 active 记录；MoogleMail 与
HentaiVerse 装备来源共用按 world/scoped/shared 选择的单 Cookie。远端明确返回登录页时才允许
回写数据库凭据失效，单 Cookie 失效后不会自动切换，登录页属于凭据不可用而不是 URL/页面协议错误。
用户明确提供的本地 MoogleMail 联调凭据可通过 `load_mooglemail_keyenv` 读取；该
loader 只接受 `.keyenv` 中的 `hv-ck-key1`（Worker）和 `hv-ck-key2`（隔离验证），强制
owner-only 权限并拒绝未知/重复键；Worker 只装配前者，后者不自动回退。`.keyenv` 仍不得提交或
进入普通配置、日志和 DTO。

## 凭据 owner 与取消传播

Worker 的 MoogleMail/论坛凭据 owner 会由凭据集合自动传给 gateway 与 delivery HTTP client；
client 克隆及 sender、分页上下文继续共享同一 owner。每次物理 HTTP 请求在限速等待后、发出前
核验原持锁 session；装备来源的独立 reqwest client 也读取同一 MoogleMail 凭据 owner。
失锁状态不可恢复，集合停止提供凭据，GET 返回凭据不可用；POST 前发现失锁仍保守归为
`write_unknown`，避免先前 compose 已暂存资产时被误判为可安全重发。显式不带 owner 的本地构造
保持原行为。该边界不能撤回已经发出的请求，也不能把 `check → send` 与远端操作原子化。

## 维护与验证

修改本模块时同步对应[契约](../../docs/contracts/README.md)与源码入口；跨模块理由按[决策记录规则](../../docs/documentation.md)维护。

```bash
cargo check --locked -p auction-integrations
```

该命令只做静态编译；完整门禁见[开发指南](../../docs/development.md)，运行证据边界见[验证状态](../../docs/status.md)。
