# auction-integrations

外部服务网关：E-Hentai 论坛、MoogleMail，以及 Admin 装备来源。

- `src/clients/http.rs`：`build_http_client()` 构建统一使用 rustls/ring 的 `reqwest::Client`。
  外部适配器应复用该客户端，避免每个网关各自维护连接池或选择不同 TLS provider。
- `src/clients/ehentai.rs` 与 `src/clients/mooglemail/`：各自的传输与认证边界。MoogleMail
  已实现固定 endpoint、无重定向 GET、响应大小/内容类型/限速边界、session 失效分类、
  inbox 分页与 cursor 扫描、摘要/详情一致性校验、正文/附件/Credits/COD/操作证明解析，以及
  Bot/测试 Cookie 的有序切换。写入侧已实现 compose/detail typed target、表单闭集、无重定向
  单次 POST、write_unknown 不确定性分类、附件/COD 状态证明和发送成功页证明；解析结果不携带
  原始 HTML，token 只在短生命周期状态对象中受控引用，Cookie 不进入返回值。`auction-application`
  已定义窄的 `DeliveryTransport` 端口；读取适配器已经由 Worker 的持久化轮询 owner 装配，但当前
  只提交 `parsed_pending_processing` 安全入站事实。结算 payload 尚未补齐消息模板、外部装备
 ID/key 的解析 owner，因此 DeliveryTransport 仍未装配，不会凭空发出远程写请求。
  `MoogleMailSender` 提供了可复用的高层发送编排：GET compose 后要求干净状态，逐步附加
  Credits/装备/CoD，并在每次 POST 后证明状态，最后证明发送结果；它只接受已解析的远端装备
  ID，不负责把内部装备 UUID 或密钥信封转换成远端附件身份。

调用顺序是：用 `MoogleMailClient::production()` 建立受限客户端；用
`MoogleMailCredentialSet::new(keyenv.bot_cookie(), keyenv.test_cookie())` 建立 Bot 优先、测试号
后备的凭据集合；再以明确的 sender allowlist 构造 `MoogleMailGateway` 并调用
`fetch(world, cursor, limit)`。只有摘要扫描、allowlist 筛选、详情读取和全部一致性校验成功后，
gateway 才返回可提交的新 cursor；应用层仍负责幂等入库和业务分类。
- `src/equipment/mod.rs`：固定 HTTPS origin 的旧装备页解析和 canonical JSON 规范化；关闭环境
  代理与自动重定向，并限制响应类型/大小。`src/cryptobox/mod.rs` 只在适配器边界使用敏感
  数据密钥，以 AES-256-GCM 生成绑定装备 ID 的密文信封，明文 key 不进入应用写端口。
- `src/forum/mod.rs`：把 Admin 提供的 HTML 或纯文本论坛内容解析为有界的 catalog、posts、
  warnings 和命令预览；该适配器只在内存中工作，不执行远程抓取、回帖或数据库写入。

## 边界约束

本 crate 只负责外部传输和认证。外部响应**不得直接修改拍卖状态**，必须经过应用命令或持久化
Worker 操作。

外部写入发生在本地事务提交之后：论坛发帖、邮件和通知失败不能抹掉已接受的竞价。每个外部写入
都要有 timeout、重试上限、幂等键和审计日志。

兼容旧系统时建立防腐层或映射，不把旧 DTO 或旧命名未经审计地传播到新领域。

论坛 Cookie 与生产邮件 Cookie 只能从 `auction-config` 的显式 getter 读取，不得进入日志、DTO
或快照。用户明确提供的本地 MoogleMail 联调凭据可通过 `load_mooglemail_keyenv` 读取；该
loader 只接受 `.keyenv` 中的 `hv-ck-key1`（Bot）和 `hv-ck-key2`（测试号），强制 owner-only
权限，并拒绝未知/重复键。`.keyenv` 仍不得提交或进入普通配置、日志和 DTO。
