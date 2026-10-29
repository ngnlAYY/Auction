# Auction Rust

面向论坛生态的全 Rust 拍卖系统：公开竞价、管理后台，以及处理生命周期、论坛和 MoogleMail 的 Worker。浏览器为 Dioxus CSR，Public/Admin
WASM 分别内嵌；数据库仅 PostgreSQL，Redis 默认关闭。原生程序仅支持 **Linux x86_64**，正式发布 GNU/glibc；语义由本仓库独立定义。
## 从哪里开始
| 任务 | 入口 |
| --- | --- |
| 玩家注册、寄售、竞价、付款或返还 | [中文玩家指南](docs/guide/player-guide.md) |
| 系统设计、接口、契约与页面 | [文档导航](docs/README.md)、[设计与架构](docs/design/architecture.md)、[契约](docs/contracts/README.md) |
| 开发、编译、静态检查与发布 | [开发指南](docs/guide/development.md)、[CI 说明](ci/README.md) |
| 配置、部署、备份恢复、排错 | [运维入口](ops/README.md) |
| 实现范围、历史验证与未验收缺口 | [状态页](docs/status.md)、[AGENTS.md](AGENTS.md)、[文档维护](docs/guide/development.md#文档维护) |

## 目录和根文件有什么用
`apps/` 是薄装配根，`crates/` 按[owner](docs/design/architecture.md#crate-所有权)划分实现；[docs/](docs/README.md) 导航正式文档，
[ops/](ops/README.md) 管配置与部署，[ci/](ci/README.md) 管检查与发布，[migrations/](migrations/postgres/README.md) 只追加迁移。根文件
[Cargo.toml](Cargo.toml) / [Cargo.lock](Cargo.lock) 管工作区、依赖、MSRV 与 profile，[rust-toolchain.toml](rust-toolchain.toml) 固定工具链、WASM 目标，
[Justfile](Justfile) 是命令入口，[rustfmt.toml](rustfmt.toml)、[.editorconfig](.editorconfig) 定义格式，[AGENTS.md](AGENTS.md) 是协作约束，
唯一 CI 工作流是 [.github/workflows/ci.yml](.github/workflows/ci.yml)。
## 运行边界
| 程序 | 职责 | 默认监听 |
| --- | --- | --- |
| [auction-public-api](apps/auction-public-api/README.md) | Public API、会话、实时通知和公开页面 | `127.0.0.1:8090` |
| [auction-admin-api](apps/auction-admin-api/README.md) | 管理 API、会话、后台及独立设置入口 | `127.0.0.1:8091` |
| [auction-worker](apps/auction-worker/README.md) | 生命周期、论坛、邮件与履约 | 无业务 TCP HTTP；可选 Unix control |
| [auctionctl](apps/auctionctl/README.md) | 配置、迁移、systemd 总控与受控灾备 | 无监听 |
| [auction-server](apps/auction-server/README.md) | 可选单文件 supervisor，自管三角色子进程 | 沿用 Public/Admin 两个入口 |
默认分体发布；合并模式仅用于 Linux/PostgreSQL 单主机，auctionctl 仍独立。角色文件用 AUCTION_CONFIG_FILE 选择，合并服务用
--config-dir 并拒绝该环境变量；配置分层、权限、首次设置及停机迁移见[运维手册](ops/README.md)，普通启动不隐式迁移。论坛新主题目录
每帖最多 150 件并连续分页、更新楼位于目录之后，标题为 `[Auction] {世界} Equipment Auction - Session {期数} ({状态})`；首帖倒计时由本站 Rust Public 服务生成，显示计划截止的剩余时长。账户“竞价警告”默认
MM 或论坛 PM 并按领先周期跨渠道去重，受控升级须追加 `000042`；入口支持 Cloudflare 橙云 → Caddy HTTPS → Unix socket，仅静态资源
允许 CDN 缓存，细节见[玩家指南](docs/guide/player-guide.md)与[部署约束](ops/README.md#cloudflare-整站代理)及[技术栈](docs/design/tech-stack.md)。

## 构建与检查
从仓库根目录执行，工具准备见[开发指南](docs/guide/development.md)：
```bash
just verify                       # 完整静态检查，不运行行为测试
just build-api                    # PostgreSQL API-only 制品
just build-release                # 包含两端页面，需要配套 dx CLI
just verify-artifacts             # 资源、依赖和隔离门禁
just release-check --single-binary # 可选合并归档，要求干净工作树
```
apps 默认不含数据库驱动或 Web，自行调用 cargo 须显式选 postgres，需要页面还须先构建并选 web-embed；后端 release 保留 Fat LTO、
单 CGU、panic 展开与通用 CPU 基线，WASM 优先体积。dev 保留行号回溯并减少调试数据，CI 静态检查不生成调试数据；
具体取舍见[编译优化边界](docs/design/tech-stack.md#编译优化边界)，参数不是性能验收。
默认分支推送或手动运行通过 CI 后可下载四程序包，见[CI 制品说明](ci/README.md#下载-actions-制品)。
本地可通过[Blaze 试验入口](docs/guide/development.md#blaze-本地试验)评估函数级增量编译；当前版本存在后端与测试框架
兼容阻断，默认检查、Web 打包和发布仍使用上述入口。

## 当前交付边界
- 新建拍卖默认卖方按成交价全额收款，买方仅享一份奖池基础优惠；预算不足同比缩减，零预算无优惠仍正常结算；
  须追加 `000064` 并更新模板为 v2，已有场次不追溯，见[卖方全额升级](ops/README.md#卖方全额结算升级)。
- 可选赛季早鸟期在异世界赛季更新后的窗口内替代周场加开场次，窗口内投稿使用独立品质，赛季开始前暂停收件；默认关闭，
  须追加 `000065`，见[早鸟期配置](ops/config/README.md#赛季早鸟期)与[早鸟期运维](ops/README.md#赛季早鸟期)。
- 公开目录的稀有/融合分组、等级上限、无等级显示、组合筛选和分页由[装备分组契约](docs/contracts/routes.md#p09p11-装备分组)定义；
  名称拆分、详情类型及历史快照兼容由[装备录入契约](docs/contracts/invariants.md#i-05a-admin-装备录入的敏感边界和原子集合)定义。
- “我的”支持账号登录时长，默认永久，可设置固定时长；当前与后续登录的范围及浏览器限制见
  [登录时长契约](docs/contracts/routes.md#p27p28-登录时长)。启用前须追加 `000058` 并同版更新 Public API/页面。
- 公开装备目录支持列显隐和浏览器本地记忆；公开页容器统一使用可用宽度与较小安全边距，详见[Public 设计](crates/auction-web-public/DESIGN.md#5-表格与信息密度)。
- 两端分别维护桌面／移动展示模块与样式；请求、表单和未决命令继续共享。Public 手机业务表按记录显示卡片，Admin 保持表格。
  文件归属与断点见各端设计及[共享样式](crates/auction-web-shared/README.md#设计变量与组件)；实际验证范围见[状态页](docs/status.md)。
- 公开出价记录与装备详情保留普通报价及代理回应，明确来源，不再隐藏触发代理的普通出价；
  不改竞价规则或公开代理上限，读取与分页边界见[历史契约](docs/contracts/routes.md#标的编号来源与历史读取)。
- 拍卖历史“查看”列支持真实论坛地址，由新增 v2 列表随页返回，旧 v1 字段保持不变；版本与升级顺序见
  [历史论坛链接契约](docs/contracts/routes.md#p30-v2-拍卖历史与论坛地址)。
- 新场次软截止窗口/延长默认均为 900 秒，不改既有显式配置或冻结快照；
  老配置与草稿的处理见[截止升级说明](ops/README.md#截止规则与邮件发送隔离升级)。
- Admin 支持[七类邮件发送间隔](ops/config/README.md#邮件业务发送间隔)，超价 MM 提交后优先唤醒；需追加 `000059`，保存后重启 Worker。
- Admin Cookie 页支持论坛账号密码获取，也可将完整论坛 CK 导入为游戏 CK；密码不持久化，取得的 CK 不回传浏览器，
  共享字段、幂等与挑战处理见[外部凭据契约](docs/contracts/routes.md#外部凭据)。无需新增迁移，替换凭据后仍须重启 Worker。
- MoogleMail 在发送 fence 前只读检查草稿并完成完整 Sent 基线；Bot 内部履约依赖联合身份、正向库存、资金账本及统一现金发送门。
  启用前须[停机同版升级并显式对账](ops/README.md#bot-内部履约与资金保护升级)；旧未知投递仍人工核对，税与内部留存不自动再投资。
- 会话 Cookie 同步与邮件维护轮转升级需追加 `000060`，并同步更新 Public/API、Admin/API 与 Worker；涉及
  邮件维护游标、Admin 预算引用和共享运行目录，按[升级说明](ops/README.md#全源码审计修复升级)处理，不能只替换单个旧进程。
- MoogleMail 的 Inbox、Read Mail、Sent 共享有界读取，写入与资金授权保持独立互斥；所有新发送冻结实际内容摘要并唯一绑定远端 MID。
  需追加 `000061`，按[共享读取升级说明](ops/README.md#邮箱共享读取与发送证明升级)停机同版切换，不补造旧发送证明。
- `000062` 加速发送引用碰撞查询，`000063` 冻结 Take 原账号并对恢复实施身份/fence 核对；
  会话、未决命令、握手超时及启动收口修复须同版更新并刷新页面，前提见[稳定性修复升级](ops/README.md#发送引用索引与稳定性修复升级)。
- 全仓复审保持既有迁移前提，修复早鸟收件窗口与冻结轮次门禁、首次配置生成、跨角色名单一致性、账户读取身份和双端主题层叠。
  生命周期计划借用、日历遍历及 Web/协议解析减少重复分配和复制；发布与工具回执严格绑定完整构建身份。
  修改、实际验证与剩余工具公告见[本次审计](docs/status.md#2026-10-29-全仓复审与配置身份修复)。
- 升级仍须同版替换程序与两端 WASM、刷新旧页面；Public 账户读取新增的可选身份前置条件要求 API 与页面配套。
  数据库读取、实时分发与设备展示的既有边界见[同版升级说明](ops/README.md#读取快照与实时分发优化升级)。

源码、路由和静态编译不构成生产验收。[状态页](docs/status.md)分别登记实现、历史证据与缺口；
真实服务、非空数据、并发、浏览器和恢复流程须另行授权验证。默认不新增或执行行为测试，已有测试保留。
