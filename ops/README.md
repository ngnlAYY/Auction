# 运维：配置、部署与排错

[文档导航](docs/README.md) · [CLI](apps/auctionctl/README.md) · [配置](ops/config/README.md) · [systemd](#systemd-服务与顺序控制) · [恢复](ops/deploy/RECOVERY.md) · [v28 接管](ops/deploy/LEGACY_V28_ADOPTION.md)

原生制品仅部署于 Linux x86_64，正式包使用 GNU/glibc ABI；其他 OS/架构及 musl 不在发布承诺内。默认按 Public/Admin/Worker
分体部署，可选同平台 PostgreSQL 单主机 supervisor。部署前准备真实配置、PostgreSQL 数据库与应用账号；本目录提供审阅模板，
不自动安装或覆盖数据库。以下命令供获准维护窗口执行，阅读文档不授权启动、迁移或行为验证。

## 目录用途

| 文件或目录 | 何时使用 |
| --- | --- |
| [config/README.md](ops/config/README.md) 与 `config/examples/` | 准备公共、三角色、敏感及 runtime TOML；示例也供首次设置生成器使用，不能随意合并 |
| `deploy/systemd/*.service.example` | 选择分体三角色＋总控，或单独合并 server；安装见[下文](#systemd-服务与顺序控制) |
| [共享运行目录模板](ops/deploy/tmpfiles/hv-auction.conf.example) | 分体部署由 tmpfiles 独立预建 HTTP 共享根；不能由 Worker 停止时删除 |
| [业务 Caddy](ops/deploy/caddy/Caddyfile) / [临时设置代理](ops/deploy/caddy/setup.Caddyfile.example) | 业务入口与远程首次设置分别审阅，后者不默认开放 |
| [恢复](ops/deploy/RECOVERY.md) / [v28 接管](ops/deploy/LEGACY_V28_ADOPTION.md) | 当前库灾备 / 限定历史库接管；不是普通启动步骤 |
| [retention-policy.v1.json](ops/deploy/retention-policy.v1.json) | 编译嵌入的保留盘点契约，不授权删除数据 |
| [build/README.md](ops/build/README.md) 与两份 CPU TOML | 指定 AMD KVM 的可选构建候选及历史证据；不是通用发布配置 |
| [mail-diagnostics.md](ops/mail/diagnostics.md) | 邮件拒收、未知结果、加密证据导出及历史资产核对 |

## 本页导航

- [可选单文件部署](#可选单文件部署)
- [推荐目录](#推荐目录)
- [发布制品](#发布制品)
- [安全更新与整机重启](#安全更新与整机重启)
- [首次部署配置](#首次部署配置)
- [启动顺序](#启动顺序)
- [邮件详情恢复与退件消费升级](#邮件详情恢复与退件消费升级)
- [截止规则与邮件发送隔离升级](#截止规则与邮件发送隔离升级)
- [Bot 内部履约与资金保护升级](#bot-内部履约与资金保护升级)
- [旧 Bot 结算的显式核对](#旧-bot-结算的显式核对)
- [Public 登录时长升级](#public-登录时长升级)
- [邮件发送节奏与 Sent 准备升级](#邮件发送节奏与-sent-准备升级)
- [全源码审计修复升级](#全源码审计修复升级)
- [邮箱共享读取与发送证明升级](#邮箱共享读取与发送证明升级)
- [发送引用索引与稳定性修复升级](#发送引用索引与稳定性修复升级)
- [卖方全额结算升级](#卖方全额结算升级)
- [论坛镜像](#论坛镜像)
- [全仓复审与设备展示拆分升级](#全仓复审与设备展示拆分升级)
- [读取快照与实时分发优化升级](#读取快照与实时分发优化升级)
- [数据库恢复与保留](#数据库恢复与保留)
- [反向代理与 systemd](#反向代理与-systemd)
- [Cloudflare 整站代理](#cloudflare-整站代理)
- [systemd 安装、探针与权限](#systemd-服务与顺序控制)

## 论坛镜像

论坛目录中的游戏装备直链及装备 key 是公开数据；数据库镜像和备份中的这些链接不构成隐私泄露。
论坛正文继续保留固定帖身份、内容版本及未知写入对账。完整数据库备份仍按其中其他敏感资料的保护规则保管。

## 卖方全额结算升级

先保存可验证数据库恢复点、旧程序和配置，停稳全部旧 writer，再用同版 `auctionctl` 追加
`000064_seller_full_settlement`；普通启动不执行迁移。同版更新 Public/Admin API、Worker、auctionctl 及配套页面，
合并部署同时更新 server；旧程序不认识 `hold_curve_v2`，不得与新结算混跑。
迁移只替换既有三张表的 CHECK，不新增表或重写经济行；保留旧 v0/v1 快照、金额、义务、账本和投递。

新建模板默认 `version=2/mode=hold_curve_v2`。升级前显式把公共 `[auction.settlement]` 的旧 version/mode 改为 2/v2，
复核 Admin/Worker 有效模板一致；保留旧曲线锚点。新建模板拒绝 v1，不自动改部署文件或既有草稿快照。
已创建而尚未结算的 v1 场次仍按原冻结规则；如果要将其切换为 v2，须另行审查目标与显式命令，本轮不提供批量改库。
已结算场次不补发差额、不修改旧账或重复发送。

v2 保持卖方应收等于成交价；买方仅享一份基础优惠，由世界奖池承担，不追加额外优惠。预算不足时，
按全场名义优惠权重同比缩减，每件向下取整、余款留池；零奖池无优惠仍正常结算，不为等待优惠预算保持 settling。
新买家账单用显式 `pricing_mode=hold_curve_v2` 的精确九变量区分名义曲线和实分基础优惠；旧三/八变量原摘要及正文不变。
本轮不自动充值、发行预算或把 Bot 余额视为奖池；补充预算须另行提供合法来源与运维方案。
逻辑奖池足额也不替代现金用途授权、同账号新鲜证据或原发送 fence；配置保存与静态检查不表示线上已采用新规则。

## 发送引用索引与稳定性修复升级

先保留可验证数据库备份、旧制品、配置及 unit，停止旧 writer，再用同版 `auctionctl` 追加
`000062_delivery_reference_index` 与 `000063_delivery_take_expectations`。前者只为非空外部引用建立非唯一部分索引，
不清理历史重复；后者只追加新 Take 的原账号/MID 尝试，不回填旧凭据或成功证明。
索引创建需要扫描已有 deliveries，大表须预留磁盘和维护窗口。普通服务启动仍不执行迁移。

同版更新 Public/Admin API、两端 WASM、Worker 和 auctionctl，合并模式同时更新 server；刷新旧标签页，禁止混用旧 Worker。
本次会话与未决命令修复不改变 HTTP 字段、命令身份或服务端权限；未知外写不能因升级而重发。
WebSocket 的升级前握手受原 `request_timeout_seconds` 约束，升级后的连接仍按实时协议的会话、写入及 drain 预算运行。
启动期收到终止信号会取消初始化；readiness 只接受完整 CRLF 终止的成功状态行，不再把截断响应当作就绪。
工具独立锁变化须重新执行[受控工具安装与校验](ci/tools/README.md#安装与验证)，不能沿用旧回执或旧工具二进制。
这些要求不表示目标环境已升级，实际验证与剩余风险见[状态页](docs/status.md)。
旧无冻结账号的未知 Take，以及配置 Cookie 没有持久绑定的未知 Take，升级后仍须人工核对。
只有与原 credential ID/version 一致、当前 generation 有效且原 fence 未被替换的观察才能恢复；不因当前账号查无 MID 而认定旧账号已领取。

## 邮箱共享读取与发送证明升级

先保存可验证数据库备份、旧程序、配置及 unit，停稳全部旧 writer 后，用同版 `auctionctl` 追加
`000061_delivery_send_expectations`，再启动同版 Public/Admin/Worker/auctionctl；合并部署同时更新 server。
普通启动不隐式迁移，旧 Worker 不得与新发送协议混跑。须重建并更新 Admin WASM、刷新旧标签页；
旧浏览器 decoder 不接受新增的明确准备失败时间链，不能只替换 API 而保留旧页面。

迁移只新建发送尝试摘要、唯一远端 MID 绑定和相应索引/追加保护，不回填旧邮件正文、成功证明、凭据身份或资金结果。
新发送在写入远端前冻结实际渲染摘要；未知发送只能按原尝试核对，不得在升级后批量重发。
Read Mail 只帮助定位已持久登记的待读邮件及动作恢复，不把历史已读邮件导入为新注册、寄售或退件命令。

Inbox/Read Mail/Sent 普通读取与 compose 分离；共享观察有独立的并发、字节和时效上限，不改变部署请求间隔。
资金/库存联合证明仍绑定原凭据、generation、单调截止和提交 guard，不能以共享缓存代替。
重启会清除易失观察，不清除发送尝试、原有游标、资金占用或未知结果；凭据更换仍须按原 owner 交接。
回退只允许使用理解新发送证明与绑定约束的制品，禁止删除新表或绕过证明。实际验证范围见[状态页](docs/status.md)。

## 全源码审计修复升级

本轮修复的源码与静态证据见[状态页](docs/status.md#2026-10-23-全源码审计修复静态验证)。未执行迁移或部署，
不能将下列说明视为目标服务器已经更新：

1. 先保存可验证的数据库备份、旧程序、配置及 unit，再按既有分体总控停稳全部旧 writer，同版 CLI 追加 `000060`。
   新表只保存邮件维护尝试的环形游标，不修改旧投递、资金、命令或人工核对状态，也不是远端完成证明；禁止编辑既有迁移。
2. 同版更新 Public/Admin API 与各自 WASM、Worker 和 auctionctl；合并模式同时更新 server。P06 GET 不再写 Cookie，
   新增空正文、无 query 的 `PUT /api/v1/session` 显式同步。须刷新已经打开的旧页面，防止旧客户端仍发起无协调的 Cookie 写请求。
   新页面使用安全上下文中的原生 Web Locks，HTTPS 或 localhost 且浏览器支持该 API 才能登录、退出、同步及保存登录时长；
   不支持或锁等待超时会明确拒绝本次请求，不用假锁降级。锁仅协调同 origin 的升级客户端，不提供跨 origin 或恶意客户端保证。
3. 需要分别接管 `operations.mail` / `operations.worker` 时，在 Admin 角色 TOML 配置 `worker_config_file` 指向本部署 Worker
   非敏感角色文件；无需让 Admin 读取 Worker secrets。引用校验和有效预算使用 Admin 启动快照，修改 Worker 基值后重启相关进程；
   未提供基值时单组 managed 的新候选明确拒绝，既有文件读取和 durable 命令恢复不改写。
4. 分体部署安装[共享运行目录 tmpfiles 模板](ops/deploy/tmpfiles/hv-auction.conf.example)，并按
   [systemd 安装顺序](#systemd-服务与顺序控制)先停旧 unit，再改 Worker 的 RuntimeDirectory。`/run/hv-auction` 由 tmpfiles 拥有，
   Worker 只拥有 `/run/hv-auction/worker`；这不取消既有 Requires 关系或承诺 Worker 重启时 API 零中断。
5. 预检要求三角色的装备密钥 ID/材料一致，其他凭据密钥仍按各自 owner 比较；修正不匹配配置后再启动，
   不通过临时关闭解密、未知外写门禁或资金证明来绕过失败。

旧 prepared 退款现在同样需要同账号、正确世界的真实 Armory 正向库存；无证明的任务继续等待或人工核对。
旧 COD、退件和论坛命令不批量重发，既有未知结果不因为本轮修复而变成“未执行”。维护游标和 discard 围栏也不授权补造历史回执。
`realtime.data.total_lag` 为新增必填汇总字段，不能混用旧 Admin 页面与新 API；完整接口、配置及安全边界见
[路由](docs/contracts/routes.md)、[配置](ops/config/README.md)、[不变量](docs/contracts/invariants.md)。

## 邮件发送节奏与 Sent 准备升级

同版更新 Public/Admin/Worker/auctionctl，先保存可验证数据库备份、旧程序/配置和 unit，按分体总控停稳全部旧 writer 后
追加 `000059`；七类发送起点只从既有 sending_at 初始化，不改旧投递/命令/人工核对状态。新 Worker 依赖该表和映射函数，
不能先启动程序再补迁移；回退也须保留新表与高水位，禁止删除它们来跳过间隔。

管理页字段见[配置手册](ops/config/README.md#邮件业务发送间隔)。升级不默认开启 managed 或改各类间隔；已保存非零新字段后，
只回退到支持该配置/schema 的同版兼容制品。Sent 的页数和总预算须一起调整，并检查有效来源：mail managed 控制读取参数、
worker managed 控制 delivery 参数。发送总预算仍须大于两倍 fetch 预算，grace 必须大于发送预算；租约可由原续租路径延长，
资金证据仍保持首次读取后最长 30 秒的原截止。准备阶段页数/时间耗尽只延后，写后失败仍未知；不以错误码批量重发历史记录。

论坛标题改名只在正常发布/刷新时应用，保留旧标题精确核对；不清除未知写入门禁、不强制重发主题，已完成终态刷新不批量重开。
发布核验只确认程序/配置/schema/资源及就绪状态；发送和论坛业务验收须另行授权。

## 可选单文件部署

`just release-check --single-binary` 输出 `target/dist/package-single/`，可执行文件仅 auction-server、auctionctl；server 派生三个
独立角色进程。服务账号执行 `auction-server --config-dir /var/lib/auction/config`：仅目录明确不存在时开放向导，网页明确确认后
才保存、初始化预备专用空库并启动；普通启动不隐式迁移，只校验 schema。

仅启用[合并 unit](ops/deploy/systemd/auction-server.service.example)，同库禁并行分体或远端角色；合并锁与分体停机门禁互不替代。
路径/锁、远程 HTTPS 设置、失败处置及 `migrate-managed` 见[server 手册](apps/auction-server/README.md)。托管迁移必须
在同一次打开流程中取得现有 runtime 目录与 `owner.lock` 的独占锁，核对属主、权限和 inode，并持有到迁移结束；
目录或锁缺失、替换或竞争时拒绝，不能降级为无锁迁移。持锁期间仍须保持目录及锁路径稳定。
首次空库初始化和普通启动前，Unix 监听路径只允许不存在，或为当前服务账号拥有且连接明确拒绝的旧 socket；
活动 socket、其他文件、属主不符、连接超时或路径替换均阻止继续。预检不删除路径，实际绑定仍复核，预检后须保持目录稳定。


## 推荐目录

分体部署示例：

```text
/opt/auction/bin/                 # Public、Admin、Worker、auctionctl
/etc/auction/common.toml
/etc/auction/public.toml
/etc/auction/admin.toml
/etc/auction/worker.toml
/etc/auction/public.secret.toml
/etc/auction/admin.secret.toml
/etc/auction/worker.secret.toml
/etc/auction/runtime/admin.toml
/var/lib/auction/deployment/       # 总控锁与启动回执
/var/lib/auction/                  # 已配置的私有运行数据
/run/hv-auction/                  # Public/Admin HTTP Unix socket
/run/hv-auction/worker/           # Worker control socket
```

三个 common_file 指向同一只读普通文件；三角色 runtime 须同源且装备规则快照一致，或全部省略。敏感 TOML 仅对应服务账号可读，父目录不得 group/other 可写，Admin runtime 按 0640
管理。连接串、Cookie、登录 key、数据密钥不得进入 unit、参数、代理配置或日志。三角色须同一数据库目标，可用不同账号；
统一规范主机名或 Unix socket 目录，显式填写 host/port，目标比较及未知参数/options 拒绝规则见
[部署预检](apps/auctionctl/README.md#部署预检)。runtime 共享、operations 接管、恢复指纹变化后的重新部署重启见
[配置手册](ops/config/README.md#runtime-共享与重启)；进程存活不证明配置生效。
Admin 配置 `worker_config_file` 时，预检要求它指向本部署实际 Worker 角色文件，仅用于非敏感预算快照；
不要求 Admin 读取 Worker secrets。已有独立 managed 部署须先补充该引用并重启 Admin，再保存依赖另一组部署基值的新候选。

## 发布制品

生产使用干净工作树的 `just release-check` 归档；Actions 在默认分支推送或手动运行通过全部检查后复用同一入口，提供通用
Linux x86_64 四程序包，下载、14 天保留期及 ABI 边界见[CI 制品说明](ci/README.md#下载-actions-制品)。下载后仍须核验清单与
部署兼容性，不会自动更新服务器。也可在本地逐步执行：

```bash
bash ci/tools/rustup.sh
bash ci/tools/bootstrap.sh install release
just build-api
just build-release
just verify-artifacts
just package
```

分体归档 `target/dist/package/` 仅含四程序、配置示例、systemd/Caddy/tmpfiles 模板、恢复文档、retention policy、依赖摘要和 payload
清单；包内 SHA256SUMS 绑定全部批准文件，外层清单绑定 archive/依赖摘要。不含数据库、Cookie、.keyenv 或运行数据；解包后向
PostgreSQL 五字段敏感模板注入真实值。API-only 无 Web 资源；内嵌档 Public/Admin 仅含各自 payload，Worker/auctionctl 无页面。
API-only、内嵌和合并变体的中间制品目录另有 `SHA256SUMS`，覆盖构建信息、全部程序及适用的 Web 清单/标记；
复用或装包前重算摘要，缺失或不符时须重建。该缓存清单不进入归档，包内与外层校验仍使用各自的既有清单。
源码身份须构建前冻结、各阶段复查；正式 package 拒绝脏树，`--allow-dirty` 仅供受控本地取证，不是上线批准。归档保留
`systemd/README.md` 作为指向 `docs/DEPLOYMENT.md` 的导航，安装说明只维护本页；完整发布门禁见[CI 说明](ci/README.md#发布边界)。

后端沿用[根 release profile](Cargo.toml)：opt-level=3/Fat LTO/单 CGU、panic 展开，通用包禁用构建机 `native`；WASM 以体积
为先，LTO 增加构建时间和内存，不证明性能收益。[AMD KVM 定向候选](ops/build/README.md)不进入通用归档，须核对资源身份、
CPUID/ABI 和运行证据；型号或 cargo check 不证明兼容性/性能。

### 性能与页面生命周期修复升级

2026-10-24 的性能／生命周期修复自身不新增迁移；当前完整版本仍要求已发布的 `000001`–`000064`，
不得跳过[Take 身份](ops/README.md#发送引用索引与稳定性修复升级)或[卖方全额结算](ops/README.md#卖方全额结算升级)的前提。
按原停机、备份、预检与同版发布流程更新 Public、Admin、Worker 和 `auctionctl`，两端 WASM 必须与对应 API 一起重建，
并刷新已有浏览器标签页；旧页面不会自动获得身份 Scope、查询恢复或未知命令修复。

自动 Take 维护的领取、收口和过期隔离保留已消费重排次数，Admin 显式重试仍按原规则重置；
不推断或回填旧版本已丢失的计数，不重发未知 Send，也不补造历史 Take 凭据证明。
构建输入/环境指纹改进会使旧复用标记失效，不能手工改写回执认可旧制品。
本轮隔离浏览器使用调试 WASM，不可替代正式 `just release-check` 的发布 payload；
验收范围与剩余风险见[状态页](docs/status.md#2026-10-24-全项目复审与通用构建优化)。

### 全仓复审与设备展示拆分升级

2026-10-25 复审不增加迁移、配置或 HTTP/事件字段。按原备份、停机和同版发布流程重建三角色与 `auctionctl`，
合并部署须同版更新 `auction-server` 与独立 `auctionctl`，不能只替换分体角色程序。
Public/Admin 分别携带同版 WASM 与全部 CSS asset，并刷新旧标签页；桌面／移动规则的优先级由 CSS 层固定，
不能只拷贝旧主 CSS 或把未验收的页面构建描述为移动设备验收。

论坛 Cookie 的失效现在同步撤销已派生的会话；新进程必须重新取得合法凭据及运行时 owner。
MoogleMail 写后 Sent／compose 检查中的明确登录失效保留原原因和证据，但投递仍按未知写入人工核对，
升级不授权重发、清理草稿或恢复旧凭据。已发出的远端请求不能被本地撤销追回。
Worker 启动门禁阻止在初始化期间已经收到停止请求后派生新循环；原在途 drain 预算与租约策略保持。
结算失败的退避起点移到目标行锁之后，邮箱维护先限定候选批次再关联证明，不放宽页外歧义或 CAS/fence。
实际静态证据和未验证的数据库／浏览器／外部协议边界见[状态页](docs/status.md#2026-10-25-全仓复审与桌面移动展示分离)。

### 读取快照与实时分发优化升级

2026-10-26 性能与稳定性复审不新增迁移、配置键、HTTP 字段或事件字段，当前版本仍要求已发布的 `000001`–`000064`。
存储层的事务开启方式、公开只读快照和会话活动记录由 Public、Admin、Worker 与 `auctionctl` 共用，按原备份、停机和
同版发布流程一起替换；合并部署同版更新 `auction-server` 与独立 `auctionctl`。

- 全部事务入口的隔离级别与只读模式随 `BEGIN ...` 声明，Serializable 锁序、三次重试与 Read Committed 队列领取保持原义；
  Public 目录、详情、竞价日志、成交历史、账户和补偿池读取使用 Repeatable Read 只读快照，不再持有 SSI 谓词锁。
- Worker 主循环的 `[worker].poll_interval_seconds` 改为最长等待：生命周期阶段、慢速代理或结算存在严格未来的到期／重试
  时刻时，按只读提示在该时刻提前唤醒，开拍、最终阶段、慢速代理与结算不再最多延后一个周期。领取仍由原锁内谓词决定，
  已过期未领取事项和提示失败继续按原周期重试；升级后观察 Worker 日志与 schedule delay 指标确认节奏符合预期。
- 竞价、生命周期、录入、结算与恢复追加公开事件时以单条持锁 upsert 分配 stream 序号，事件内容与序号连续性不变。
- Public relay 每轮只列出头部序号超过本 consumer 游标的 stream；失败事件仍不推进游标并按原节奏重试。
  进程内 Hub 每个事件只校验、编码一次，非法事件在广播前被拒绝并记入 failure，不再以 1002 关闭既有连接。
- Public 会话认证与只读复核共用同一谓词，`last_activity_at` 只在距上次记录满 60 秒时写入；有限期限仍不随活动续期。
- Public/Admin 的控件边界、错误文字对比度、`prefers-contrast` 适配及规则正文阅读行宽属于 CSS/译文变化，
  须同版重建两端 WASM 与全部 CSS asset 并刷新旧标签页。

实际静态检查范围与未执行的数据库、浏览器和外部服务验证见[状态页](docs/status.md#2026-10-26-读取快照与实时分发复审)
及其[后续收敛记录](docs/status.md#2026-10-27-worker-到期唤醒事务入口收敛与规则阅读行宽)。

2026-10-27 的[全仓性能精简](docs/status.md#2026-10-27-全仓性能稳定性与依赖复核)沿用同一升级流程，不新增迁移、配置字段或外部协议。
代理写入、告警恢复、履约摘要及 tracing 优化随原生程序生效，倒计时与 UUID 解码优化须重建 Public WASM 并刷新页面。
本轮只完成源码与静态检查，未执行发布或改变线上进程；已有未知外写、资金与恢复门禁保持原义。

后续[最新源码复审](docs/status.md#2026-10-27-最新源码注释文档与依赖复审)沿用该流程：装备选择器、配置判重、恢复预检与
追踪 feature 精简随原生程序生效；共享 Markdown 和 Admin 提交编号展示优化须重建两端 WASM 并刷新页面。

[资源生命周期复核](docs/status.md#2026-10-27-全仓精简与资源生命周期复核)继续沿用同版升级流程：邮件解析、结算证据、
Recovery 重试、配置投影、Admin 事件分页和指标句柄优化随原生程序生效；可见性监听器自动卸载、Markdown 与 Admin
解码优化须重建两端 WASM 并刷新页面。无新增迁移、配置键或业务接口，资金、外写未知结果及历史证据仍按原门禁处理。

[批量持久化与边界复核](docs/status.md#2026-10-27-批量持久化与边界复核)同样不新增迁移：待读邮件批量登记、Worker drain
复核、首次设置安全头与配置预检随原生程序生效；命令/分页响应绑定和词典判重须重建两端 WASM 并刷新页面。
设置代理须发送单值 Host/Origin/HTTPS 证明头；非法日志过滤器在配置预检即失败。构建工具配方已补齐 Cargo 环境别名，
构建机需重新执行[工具安装与回执校验](ci/tools/README.md#安装与验证)，再按原同版流程生成和切换制品。

## 安全更新与整机重启

依赖修复同时核对源码锁、构建工具、已安装程序和运行进程。Rust 静态依赖须重新构建发布；进程持有的 TLS 客户端和
信任集合须通过受控重启刷新。构建工具使用[独立依赖锁及安装回执](ci/README.md#发布边界)，同版本旧二进制不得仅凭版本号复用。
系统组件以发行版修复状态为准，记录实际运行内核、代理所用 Go/模块版本及生效的 SSH 配置。

内核更新后，在已授权维护窗口按以下顺序重启分体部署：

1. 保存并核验应用、配置、unit 及数据库备份，核对当前 schema 和所有已发布迁移校验和；记录 boot ID、三个服务的原启用状态和启用链接。
2. 持久禁用三个应用服务的自动启动，再通过 `auctionctl deployment stop` 反序停稳，确认 Worker 正常 drain、数据库无遗留 writer，
   runtime 命令账本没有 pending/unknown。准备完成回执落盘后才重启主机；`--runtime` 禁用或 mask 会随重启失效，不用于此流程。
3. 新 boot 中核对目标内核、原制品及配置；确认应用未提前自启，通过总控重新按 Worker → Admin → Public 启动，复核启动回执、
   readiness、进程哈希和稳定性，再还原原启用状态及链接。同步核对代理、SSH 和公开静态资源。

禁用且停止的 unit 可能被 [systemd 回收](https://www.freedesktop.org/software/systemd/man/latest/systemd.unit.html#Unit%20Garbage%20Collection)，
退出字段随之清空。停机前记录 boot、PID 和 InvocationID；若回收已发生，以同一进程的 drain 开始/完成日志和 PID 1 的成功停止事件
核对正常退出，并保留原失败回执及补证链，不能把清零字段直接当作成功。读取停止契约时，在同一 D-Bus 连接上通过
`Manager.RefUnit` 持有单元引用，再核对依赖、停止 hook、信号及超时，结束时释放引用并关闭连接，避免多次属性读取之间发生回收竞争。
hook 按 D-Bus 类型和空数组校验；`systemctl show --all` 仍可能省略空的 `ExecStop`/`ExecStopPost`，不能把缺字段直接当作空 hook。

当官方代理制品尚未包含上游修复时，可在同一官方源码版本上仅调整所需依赖和编译器，保存来源摘要、依赖差异、扫描及部署回执。
这类自建 Caddy 制品的包管理版本号可能不变；后续安装官方包前须核对其 Go 和模块修复基线，避免覆盖为旧依赖制品。
SSH 压缩相关缓解通过独立 drop-in 设置 `Compression no`，先运行 `sshd -t` 和 `sshd -T`，reload 后另建连接确认生效。
每次执行结果另记[状态页](docs/status.md)，不把文档步骤当作已完成维护。

## Bot 内部履约与资金保护升级

以下为显式维护流程，执行需要已获授权的维护窗口；迁移与账户初始化分别执行。具体制品、上线及修复证据见[状态页](docs/status.md)。
账户默认 `strict_cash`；自行使用 Credits、仅需记录拍卖账目的账号可显式初始化为 `record_only`，其流程见下文。

1. 核对精确制品/数据库目标并取得可验证备份，停止全部相关 writer、旧 Worker 和可能持有 mail owner 的 CLI。分体/合并遵循
   现有停机迁移门禁；所有相关程序必须同版，禁止滚动混用旧 Worker 绕过资金门；普通启动不隐式迁移。
2. 用同版 auctionctl 显式迁移。旧 `000039` 环境顺序处理 `000040`—`000048`，不跳号、不改历史 checksum；`000046` 为全局
   mail generation，`000047` 为资金账本，`000048` 为内部命令/证明，迁移不回填资金或成功事实。须使用同时包含 44 延迟约束
   与 47 保留字列引用兼容的配套程序；若旧尝试已停在 46，保留备份/日志及停机门禁，由修复版从 47 继续，不手工建表、
   登记迁移或修改已发布文件/checksum。
3. 联合身份要求已启用论坛读取能力、数据库 active mail 凭据 ID/version 与当前认证 HV UID。旧预期名 None 可真实核验后识别，
   有值精确匹配；不用删除重导入旧 Cookie，配置/keyenv 不具备内部提交身份。论坛关闭时新内部准备和现金证据延期，
   Isekai 独立证据不足时保持等待。
4. `strict_cash` 使用[bot-funds](apps/auctionctl/README.md#bot资金对账)先只读预览责任、版本、blockers 和摘要；preview 不取 owner、不推进
   generation、不观测远端。apply 才取得独占 mail owner/generation 和同 session 联合身份、Main 现金、clean compose，因此须先
   停止持有 owner 的 Worker；必须提供稳定命令、版本/摘要、精确确认及分别指定的自有/补偿用途额度，未知提交仅按原命令和
   原负载核对。
5. `strict_cash` 对账前确认专用账号无并发人工/其他程序资金写入、无未登记外部负债，完整核对既有应付、退款替代责任、税/补偿、出站和
   未知。旧无 UID 责任仅保守隔离，跨 opening 历史收款不成为新可消费来源，未知或无法界定责任阻断开额度。Main 装备 COD 或
   跨世界现金 carrier 已外发而收款事实未闭合时，preview/apply 以 `payment_collection_unclosed` 阻断 opening，付款关联缺失
   同样阻断，不重建 opening 或改旧队列绕过。补偿与隔离的新增需求最低为零，既有 allocation 仍完整扣占；余额不是自由现金，
   retained/税不自动再投资，发现账实缺口冻结新增支出。collected 核对在满足/发行/解依赖前检查同一现金证据，缺口确定提交
   冻结并延期，不把冻结随业务错误回滚。
6. 用户确认升级、对账与外部前提后才启动同版程序。内部满足只处理合格新 pending，保留原 B+C=S+T、payment→handoff→seller，
   退款仍需原恢复授权，未新增内部取消退款；有资金绑定的新五类 Admin delivery 动作拒绝，原成功命令仍先重放，本轮没有
   人工返还或 unknown 重发入口，不能用 failed/TTL 释放资金。

装备占用按已识别的 origin/payload schema 与真实 EID/world 判定；已验证无装备通知不占用目标装备，未知/畸形 schema 及真实
装备冲突仍阻断，历史 succeeded 不独立授予放行。现金 sending fence 沿 prepared 原单调绝对截止，并取有效租约/finalize 截止
的最小值；数据库用 PostgreSQL 18 `transaction_timeout` 覆盖锁、审计及 COMMIT，重试不重置预算，过晚/错误/取消或提交未知撤销
owner 不发送。旧 32 条未知投递及残留草稿仍须逐条人工证据核对，禁止自动清草稿、领取、重寄、强制结清或改写旧任务；
按 UUID 锁全部 deliveries 的内部冲突扫描和 ready 义务预锁须独立审查规模/事务时限。真实 Main/Isekai 协议、迁移/并发及资金
对账运行证据见[状态页](docs/status.md)。

`000055` 为 UID 资金账户增加持久策略，默认 `strict_cash`，不自动开户或改变历史分录。
仅记账账号使用 `auctionctl bot-funds-record-only --expected-uid <UID>` 获取数据库预览，再以预览要求的稳定命令、
操作者、版本、摘要及逐字确认执行。Worker 须停止，CLI 仍核实真实联合 UID 和干净 compose，执行不读现金余额，
也不要求账号所有人停止个人用款。该命令仅初始化缺失账户，已有真实现金账户须另行设计专门转换流程。
内部购买及补偿按每笔原冻结金额建立唯一有限的会计来源；旧已知责任先登记隔离用途，精确收款证据核对后再认领。
跨世界外部付款的实际 1C 载体从原 Isekai 义务及完整 payload 逐 delivery 登记，不依赖个人现金预算；内部付款不寄载体。
账目不表示整号可用现金；卖家款仍按真实投递和领取证据推进，原标的未知结果及装备冲突继续保留。

## 旧 Bot 结算的显式核对

`000053`/`000054` 仅增加只追加证明表，迁移不更改旧任务或资金。专用 `auctionctl legacy-settlement`
与普通重试分开：默认只读数据库，执行才取得原网关的排他 mail/forum owner、推进 generation 并读取真实证据。
同 ID/同负载重放原结果，提交未知按原命令查询，不更换 ID 重试。

1. 核对目标数据库、制品与备份，停止全部相关 writer 后迁移并同版更新；准备冻结的单笔预览、稳定命令与完整执行参数。
   CLI 实际修复期间 Worker 保持停止，防止同时争抢凭据或改变领取/资金责任；执行不寄信、不领取、不清草稿。
2. `legacy-settlement self-send --obligation-id <UUID> --expected-uid <UID>` 预览旧自寄。执行须原投递及履约版本、完整摘要、真实 UID、
   操作者、原因与精确确认；同账号正向库存和干净 compose 通过后，流拍装备直接留存。成交自寄还要求原明确拒绝及
   已清理草稿证明，仅恢复内部付款准备，旧发送证据与全部原指针保存在只追加快照；不把拒绝寄信记成付款。
3. 按所选账户策略重新预览全部责任：`strict_cash` 按[Bot 资金对账](apps/auctionctl/README.md#bot资金对账)核实外部责任、
   现金和用途额度；`record_only` 使用上节专用命令，只核对身份和拍卖责任。零资产纯通知必须严格解码后才可排除资金
   未知阻塞，其原人工核对状态保留；未知现金或装备责任仍阻断。
4. `legacy-settlement adopt-collected --obligation-id <UUID> --expected-uid <UID>` 预览旧 Bot 卖家款。单笔执行绑定
   原付款、投递、MID、收件人、冻结金额与隔离来源边界；同 session 精确 Sent-collected 及对应账户策略通过后，从既有隔离
   用途转为该标的卖家留存，不重复发行 B、不增加自有额度。旧付款和历史跨 opening 收款不经普通新收款路径重放。
5. 核对每条追加证明、资金分录、义务及原投递，按顺序启动同版角色；确认 readiness、内部付款及后继卖家款的真实状态。
   未取得真实库存、Sent、资金或账号前提的条目保留原状态；专用命令不接受管理员声明替代远端事实。

接口及精确参数以[auctionctl](apps/auctionctl/README.md)和命令预览输出为准，应用与 SQL owner 见
[旧结算端口](crates/auction-application/README.md#旧结算显式核对)及[数据库对象](docs/contracts/schema.md#bot-内部履约对象)。

## 截止规则与邮件发送隔离升级

本轮仅交付源码和静态检查，以下由用户在另行批准的维护窗口手动执行；不自动操作现有实例。

1. 核对制品、数据库目标及可验证备份，按现有分体停机门禁或合并 `migrate-managed` 流程停止全部旧 writer，使用配套
   auctionctl 追加迁移；当前已扩展到 `000048`，须同时遵守下节 Bot 升级门禁，保留 `000044` 时间修复及既有 checksum。
   普通重启不迁移，不混跑旧程序。
2. 核对 [managed 的有效 economics 来源](ops/config/README.md#缓存与拍卖模板)：未接管时修改公共模板，接管时修改有效 runtime
   economics，使 `soft_close_window_seconds` 与 `soft_close_extension_seconds` 均为 900。程序新默认不会覆盖用户文件里的
   显式 60；按[共享配置重启](ops/config/README.md#runtime-共享与重启)使 Admin/Worker 生效。
3. 对每个需要更正的旧草稿先运行 [repair-draft-soft-close 默认只读检查](apps/auctionctl/README.md#草稿软截止显式修正)，
   核对 `eligible`、`blockers` 与当前版本；保留稳定 command_id/完整负载，再用 execute 和精确确认显式修改唯一目标。
   不自动遍历，不删发布证据强行过门禁，不修进行中/已结算场次，不追溯出价或重开旧场次。
4. `000045` 只保存新导入 mail Cookie 的可选预期完整玩家名称，旧记录保持 NULL、原 ID 与密文；当前没有安全补绑定入口，
   不要删除重导入旧凭据或改 rename 语义以绕过，保留历史未知投递的身份与证据。
5. 新 Worker 在 sending 前读取真实 compose，脏草稿使对应 world 的 Send 本轮延后并释放有效 claim，不消耗一次发送、不把每个
   新任务推进 unknown；Take/Return 独立，下轮重新观测。它不会自动清理远端草稿或修复旧任务。

bot 内部履约已接独立联合身份、库存及资金证据，启用遵守上节；管理员声明、Cookie 检查成功或装备详情可访问不能替代证明。
自寄拒绝后已暂存的资产仍按未知结果保守处理；事故旧 32 条 sending/manual_reconcile/outcome_unknown 及草稿装备保持人工核对，
禁止自动清草稿、重寄、领取附件或强制结清。状态干净不能证明历史未知邮件未发。业务门禁见[不变量](docs/contracts/invariants.md)，
证据与运行缺口见[状态页](docs/status.md)。

## 首次部署配置

以实际服务账号准备 PostgreSQL 18 空数据库及应用账号，在该账号拥有的受信父目录运行：

```bash
/opt/auction/bin/auction-admin-api setup --output-dir /var/lib/auction/initial-config
```

目标目录必须不存在。无默认配置且未设置 AUCTION_CONFIG_FILE 时，直接运行内嵌 PostgreSQL Admin 可生成当前目录的
auction-config/；显式配置缺失、残缺目录、符号链接或权限错误均不降级为向导。设置监听 `127.0.0.1:8091/setup`，远程用 SSH
隧道；正式 Admin 无 setup 写接口。

1. 验证终端一次性部署码，填写数据库五字段、管理员和精确站点 origin；令牌不存浏览器。
2. 检查连接/候选（不写文件），保存时重新验证；改参数即清除旧检查，令牌失效须重新验证并填写密码。
3. 保存公共、角色、敏感及共享 runtime TOML 后退出，不迁移或启动其他角色。
4. 审阅并安装配置，按[启动顺序](#启动顺序)准备目录/用户组/unit/代理、严格预检及受控迁移启动。

正式 Public/Admin 端口默认 8090/8091，可设 1–65535 且不同，生成 loopback listener；不改变当前 setup 端口或 origin。
同步调整代理/隧道并核对占用和低端口权限，设置检查不会试绑定。周排期默认关闭，可设 IANA 时区和同一周一至周日周期内的
预览/开拍/结束，预览至少提前 120 分钟、开拍早于结束；不自动跨周，使用所填时区。计划写入 common.toml，runtime 排期接管
默认关闭，新设置不改变已有拍卖。

三个设置接口逐请求验证部署码，共享非阻塞门禁；每请求总预算 10 秒，忙碌时不排队连接数据库。保存超时、连接丢失或终态
不明时，页面清空令牌/密码并锁定；未收到成功不代表未写入，须停止初始化进程、核对配置目录及数据库，禁止刷新重试、删除或
覆盖目录，重输部署码不能解除服务端未知状态。

生成器在私有临时目录设权限，依次同步文件、runtime 子目录、根目录，禁止覆盖地发布后同步父目录；发布后结果未知须保留
现场，详见[生成器边界](crates/auction-config/README.md#解析与装配入口)。合并服务的确认与空库初始化见
[server 手册](apps/auction-server/README.md#首次启动)。

## 启动顺序

1. 安装三角色/common/敏感/runtime 配置，核对路径与 owner；启用 Worker control，预建目录/组，allowlist 纳入实际 Admin/
   运维 UID 或 GID。HTTP Unix listener 同样预建目录/socket 组。
2. 审阅三角色 unit 与 auction-stack.service，准确绑定 AUCTION_CONFIG_FILE；只 enable 总控。
3. 严格预检后由总控迁移、启动：三角色须 `inactive`/`failed` 且 `MainPID=0`，转换中或停机钩子未完即拒绝；随后
   Worker → Admin → Public 逐个 readiness。全部已运行仅复核顺序/就绪并更新 adopted 回执，部分运行拒绝。
4. 代理仅连实际 Public/Admin listener，禁止公开 metrics；Unix upstream 同步目录权限。

```bash
/opt/auction/bin/auctionctl deployment-check --config-dir /etc/auction --strict-permissions
sudo install -d -m 0700 /var/lib/auction/deployment
sudo /opt/auction/bin/auctionctl deployment start --config-dir /etc/auction
sudo /opt/auction/bin/auctionctl deployment status
sudo /opt/auction/bin/auctionctl deployment restart --config-dir /etc/auction
sudo /opt/auction/bin/auctionctl deployment stop
```

预检不连接数据库；配置目录/common/角色/runtime/敏感文件均禁 symlink，strict-permissions 将权限警告升为失败。总控状态目录
必须预建、规范绝对路径、归其有效用户所有且至多 0700。restart 先预检并反序停机，再迁移和启动；失败只回滚本次启动的
unit，迁移不自动回退。独立建库/迁移命令见[auctionctl](apps/auctionctl/README.md#停机迁移与初始建库)。
迁移前还按实际 Admin unit 的 `User` 验证 runtime 文件及可写父目录的属主，runtime 必须精确 `0640`；
空 `User` 按 systemd 的 root 语义处理，`DynamicUser=yes` 不能满足持久可编辑 runtime 的固定属主要求。运维命令以
root 执行不等于 runtime 应归 root，不能拿运维进程 UID 代替服务身份。旧 `session_signing_key` 无有效会话协议，
新版显式拒绝该无效选项，须从敏感文件移除后再做部署预检。
旧部署还须检查 runtime 同目录的 `.admin-runtime-config.lock` 属主和 `0600` 权限；保留 inode 的停机收紧流程见
[Runtime 共享与重启](ops/config/README.md#runtime-共享与重启)，不能只核对 TOML 的 `0640`。

Admin 每次启动永久撤销旧管理会话，失败即阻断启动；须重新登录，Public 不受影响。轮换 actor/secret 前停掉所有旧凭据实例并
统一新值，恢复旧配置不复活会话。SIGTERM：drain 最多 10 秒、exporter 最多 30 秒，分体 unit 45 秒；合并及外层预算见
[关闭预算](#关闭预算)。单角色重启可能联动依赖角色，操作前核对 Requires/After。

实际取得的邮件或论坛凭据运行时 owner 永久失效时，Worker 在一秒观察周期内停止新领取并沿用上述排空预算，随后非零退出；
分体部署依赖 `Restart=on-failure` 重建进程。新进程重新取得对应 advisory lock，邮件 owner 另推进 generation 后才能恢复外呼，
不在旧进程内重新抢锁。首次撤销记录凭据类型及闭集原因，不记录凭据或连接错误原文；暂停/恢复与手动触发不会恢复
已撤销 owner。单独更新所有权监督逻辑不新增迁移；同时包含邮件待读与退件去重修复的版本按下节完成迁移。更新前保留原制品与哈希，按实际
依赖关系反序停止、正序启动。未知发送和资金对账仍遵守原有门禁。

### 邮件详情恢复与退件消费升级

`000051` 为 recovery 追加已消费退件事实，`000052` 为 mail-poll 增加详情 GET 前的持久待读队列。
升级须确认目标数据库、保存可验证备份、停止全部旧 writer，使用同版 `auctionctl` 按既有停机门禁迁移至
`000052`，再启动同版角色；合并部署停止 `auction-server` 后按原 `migrate-managed` 流程执行。
普通进程重启不执行迁移，新旧 Worker 不混跑。

退件迁移只回填已有来源能够证明的消费记录，保留既有退回计数及历史投递结果；新回收命令按具体退件生成稳定身份，
已存在的准备/投递继续使用原身份。邮件待读队列从升级后首次详情读取前建立，不猜测升级前已经已读但未登记的邮件，
历史漏收须按原邮件 ID 与业务记录人工核对。迁移及源码完成情况见[迁移目录](migrations/postgres/README.md)与
[状态页](docs/status.md)。

### 装备标记与超价提醒升级

装备标记基础为 `000041`，账号 MM/PM 竞价警告必须再追加 `000042`，同时更新 Public 页面、三角色程序和内嵌迁移的
`auctionctl`；合并部署更新 `auction-server` 与 `auctionctl`。按受控停机门禁升级，不能把普通进程重启或替换 Worker 当作迁移
完成，也不能混用缺少标记通知事实的新旧竞价 writer。论坛结拍尾注还需追加 `000043`：使用内嵌最新迁移的 auctionctl 在全部
旧 writer 停止后迁移，普通重启不隐式升级；升级前 settled 主题明确豁免、完成版本保持空，未结算及新场次才进入终态刷新。
终态只编辑原有冻结目录页及日志，必须全页证明和版本 CAS 成功后才停止轮询；pending、超限、租约丢失或 write_unknown 仍阻断，
不得手工填成功版本绕过，不重放终态新竞价，缺权威结算或旧未知写入保留现场另行处理。该能力不新增配置，大场次全固定页
确认仍消耗既有 item/lease/限速预算。

迁移只给已有有效代理补建默认标记，不补发过去的超价提醒，也不重放历史竞价；标记保存不新增 TOML 开关。实际 MM 发送仍要求
既有邮件服务启用、有效凭据、mailbox identity 和发送预算。PM 使用独立持久队列，不借 MM 开关、delivery 或 Sent proof，必须
启用 forum 并复用同一网关和唯一 ForumCookie owner；无新增配置字段，每轮最多 1 新发送＋1 只读核对、串行执行，复用
`worker.delivery.poll_interval_seconds/jitter_seconds`，总工作预算和租约用 `worker.forum.item_timeout_seconds/
lease_duration_seconds`，共享 forum HTTP 限速。轮询代码默认 60 秒不是实际部署配置，也不是到信时间或论坛官方限流承诺；
forum.enabled=false 不外呼。通知与普通论坛 `notify mm` 来源分离，未知 sending 永不重排，只允许原 sender 只读核对，
禁止人工重寄；取消或重新领先抑制未开始发送的提醒，已进入发送的不可撤回，账号渠道切换不改已冻结队列。

`000042` 迁移前停止全部旧 writer/Worker，核对目标并备份，禁止新旧竞价 writer 混跑或以普通启动代替迁移；不改写旧 MM
payload/digest/命令身份，新账号默认 MM/1；缺论坛收件人留下 suppressed/recipient_missing，后续补绑定不补发。回滚应用不能
退回不识别 PM 的旧 Worker，数据库迁移 forward-only，失败保留现场。最小运维（需另行获准真实环境操作）：

```text
auctionctl lot-watch-pm list --status manual_reconcile --limit 50 --offset 0
auctionctl lot-watch-pm reconcile --notification-id <uuid> --command-id <uuid> --expected-fencing-version <positive-i64> --actor <operator>
```

list 只输出安全状态/fence/时间/sender，不输出正文或租约 token；reconcile 仅持久接受只读核对请求，原 command_id/负载重试
返回原结果，不代表已发送，由持有原 sender 凭据 owner 的 Worker 核对。换号、无副本、解析不确定保持 manual_reconcile；
CLI 不建第二个客户端，不提供强制成功、abandoned 或重发动作。获准行为验证后需单独核对旧代理补标记、领先周期去重、
并发取消/重新领先、Worker 重启、未知发送以及跨账号/设备同步；静态编译和页面可见不能替代这些证据。

### 用户快照时间修复升级

`000044` 修复一类 Admin 用户列表整体 `503`：MM 注册曾以偏快的远端接收时间建立账号，后续论坛同步使用服务器时间，形成
`updated_at < created_at`。不要删用户、重签 Key、放宽投影校验或批量猜测时区来绕过错误。

升级前确认精确数据库目标、当前迁移版本及异常数量，完成可验证备份，停止所有角色 writer，并更新含本地处理时钟修复的
Worker、用户时间下界修复的三角色和内嵌迁移的 `auctionctl`；普通重启不迁移。若目标仍为 `000039`，正常顺序还会应用
`000040`—`000043`，必须同时遵守上述论坛、标记及 PM 升级边界，不能跳号登记。

迁移只处理 MM `registered` 账本时间匹配用户创建时间、论坛身份历史时间匹配旧更新时间的倒退记录：保留创建时间，将快照
更新时间提升到创建时间与迁移时刻的较大值、推进用户版本，并逐条追加 `user.timestamp_order_repaired` 审计。不更改用户身份、
资格、Key、历史账本、原始邮件或旧审计；保存的未来创建时间不等于已核实其真实时区。其他来源的时间倒退使约束校验失败并
回滚，保留现场另行核对，不手工忽略约束或改 checksum。

执行器须包含 `000044` 延迟约束兼容：在原事务的 ALTER 前执行已排队的用户论坛身份检查，避免 PostgreSQL 因 pending trigger
events 拒绝修改表结构。使用配套修复版程序，不改已发布 SQL/checksum，不手工登记迁移或更正用户数据；若此前停在 `000043`，
先保留失败日志和备份、确认三角色仍停止，再由修复版总控从未完成的 `000044` 顺序继续，已提交的 `000040`—`000043` 不重放。
迁移后核对账本、倒退记录为零、对应追加审计及服务 readiness；经授权只读加载用户列表确认恢复，旧页面持有的用户版本须
重新读取。本轮只完成源码与只读调查，尚未执行该迁移。

## Public 登录时长升级

启用账号登录时长前，核对目标并保存可验证备份，按既有分体停机门禁或合并 `migrate-managed` 流程停止旧 writer，
使用同版 `auctionctl` 追加至 `000058`。Public API 与内嵌页面须同版更新；旧页面不能解码新的成对 `null` 会话截止。
从 Public 角色 `[session]` 删除 `idle_ttl_seconds` 和 `absolute_ttl_seconds`，仅保留可选 `max_public_sessions`；
旧字段会明确阻断加载，不可通过关闭校验绕过。Admin 角色/runtime 会话配置不改。

新登录默认永久，用户可在「我的」保存固定时长；旧会话保留原截止、不再滑动刷新，其他设备的现有会话不会随账号偏好自动改期。
尚有效的会话可以主动保存新设置，已过期或撤销的会话必须重新登录；Cookie 清理/浏览器期限仍可能要求重登。
永久会话继续占用原并发额度，丢失 Cookie 不等于服务端撤销；不要为解除上限恢复已撤销 Key 或修改审计。
保留策略元数据须与 schema 58 配套，仍不授权删除数据；精确字段与安全边界见[登录时长契约](docs/contracts/routes.md#p27p28-登录时长)。

## 数据库恢复与保留

| 任务 | 权威流程 | 边界 |
| --- | --- | --- |
| 当前 schema 备份、离线 verify、空库恢复 | [RECOVERY.md](ops/deploy/RECOVERY.md) | 备份身份/哈希、零连接、同名空库、单事务恢复、后验与 receipt |
| 显式破坏性重建 | 同一恢复手册 | 默认只读；独立 execute、目标确认、恢复点和回执，不由启动触发 |
| canonical v28 接管 | [LEGACY_V28_ADOPTION.md](ops/deploy/LEGACY_V28_ADOPTION.md) | 专用 manifest、精确格式、bridge 与前向迁移；另导入外部五语言规则 |
| 保留盘点 | 下例与[CLI 说明](apps/auctionctl/README.md#保留盘点与恢复预检) | inventory only，不判断删除资格、不归档/删除 |

```bash
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  /opt/auction/bin/auctionctl retention-inventory \
    --confirm-database auction_production \
    --output /secure/auction-evidence/retention-inventory.json
```

证据为新绝对路径，父目录预先存在且不可被 group/other 写入。输出绑定[retention policy](ops/deploy/retention-policy.v1.json)
SHA-256，含 schema/迁移/关系大小、估算行数、表覆盖及数据族摘要；不锁业务行或修改数据库。`eligibility_assessed` 和
`eligibility_authorized` 固定 false；保留期限和 mutation 仍需独立设计/授权。

## 反向代理与 systemd

[Caddy 模板](ops/deploy/caddy/Caddyfile)仅压缩 GET/HEAD 的 `/assets/*` 和 `/favicon.ico`，由浏览器 Accept-Encoding 协商
Zstandard/gzip；页面入口、API 和 WebSocket 不在范围。资源所有权、缓存和缺资源响应见[HTTP 契约](docs/contracts/routes.md)，
metrics 默认对公网返回 404。[systemd 模板](#systemd-服务与顺序控制)安装前须替换账号/路径并核对权限；迁移仅由总控
start/restart 或受停机门禁保护的显式 migrate 执行。任何 unit 均不自动备份、恢复、DROP/CREATE 或执行 retention 删除。

## Cloudflare 整站代理

推荐拓扑为浏览器 HTTPS/WSS → Cloudflare 橙云 → Caddy HTTPS → Public/Admin 独立 Unix socket。Rust 原生 HTTP 边界负责精确
Origin、Cookie、客户端 IP 与 CDN 缓存响应头，不需要 Cloudflare SDK、API token、新 TOML 开关或前端资源域名；API 和
WebSocket 仍走所属站点同源路径。[专用 Caddy 模板](ops/deploy/caddy/cloudflare.Caddyfile.example)与原直连模板二选一。

### TLS 与可信链

1. Public/Admin 使用不同 DNS 主机并开启橙云；Cloudflare SSL/TLS 必须为 **Full (strict)**，禁止 Flexible。Caddy 负责源站
   HTTPS 和有效证书，证书名称匹配两个主机；可用其自动 HTTPS，或经审阅配置 Cloudflare Origin CA 证书/私钥；浏览器直连须用
   受浏览器信任的公网证书，不能只依赖 Origin CA。私钥不进仓库。确认 ACME HTTP challenge 经橙云可达，不依赖被 Cloudflare
   终止的 TLS-ALPN challenge。
2. Cloudflare 开启 HTTP→HTTPS 重定向；源站仅开放所需 TLS/证书验证入口，Rust 不监听公网 TCP。Caddy 不再按 Cloudflare 来源
   IP 拦截，允许直接访问源站；已有云端防火墙或 mTLS 限制不会被本地配置自动解除，变更须单独授权。直连流量绕过 WAF、限流
   与 CDN，源站仍须保留应用认证、限流、精确 Origin 和 TLS 保护。
3. 将[官方 IP 列表](https://www.cloudflare.com/ips/)的当前完整 CIDR 集合注入 Caddy `AUCTION_CLOUDFLARE_CIDRS` 并随官方变化
   审阅更新；该列表仅限定地址头信任，不限制访问，禁止信任所有网段或仅凭头名识别 Cloudflare。非受信 peer 的直连使用真实
   连接 IP、忽略其 CF/XFF 声明；受信 peer 才从 `CF-Connecting-IP` 恢复地址，再以 `{client_ip}` **覆盖**发往 Rust 的
   `X-Forwarded-For`，删除其他客户端地址头。不要改成透传访客 XFF 或让任意 peer 提供 CF 头。此拓扑不含修改 IP 头的
   Workers/多 CDN 链；Pseudo IPv4 选 Off 或 Add Header，不用 Overwrite Headers，不启用移除访客 IP 的 Managed Transform。
4. Rust Public 的 `[http_security]`、Admin runtime 的 `[security]` 分别填写各自精确 HTTPS origin，`allowed_origins_enabled=true`、
   `cookie_secure=true`。Unix listener 设置 `trusted_unix_proxy=true`、`trusted_proxy_cidrs=[]`，所有有权连接 socket 的进程都被
   视为可信代理；socket 0660，目录允许代理组遍历但不可写。示例采用 `ngnl:ngnl`，Caddy 保持原用户并附加 `ngnl` 组；
   须控制组成员及其他组可读文件，敏感 TOML 保持 0600。Unix peer 不伪装成 loopback TCP，TCP peer 不会因 Unix 信任开关而被
   信任；若选择 TCP 上游，必须关闭 Unix 信任并只列实际本机代理 `/32`、`/128`，不能把客户端或 Cloudflare 网段无差别塞入
   Rust 配置。`Forwarded`、`X-Forwarded-Proto`、`CF-Connecting-IP` 不用于 Rust 推导 Origin 或 Secure Cookie。

Public/Admin Unix 配置片段及 runtime 覆盖规则见[配置手册](ops/config/README.md#cloudflare-与-unix-上游)。Worker control 本来就是
独立 Unix socket，HTTP 与 control socket 不复用；Unix 化指本机应用代理/control 通道，不要求 PostgreSQL、Redis、OTLP 或外部
HTTPS 改协议。Caddy 不能获得应用敏感 TOML 的读取权限。

### 只缓存静态资源

Rust 默认发送 `CDN-Cache-Control: no-store` 与 `Cloudflare-CDN-Cache-Control: no-store`，只有精确静态 handler 签发的响应才允许
独立 CDN 策略。Public 公开 API 的浏览器 `Cache-Control: no-cache` 保持兼容但**不允许 CDN 保存**；登录、私有数据、全部
Admin API、错误、探针、WebSocket、HTML/SPA 入口均不缓存。Admin 的内容指纹 CSS/WASM 等编译资源可按静态白名单缓存，
不允许缓存管理页面或投影。`/assets/` 非 HTML 资源沿用一年 immutable；稳定名 JS/WASM 与 `/favicon.ico` 每次再验证，
HTML 一律 no-store。

Cloudflare Cache Rules 按以下互斥范围设置，替换两个主机为真实域名：

```text
(http.host in {"auction.example.com" "admin.auction.example.com"}
 and http.request.method in {"GET" "HEAD"}
 and starts_with(http.request.uri.path, "/assets/"))
```

- 仅以上范围设置 Eligible for cache；Edge TTL 选择使用源站 Cache-Control、缺失时 bypass，不指定覆盖 TTL，Browser TTL
  尊重源站。源站 404/405/503、HTML 即使路径形似资源也必须保留 no-store。
- 两个站点内其余所有请求显式 Bypass cache，包括 `/api/*`、登录、账户、管理页面、`/index.html`、`/setup`、`/health*`、
  `/metrics`、稳定文件名及 WebSocket；不要只按文件后缀做白名单，避免 API 路径伪装静态。
- 删除冲突的 Cache Everything、Page Rules、Edge TTL 覆盖、Cache Response Rules、Workers Cache API 或自定义缓存键策略；
  不得忽略源站 no-store、移除 Set-Cookie、忽略身份 Cookie 后缓存业务响应。更高优先级规则可覆盖源站头，代码无法防御错误
  控制面配置；切换前清理历史缓存。
- 不对本站启用 Rocket Loader、HTML/JS 重写或影响 Dioxus 启动的转换，保持资源内容与同源 CSP。

### 论坛倒计时上线

首帖图片由 Public 的 [P22 动态 SVG](docs/contracts/routes.md#p22-自托管论坛倒计时) 提供，无第三方域名、JS 或新增配置字段。
先更新 Public API，再更新 Worker；合并部署更新 `auction-server`。沿用 Worker 的 `public_base_url` 作为公开 origin，正式论坛须
使用浏览器可达的 HTTPS 地址，不能使用 loopback、Unix socket 或仅局域网可达地址；未配置 origin 时仅输出文字。升级不需要
新增迁移，也不会绕过论坛未知写入门禁强行重发首帖；旧帖在正常、成功的固定帖刷新后才改用本站图片。
本次数字显示修复只需更新 Public API 并重新加载图片，已有 `list.svg` / `auction.svg` URL 不变；
主题状态后缀由更新后的 Worker 在下一次正常成功同步时写入。两项均不要求重建主题或手工解除未知写入门禁，
历史已豁免的 settled 主题不自动回填，取消仅刷新已发布的有效主题。

图片路径仍在 `/api/v1/` 下，虽然以 `.svg` 结尾也必须 Bypass cache，浏览器和两种 CDN 头均 `no-store`；不得给这些图片注入
登录、交互挑战、SVG 转换或跨站图片阻断，也不需要开放 CORS 凭据。未知计时器、隐藏场次、依赖故障分别按接口契约失败，
不返回伪造日期或默认计时。图片动画受网络、浏览器暂停和第三方论坛图片代理影响，打开后不自动获知改期；UTC 文字和官网
权威状态必须保留。先在获准的本地环境验证，再单独申请真实论坛发帖/回读。

### WebSocket 与验收边界

Cloudflare Network 开启 WebSockets；`wss://<Public主机>/api/v1/ws` 继续传递 Cookie 和精确 HTTPS Origin。Caddy
`reverse_proxy` 原生保留 Upgrade，无需改业务路径、认证、CSRF、HTTP/2 CONNECT 或普通请求超时。不得对 API/握手应用缓存、
交互式挑战或内容转换；WAF/限流例外须限定具体路径和风险。Cloudflare 发布可能断开长连接，客户端继续按 sequence/version 经
HTTP Catch-up 和权威快照恢复，不能用增加 CDN 缓存掩盖断流；代理空闲时限须适配现有心跳。本节是配置与源码契约，格式与静态
编译不证明证书续期、边缘缓存命中、真实客户端 IP、WAF 或长连接行为。

### 官方依据

- [Cloudflare CDN-Cache-Control 与优先级](https://developers.cloudflare.com/cache/concepts/cdn-cache-control/)：
  Cloudflare 专用头优先于通用 CDN/浏览器头，Cloudflare 专用头不转发给浏览器；Edge TTL 等规则仍可覆盖源站。
- [Cache Rules 设置](https://developers.cloudflare.com/cache/how-to/cache-rules/settings/)：缓存资格、bypass 和尊重源站 TTL。
- [Full (strict)](https://developers.cloudflare.com/ssl/origin-configuration/ssl-modes/full-strict/)：源站证书有效性和主机匹配。
- [Cloudflare 请求头](https://developers.cloudflare.com/fundamentals/reference/http-request-headers/)：CF-Connecting-IP、
  XFF 追加、Workers 与 Pseudo IPv4 限制。
- [Cloudflare WebSockets](https://developers.cloudflare.com/network/websockets/)：Upgrade、启用方式与边缘重启断连。
- [Caddy 全局选项](https://caddyserver.com/docs/caddyfile/options)：trusted_proxies、trusted_proxies_strict、client_ip_headers。

## systemd 服务与顺序控制

分体由 systemd 管理单角色 PID、信号、重启及日志，由 auctionctl 管理跨角色顺序、readiness、锁和回执。合并部署只启用
[auction-server.service](ops/deploy/systemd/auction-server.service.example)，不得同时启用分体 unit/总控。Public Requires
Admin/Worker，Admin Requires Worker；单角色停止或重启可能联动依赖者，不能按命令点名范围推断影响。进程存活不能代替
readiness，数据库不可用或 schema 未就绪时 ready 保持 503。

### 安装前提

所有 `.example` 都是待审阅模板，不自动安装。先替换 User/Group、程序和配置路径，准备运行目录与用户组，核对敏感 TOML
可读性、Worker control allowlist、Public/Admin listener 和代理上游。首次配置和数据库前置条件见[部署手册](#首次部署配置)。
分体 Unix 部署还须按实际服务 UID、HTTP listener group 和代理遍历权限审阅
[tmpfiles 模板](ops/deploy/tmpfiles/hv-auction.conf.example)。共享 `/run/hv-auction` 由 tmpfiles 管理，
Worker 仅管理 `worker` 子目录；不可把共享根重新加入 Worker 的 `RuntimeDirectory`。
升级旧模板必须先停稳旧三角色，等待旧 Worker 的目录清理完成，再安装新模板并预建目录；
禁止在线删除根目录、HTTP socket 或实例锁，也不要递归 `chown` 既有目录树。

### 安装示例

先建立所需服务用户和代理组，审阅模板后预建共享目录，再安装四份分体 unit：

```bash
sudo install -o root -g root -m 0644 ops/deploy/tmpfiles/hv-auction.conf.example \
  /etc/tmpfiles.d/hv-auction.conf
sudo systemd-tmpfiles --create /etc/tmpfiles.d/hv-auction.conf
sudo cp ops/deploy/systemd/auction-public-api.service.example \
  /etc/systemd/system/auction-public-api.service
sudo cp ops/deploy/systemd/auction-admin-api.service.example \
  /etc/systemd/system/auction-admin-api.service
sudo cp ops/deploy/systemd/auction-worker.service.example \
  /etc/systemd/system/auction-worker.service
sudo cp ops/deploy/systemd/auction-stack.service.example \
  /etc/systemd/system/auction-stack.service
sudo systemctl daemon-reload
sudo systemctl enable --now auction-stack.service
```

### 配置绑定与探针

AUCTION_CONFIG_FILE 仅接受 Environment 中无空白/转义的简单赋值，可整体加双引号；拒绝非空 EnvironmentFiles、
UnsetEnvironment、PassEnvironment、PAMName，不回显环境值。systemctl show 仅可省略空 EnvironmentFiles，其他身份/环境字段必须
返回。Worker control 须启用并允许实际 Admin/运维 UID 或 GID，探针走 Unix `/v1/health/ready`；Public/Admin 走有效 TCP/Unix
`/health/ready`，Admin 不回退 runtime 覆盖前地址。每次异步连接、写入、状态行读取共用最多 1 秒且受剩余启动预算约束；
每角色默认 30 秒。

`auctionctl` 的 systemctl 状态查询最多 5 秒，readiness 前后查询同时受该角色剩余预算限制；单次启停命令最多
60 秒。超时、管道失败或 stdout 超过 64 KiB 时终止直接子进程，并额外用最多 5 秒确认回收；stderr 只保留 2 KiB
诊断前缀。错误不自动重试，systemd 已接受的 job 可能继续执行，必须重新核对状态；停止或状态查询失败仍阻断迁移。

### 关闭预算

| 边界 | 当前预算 | 安装时核对 |
| --- | --- | --- |
| 单角色 | drain 10 秒 + exporter 最多 30 秒；unit 45 秒 | 二次信号强制结束 drain，exporter 仍受自身超时保护 |
| 分体总控 | TimeoutStartSec=180、TimeoutStopSec=155 秒 | 停止覆盖三角色各 45 秒及 20 秒总控余量；启动仍须覆盖迁移和三个 readiness |
| 合并 supervisor | drain 135 秒，强杀后每角色回收最多 5 秒；unit 155 秒 | KillMode=mixed，超时回收整个 cgroup；不得套用分体 45 秒 |

systemctl 的单次上限与外层 unit 预算独立；systemd/D-Bus 异常时，总控 155 秒上限可能先终止整段流程，不能将三个
命令上限相加当作延长总控预算。这里只核对模板和源码预算；既有安装须在维护窗口同步 unit 并重新加载。

### 权限与回执

StateDirectory=auction/deployment 创建私有状态目录；总控持非阻塞锁、原子发布 startup-order.v1.json。手工运行须预建规范
绝对路径，归有效用户所有、权限至多 0700。回执仅含 unit、InvocationID、PID、单调启动时间及观察时间，不含配置/凭据。
每次 readiness 成功前后复核 unit 的运行状态、PID、InvocationID 和单调启动时间，发布 started/adopted 回执前再次复核三角色身份。
started 失败回滚本轮启动的 unit；adopted 失败不停止既有 unit。身份一致不证明 socket 归该 PID，发布后仍可能退出。
全部已运行时 start 复核 readiness/顺序并发布 adopted 回执；部分运行或转换中拒绝。须预建 auction-worker-control 组并纳入
Worker/Admin supplementary group；TOML group、allowlist 须匹配实际 UID/GID，禁止照抄数字 ID。Admin/Worker 须匹配 control
路径、用户登录及外部凭据两组密钥和有效拍卖/结算/排期；装备数据密钥的标识与材料必须三角色一致，
全部未配置仍保留既有能力降级。三角色须共用 runtime 或全部省略；Admin 的 Worker 预算引用还须绑定实际角色文件。保存不热加载，重启范围见
[配置说明](ops/config/README.md#runtime-共享与重启)。

### 可写目录

Admin 模板开放 `/etc/auction/runtime` 和 `/run/hv-auction`，Public 开放后者；共享根由独立 tmpfiles 规则预建。
Worker 仅用 `RuntimeDirectory=hv-auction/worker` 管理自身子目录，开放 `/run/hv-auction/worker`、`/var/lib/auction`；
其自动重启不能清理 Public/Admin 的 socket 与锁。按实际 socket 和诊断目录收窄或补齐 ReadWritePaths，
公共/敏感 TOML 保持只读。目录 owner/group、实例锁、inode 和 peer allowlist 仍由应用检查，
systemd 可写声明不能替代这些门禁；显式重启 Worker 仍可能按既有 Requires 关系停止 API，不承诺无中断重启。

### 独立迁移门禁

`auctionctl migrate` 沿用总控的 systemctl、state-dir 和三个 unit 参数，持锁要求全部角色 inactive/failed 且 MainPID=0；
运行、转换中、状态读取失败或锁冲突均阻断迁移。总控 start/restart 在同一锁/预检边界迁移，单角色 unit 不迁移。参数及建库
边界见[CLI](apps/auctionctl/README.md#停机迁移与初始建库)；任何 unit 均不执行 DROP/CREATE、备份恢复或 retention 删除。
