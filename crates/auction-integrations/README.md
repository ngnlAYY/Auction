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
| [src/clients/mooglemail](src/clients/mooglemail) | 邮件读取、动作/发送证明与加密诊断 |
| [src/equipment](src/equipment) | 装备来源策略、页面解析与安全链接解析 |
| [src/credentials](src/credentials) | Cookie 信封与 Admin 只读认证探测 |
| [src/cryptobox](src/cryptobox)、[src/login_key](src/login_key) | 装备/登录 key 加密与短时解密 |
| [src/forum](src/forum) | 纯本地论坛解析与 BBCode 渲染 |

## 职责与实现边界

实现应用层外部能力端口，只返回校验过的事实或安全错误。
外部响应必须经过应用命令或持久化 Worker 操作，不能直接修改拍卖状态；本地提交后的外部失败不能撤销已接受竞价。

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
| 论坛写入 | 同源表单与精确 board/thread/post；固定帖需真实远端身份和正文回读证明 |

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
论坛仅导入普通出价，代理行在账本前忽略；固定 #1 展示目录，#2 展示可证明处理记录与连续楼层，不能猜测身份或进度。

Admin A44 在 [credentials/check.rs](src/credentials/check.rs) 使用独立只读探测器，仅接受数据库信封。
401/明确登录失效为 invalid，挑战、战斗页、限流、超时或协议变化为 inconclusive；无写入、重试、留证或状态回写。
每进程单并发、站点间隔 2 秒、每范围含等待预算 5 秒、正文上限 2 MiB；成功只证明当次登录态。

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
