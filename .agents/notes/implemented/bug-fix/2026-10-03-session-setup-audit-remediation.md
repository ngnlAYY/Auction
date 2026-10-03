# Agent Note: 管理会话撤销与首次设置安全收口

Status: implemented

## Problem

2026-10-02 对 `5cca279` 的全仓静态审计确认 7 项问题：Admin 更换配置凭据后旧会话仍能认证，
首次空库初始化绕过历史迁移兼容层，rustls/h2 锁定版本命中漏洞公告，以及设置服务缺少
请求/退出时限、防嵌入和拒绝路径安全头。2026-10-03 按用户“修复并优化”落地本次修改。
初始化兼容及拒绝路径安全头属于此前已登记缺口，不宣称本轮首次发现。

默认不运行行为测试，不连接数据库、不执行迁移或调用外部业务系统；已发布 SQL、checksum、
Cookie 名称与 DTO 不因修复变更。Public 与 Admin、正式服务与首次设置仍保持独立 owner。

## Decision

| 问题 | 修复与所有权 |
| --- | --- |
| F1 | application 的 Admin 服务异步构造先调用 sessions 端口永久撤销全部旧会话；storage 两后端以单条原子 UPDATE 完成，保留已撤销时间。随机 token 摘要用标准 HMAC-SHA256 绑定当前 actor/secret，current 再核对 actor；不新增凭据校验值或迁移。Admin 每次重启重新登录，Public 不变 |
| F2 | initialization 在原 SQLx 迁移锁内调用 postgres_migrator，保留专用连接、空库/零其他客户端检查、历史 checksum 与受限别名替换，不重复加锁 |
| F3/F6 | 按 RustSec RUSTSEC-2026-0285 / RUSTSEC-2026-0258 更新 rustls 至 0.23.45、h2 至 0.4.19；rustls 声明下限同步。HMAC 0.12 与现有 sha2 0.10 类型兼容，不扩大为全图哈希升级 |
| F4 | 设置复用 auction-http 连接 owner 的 5 秒 HTTP/1 请求头预算；access 为正文与异步处理设置 10 秒 deadline，令牌/操作门禁移至正文前；lifecycle 用 auction-process 排空最多 10 秒，取消仍由所持连接 JoinSet 负责 |
| F5/F7 | 提取 auction-http/security/headers.rs，正式表面与设置共用原 Dioxus 兼容策略；设置允许、拒绝、框架错误与超时应用响应统一补齐 no-store、no-referrer、frame-ancestors、DENY 等头 |

设置异步超时输出 504 `request_timeout`，复用浏览器已有错误映射；complete 遇到超时仍保守
解释为 `setup_outcome_unknown`，不新增自动重试。同步发布目录至写入终态之间没有 await，
不能用请求取消推断目录未发布，也不承诺 Tokio deadline 可以抢占阻塞磁盘操作。
停止后只用 try_lock 核对终态；无法确认、收到终止信号或结果不确定都不能进入自动初始化。

Admin 启动撤销是显式安全策略，不是只在 secret 改变时才登出。这样无需保存可独立验证口令的摘要，
即使恢复旧配置也无法复活此前已撤销的 token。同库多 Admin 实例的任一启动会撤销既有管理会话；
实例凭据须一致，轮换前必须停掉全部旧凭据实例，不把本地配置冒充跨实例全局权限栅栏。
凭据摘要、token、CSRF 和输入密码均不进入日志、DTO、审计或新持久化字段；竞价、命令账本、
拍卖状态、审计追加及 outbox 不变量不变。

新增 `just dependency-audit` 与 CI 独立 `security-audit` job，固定 cargo-audit 0.22.2，
读取锁图并获取 RustSec 公告；本地 `just verify` 不新增网络要求。漏洞、撤回版本与 unsound 提示阻断 CI，
停维等维护提示保留输出，不设置公告忽略列表。当前 `scraper -> selectors -> fxhash 0.2.1`
命中 RUSTSEC-2025-0057 停维提示，暂不在没有解析行为验证的修复中跨版本升级解析器；
2026-11-03 前复查上游兼容方案与验证成本。这是有期限维护债，不是已确认漏洞或永久豁免。
正式 cargo-audit 还发现锁定的 chacha20 0.10.1 已撤回，补升至未撤回的 0.10.2；
这不另算已确认漏洞。依赖解析产生的无关 Windows 依赖边版本变化恢复为原锁图，保持修复范围。

### 既有决定审计

- [首次设置](../feature/2026-09-17-first-run-setup.md)：部分重叠；保留独立表面、令牌、原子发布及未知结果约束，补齐时限与安全头并互链。
- [合并服务](../architecture/2026-09-20-unified-server-supervisor.md)：部分重叠；复用原首次初始化与迁移锁边界，终止不继续下一阶段，互链。
- [上轮源码审计](2026-09-22-source-audit-remediation.md)：部分重叠；原 Public 撤销与资产/恢复理由不变，本次补 Admin 和设置入口并互链。
- [五字段数据库](../feature/2026-09-18-database-connection-fields.md)、[发布范围](../simplification/2026-09-18-release-artifact-scope.md)：无关，不改配置模型或制品范围。
- [文档重建](../process/2026-09-16-documentation-rebuild.md)：仅沿用记录与验证口径，无需修改决定。
- 无完全吸收笔记或过时提案；不覆盖历史理由，不执行归档。

## Alternatives considered

- 只核对 actor：不能覆盖同 actor 更换 secret；只作确定性 HMAC 绑定，则恢复旧 secret 可能让未撤销行再次可用，故还需持久化启动撤销。
- 新建持久化凭据 epoch/校验表：能保留无变更重启的登录状态，但新增敏感认证元数据、迁移和多实例一致性边界；本次选择明确的重启重新登录策略。
- 直接修改 000005/000015：可消除原文语法错误，却破坏发布 checksum，拒绝。
- 仅增加 body 大小限制或只包请求 timeout：不能限制半个请求头、响应慢读和 graceful drain，故复用连接与退出 owner。
- setup 单独复制 CSP：局部改动更少，但会与正式 Dioxus 兼容策略漂移，故提取纯响应头函数，不引入正式业务鉴权。
- 把依赖审计塞入本地默认 verify 或忽略维护提示：前者强加网络前提，后者掩盖债务；保留独立联网门禁及公开维护提示。

## Consequences

7 项审计问题在源码与锁图范围闭合；没有新迁移、配置字段或业务 DTO。代价是 Admin 重启强制
重新登录、部署时必须一致停机轮换，以及 CI 依赖联网的公告数据库。HMAC 仅作用于随机会话 token，
不是新增密码账户存储或 Argon2id 实现。

完整格式/文档/策略、特性编译、Clippy、WASM、MSRV 和依赖审计结果登记在
[实现与验证状态](../../../../docs/status.md)。默认不新增或运行单元、集成、功能、端到端、
浏览器测试或手工冒烟，不启动产品、不连接数据库、不执行迁移；运行证据仍需单独授权。
