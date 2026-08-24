# auction-integrations

外部服务网关：E-Hentai 论坛、MoogleMail，以及 Admin 装备来源。

- `src/clients/http.rs`：`build_http_client()` 构建统一使用 rustls/ring 的 `reqwest::Client`。
  外部适配器应复用该客户端，避免每个网关各自维护连接池或选择不同 TLS provider。
- `src/clients/ehentai/` 与 `src/clients/mooglemail/`：各自的传输与认证边界。E-Hentai
  目录由 `mod.rs` 负责公开导出，`error.rs`/`credentials.rs` 持有错误和 Cookie 候选集，
  `client/mod.rs` 负责客户端装配与公共入口，`client/request.rs` 持有固定 origin、限速、
  响应边界和请求安全校验，`client/topic.rs` 持有主题表单与发布回执解析；`gateway.rs` 持有凭据轮换与 Forum 端口，
  `forms.rs` 持有隐藏表单、URL 和响应安全校验。MoogleMail
  gateway 已进一步按 `gateway/{credentials,model,reader}.rs` 拆分凭据候选池、读取结果模型和
  inbox/Sent/Balance 读取流程，`gateway.rs` 只保留稳定导出与安全错误辅助。
  其中 `parser/` 按类型、列表读取、详情动作、脚本/装备和公共 helper 拆分页面事实解析与证明，
  `transport/` 按 payload、delivery、结果证明和类型快照拆分外部 HTTP/凭据轮换与请求边界；
  `client/mod.rs` 负责 typed target、配置和装配，`client/request.rs` 负责有界 GET/POST 及结果分类，
  `client/security.rs` 负责固定 endpoint、响应类型、session 判定、限速和敏感载荷清零；
  `protocol.rs`、`forms.rs`、`writer/` 和 `sender.rs` 分别拥有
  协议值对象、表单、写入状态/compose 证明与发送编排；`writer.rs` 只负责稳定导出，
  `writer/{model,common,compose,response}.rs` 按状态模型、共用安全解析、compose 证明和最终响应分类拆分。
  已实现固定 endpoint、无重定向 GET、响应大小/内容类型/限速边界、session 失效分类、
  inbox 分页与 cursor 扫描、摘要/详情一致性校验、正文/附件/Credits/COD/动作状态解析，以及
  Bot/测试 Cookie 的有序切换。写入侧已实现 compose/detail typed target、表单闭集、无重定向
  单次 POST、write_unknown 不确定性分类、发送成功页证明，以及 return/take 的动作前身份/资产
  fence 和动作后结构化证明；解析结果不携带原始 HTML，token 只在短生命周期状态对象中
  受控引用，Cookie 不进入返回值。`auction-application`
  已定义窄的 `DeliveryTransport` 端口；Worker 现已装配 MoogleMail delivery transport，使用
  严格版本化 payload 执行静态通知、结算 send/return、return 和 take。return 必须在写入前核对
  sender，写入后证明收件箱中精确 `mid` 消失；take 还必须精确核对装备 ID、Credits、COD 和
  附件计数，并以资产归零、邮件身份不变与唯一回执证明收口。结算 send 的 compose 成功页不会
  直接满足履约，还需要 Sent 明细与余额证据；POST 后
  无法证明的结果进入 `manual_reconcile`，不会自动重发。
  send 前置要求 compose 页能够证明为无收件人/正文/附件/COD 的干净状态，否则以
  `preflight_unproven` 进入人工对账，不会覆盖可能属于上一次未知写入的资产。最终 send 响应只从
  排除 script/style/noscript/textarea 后的可见文字识别旧版闭集失败：
  “收件人不存在”会保留唯一退信 `mid`、`take_required` 和稳定 reason code；虽然拒绝本身已有
  协议证明，退信资产在取回前仍进入 `manual_reconcile`；
  “无法支付邮资”在未暂存资产时是确定失败，暂存过 Credits/装备/COD 时则进入
  `manual_reconcile`，不会盲目重试并重复处理资产。
  transport 还会在 delivery 进入 `sending` 前按 payload world 检查 scoped/shared 凭据；无凭据时
  由 Worker 释放 claim。进入 `sending` 后的凭据、网络或协议依赖失败不会伪装成普通终态失败。
  `MoogleMailSender` 提供了可复用的高层发送编排：GET compose 后要求干净状态，逐步附加
  Credits/装备/CoD，并在每次 POST 后证明状态，最后证明发送结果；它只接受已解析的远端装备
  ID，不负责把内部装备 UUID 或密钥信封转换成远端附件身份。

