# auction-integrations

外部服务网关：E-Hentai 论坛、MoogleMail，以及 Admin 装备来源。

- `src/clients/http.rs`：`build_http_client()` 构建统一使用 rustls/ring 的 `reqwest::Client`。
  外部适配器应复用该客户端，避免每个网关各自维护连接池或选择不同 TLS provider。
- `src/clients/ehentai.rs`、`src/clients/mooglemail.rs`：各自的传输与认证边界。当前只有配置结构，认证和写操作在
  外部契约冻结后实现。`auction-application` 已定义窄的 `DeliveryTransport` 端口，当前
  尚未把 MoogleMail 客户端接入该端口；因此 delivery outbox 只会推进到可重试/人工对账的
  持久化边界，不会凭空发出远程请求。
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

论坛 Cookie 与邮件 Cookie 只能从 `auction-config` 的显式 getter 读取，不得进入日志、DTO 或
快照。