调用顺序是：用 `MoogleMailClient::production()` 建立受限客户端；Worker 优先从 PostgreSQL
读取 A32 active 凭据池并用独立数据密钥解密；暂时没有 active 行时保留 degraded 运行态，实际
操作返回 credential unavailable；未配置该能力时再使用
`MoogleMailCredentialSet::new(keyenv.bot_cookie(), keyenv.test_cookie())` 或敏感配置 getter
建立 Bot 优先、测试号后备的凭据集合；数据库凭据保留 `main`/`isekai`/`shared` scope，world
专属 Cookie 只服务对应 world，shared Cookie 才能跨 world 复用；再以明确的 sender allowlist
构造 `MoogleMailGateway` 并调用 `fetch(world, cursor, limit)`。只有摘要扫描、allowlist 筛选、详情读取和全部一致性校验成功后，
gateway 才返回可提交的新 cursor；应用层负责幂等入库和业务分类。当前首条业务链已完成拍卖邮件的
 Credits/COD 拒绝、无附件通知、装备来源读取、起拍价指令解析、拍卖入站、用户注册/重绑和
 take/return 编排；用户登录 key 通过独立数据密钥生成密文信封，只有 DeliveryTransport 发送前
 短暂解密。结算邮件来源退信和后台来源装备交接通知已接入；Sent/Balance 回执严格解析、exact
 remote ID 查找、proof 持久化和人工复核边界也已由 PostgreSQL Worker 接入。E-Hentai 论坛网关已
实现固定 origin、受限响应读取、凭据轮换、精确标题查找/主题创建和主题读取；创建 POST 无法证明
时返回 `write_unknown`，不会盲目重发。`ForumThreadRenderer` 负责有界主题正文，回帖命令仍先
经过解析器、登记身份和应用竞价端口，适配器本身不直接写竞价状态。
- `src/equipment/`：`config.rs` 冻结固定 HTTPS origin 外的 host/path allowlist、响应上限、共享
  请求间隔和完整请求超时，`reader.rs` 负责无代理/无重定向读取与应用端口适配，`parser.rs` 只
  解析旧装备页、URL 和闭集属性，`model.rs` 负责 canonical JSON 事实与稳定指纹。关闭业务
  allowlist 也不会放宽固定 origin、精确 world 路径、响应类型、content encoding 和最终响应
  origin 校验。`src/cryptobox/mod.rs` 只在适配器边界使用敏感数据密钥，以 AES-256-GCM 生成绑定
  装备 ID 的密文信封，明文 key 不进入应用写端口。
- `src/credentials/mod.rs`：A32 外部凭据加密适配器。MoogleMail 与 E-Hentai Cookie 使用独立
  的 AAD/HMAC 域和 `credential_data_key`，密文信封只保存 nonce、key id 和指纹；Admin 导入
  与 Worker 解密都不返回 secret，密钥不进入日志、DTO 或审计 metadata。凭据管理只由
  PostgreSQL 完整档提供，SQLite 不返回伪造的 Lite 能力。
- `src/forum/mod.rs`：只负责 ForumPreviewAdapter 和渲染/解析出口；`src/forum/parser/` 按
  HTML 抽取、纯文本切分、目录、命令/金额、内部模型和结果辅助拆分有界的 catalog、posts、warnings
  和命令预览。解析器只在内存中工作。远程主题读写由 `src/clients/ehentai/` 拥有，回帖导入仍由
  `auction-jobs` 通过应用端口执行。

## 边界约束

本 crate 只负责外部传输和认证。外部响应**不得直接修改拍卖状态**，必须经过应用命令或持久化
Worker 操作。

外部写入发生在本地事务提交之后：论坛发帖、邮件和通知失败不能抹掉已接受的竞价。每个外部写入
都要有 timeout、重试上限、幂等键和审计日志。

兼容旧系统时建立防腐层或映射，不把旧 DTO 或旧命名未经审计地传播到新领域。

外部 Cookie 只能从 A32 的密文信封或 `auction-config` 的显式 getter 读取，不得进入日志、DTO
或快照。用户明确提供的本地 MoogleMail 联调凭据可通过 `load_mooglemail_keyenv` 读取；该
loader 只接受 `.keyenv` 中的 `hv-ck-key1`（Bot）和 `hv-ck-key2`（测试号），强制 owner-only
权限，并拒绝未知/重复键。`.keyenv` 仍不得提交或进入普通配置、日志和 DTO。
