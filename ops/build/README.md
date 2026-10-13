# AMD KVM 定向编译候选

[文档导航](../../docs/README.md) · [开发指南](../../docs/development.md) · [部署手册](../deploy/README.md)

这些文件由 `cargo --config` 显式选择，仅用于用户指定的两台 Debian 13 / glibc 2.41 KVM 机器。
默认工作区、正式发布脚本和 WASM profile 不变。它们是待部署机测量的性能候选，不是“最优性能”
或通用 Linux x86_64 兼容性的保证。

## 两份目标配置

| 配置 | 用户提供的目标 | 原生指令集策略 |
| --- | --- | --- |
| [ryzen-7950x.toml](ryzen-7950x.toml) | Ryzen 9 7950X；2 vCPU、约 2 GiB RAM | x86-64-v4 加快照中已有的扩展，包含 RDSEED |
| [epyc-9645.toml](epyc-9645.toml) | EPYC 9645；4 vCPU、约 8 GiB RAM | x86-64-v4 加已有扩展，增加 AVX-VNNI/VP2INTERSECT，不启用 RDSEED |

CPU 型号不能替代虚拟机 CPUID。特别是第二台未列出 `rdseed`，所以不直接使用完整
`znver5`；两份配置均不用 `native`、不稳定的 `-Z tune-cpu` 或型号隐含的 LLVM-only 特性。
它们以 v4 为基础建立显式扩展白名单，不宣称获得了 Zen 专用调度优化。
宿主机迁移、VM CPU 模式改变或换机器后必须重新核对指令集；不匹配可能发生非法指令退出。

两份配置使用 `opt-level=3`、Fat LTO、一个 codegen unit、非增量发布构建。
`panic=unwind` 保留后端任务失败语义；不以整个进程退出换体积。构建并发 2 只限制本机编译内存，
不改变部署的 Tokio/Worker 并发、连接池或运行配置。第一台仅约 2 GiB RAM，不建议在其上执行完整 LTO
构建；编译优化也不能替代部署后的内存与数据库资源规划。

## 在构建机生成候选

从仓库根目录执行。Web 先按既有脚本独立构建，不能把后端 CPU 配置传给浏览器目标。
不要设置额外的 `CARGO_PROFILE_RELEASE_*`、`CFLAGS`、`CXXFLAGS` 或全局 Cargo CPU 优化覆盖；
尤其不能夹带 `-march=native`。配置文件本身不会写入用户全局 Cargo 配置。

```bash
just web-build

for machine in ryzen-7950x epyc-9645; do
  build_dir="target/amd-build/$machine"

  # Web 更新后强制重新展开 RustEmbed；保留其余依赖缓存。
  if [ -f "$build_dir/CACHEDIR.TAG" ]; then
    cargo clean --target x86_64-unknown-linux-gnu \
      --target-dir "$build_dir" \
      -p auction-public-api -p auction-admin-api
  fi

  env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS \
      -u CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS \
    cargo --config "ops/build/$machine.toml" build \
      --locked --release --no-default-features \
      --target-dir "$build_dir" \
      -p auction-public-api -p auction-admin-api -p auction-worker -p auctionctl \
      --features postgres,cache-redis,auction-public-api/web-embed,auction-admin-api/web-embed
done
```

输出分别位于 `target/amd-build/<machine>/x86_64-unknown-linux-gnu/release/`。
保留两台机器的制品目录与校验和，不能用型号标签替代实际特性核对，也不能混入通用发行包。
这些命令不安装、启动或替换服务，不迁移数据库；正式切换仍遵循部署手册的停机与配置门禁。
现有 `ci/release.sh` 不负责这两份本地定向候选，不能直接把它们当成通过正式归档门禁的发布包。

构建机与两台目标当前均为 glibc 2.41；相同 ABI 只是已知前提，真实加载的 GLIBC 符号版本、
动态库及目标指令集仍需核对。构建机只有 AVX2，不能执行这两份 AVX-512 候选。

## 已有测量和待补证据

2026-10-12 在本机 Intel KVM 上对真实 `auction-domain` 规划器作授权微基准：
9 类规划与计划 JSON 解码/校验，共 10 组；每组 32 份固定输入、每轮 200,000 次，
6 个编译候选交错执行 9 轮，绑核 CPU 2，规划预热在计时外，使用 `black_box` 保留计算。
所有规划输入预检成功；未连接数据库或执行 HTTP/外部请求。

| 本机候选 | 10 组等权几何平均吞吐，相对默认基线 |
| --- | --- |
| x86-64，默认 Release / 16 CGU | 1.000× |
| x86-64，Thin LTO / 16 CGU | 1.057× |
| x86-64，Fat LTO / 1 CGU | 1.106× |
| v3，Thin LTO / 16 CGU | 1.097× |
| v3，Thin LTO / 1 CGU | 1.132× |
| v3，Fat LTO / 1 CGU | 1.158× |

统计先取各组 9 轮批量平均耗时的中位数，再对速度比取等权几何平均；这不是业务流量配比、
请求 p99 或 PostgreSQL 吞吐量。宿主机调度和代码布局也可能影响结果，不能外推成 AMD
服务器收益或“全系统提升 15.8%”。这里只据此保留 Fat/1 作为后续候选，没有穷举参数空间。

临时源码、锁图、原始样本、构建日志和专用测量程序保存在
`target/compiler-performance/`；该目录是被忽略的本地取证产物，不加入默认 CI。
其中 `bin/ryzen-7950x`、`bin/epyc-9645` 仅是纯领域测量程序，**不是服务程序**；
两者已编译，未在本机执行。Cargo 指纹确认采用不同的扩展参数，但该微基准的两份机器码哈希相同，
说明仅增加目标特性不保证这类负载能生成不同代码或获得额外收益。
对应目标机可与 `bin/baseline`、`bin/v3-fat1` 交错重复测量，
不能仅比较一次运行，也不能把该微基准当成完整业务验收。

两份配置已经通过 PostgreSQL + Redis + 对应 Web feature 的四程序 Release 模式
`cargo check --locked`。同日用户继续要求重编 Ryzen 7950X，已完成该档四程序的 Release 链接，
并独立重建 Public/Admin 页面；EPYC 仍只有静态编译检查与微基准程序，没有专用服务制品。
交付后用户单独授权远端运维，Ryzen 目标已完成空库迁移、三角色启动和 readiness 核对，见下节记录；
EPYC 目标未执行运行验证，不能把 Ryzen 的启动证据当作两种 CPU 的性能测量。
PostgreSQL 端到端、并发/p99、PGO 训练尚未执行；继续测量只使用隔离数据库，不触碰业务库。
PGO 需要代表性负载和与 rustc LLVM 匹配的工具，不能把纯领域微基准训练结果直接用于整套服务。
默认测试禁令不变，本轮性能测量仅来自用户的明确授权。

### 2026-10-12 Ryzen 7950X 重编记录

独立构建目录为 `target/amd-build/ryzen-7950x/`；可复制的四程序、构建日志和证据位于
`target/amd-artifacts/auction-ryzen-7950x-20261012T044747Z-dirty/`，同级 `.tar.gz`
为 28,406,531 字节，附 `.tar.gz.sha256`。包内 `SHA256SUMS` 绑定四个程序及交付证据，
不含真实配置、凭据或自动安装脚本。该目录被忽略，是本次工作树的本地候选，不是正式归档门禁产物。

源码身份在 Web/native 构建前冻结、阶段边界复核；资源清单与构建后的内嵌标记匹配，
Public/Admin 对端资源均缺失，Worker/auctionctl 不含页面。Cargo 指纹与实际编译命令确认
四个程序均启用 PostgreSQL、Redis 及指定 CPU 参数、Fat LTO、单 CGU；未启用 SQLite。
ELF 静态检查显示三个角色程序最高引用 GLIBC 2.34，auctionctl 最高引用 2.39，未超过目标的 2.41。
依赖为 `libgcc_s.so.1`、`libm.so.6`、`libc.so.6`，不把 ABI 静态核对当作目标机启动验证。

详细日志保留第三方依赖的 cfg、弃用和生命周期等警告；本轮没有修改依赖或关闭项目 lint。
重编交付阶段没有运行 AVX-512 制品、启动/替换服务、连接数据库或执行行为测试；未覆盖既有部署目录。
本节及状态说明更新发生在产物封存后，不把事后文档改动伪装成构建前的源码身份。

### 2026-10-12 Ryzen 目标初始化与启动

用户另行明确授权 SSH 排查、修正配置，随后授权使用管理员权限初始化并启动指定目标。
首次失败来自沿用构建机目录的 systemd/TOML 路径，以及已能连接但完全没有业务表和迁移账本的目标库，
不是已确认的 CPU 非法指令问题。配置修改先备份，只修正 Worker/Admin control 路径和权限，未改凭据。

在三角色均 `inactive`、`MainPID=0` 后，再次确认 PostgreSQL 18.6 目标为空、主库可写且具备建表权限，
保存并校验普通 `pg_dump` 空库快照。以服务账号执行带实际 unit 名与私有状态目录的 `auctionctl migrate`，
39 条迁移全部成功、最大版本 39、失败数 0，建立 64 张 public 表；不执行 DROP/重建，也不改历史迁移文件。

随后按 Worker → Admin → Public 启动并逐一通过 readiness；三个进程保持同一 InvocationID、自动重启为 0。
采集的本次 invocation 最近 160 条日志窗口未见 WARN/ERROR。`auctionctl deployment start` 在全部已运行分支
复核就绪与启动顺序、发布 adopted 回执；后续 status 三角色均为 `receipt=matched`。
磁盘四程序与三个运行中 `/proc/<pid>/exe` 的 SHA-256 均匹配本次候选。

脱敏元数据副本位于被忽略的 `target/operations-evidence/ryzen-7950x-initialization-20261012/`，
原始空库快照留在目标机私有证据目录，不把它伪装为正式恢复 manifest。目标地址、凭据和机器专属绝对路径
不写入版本化文档。本次只证明指定制品在该机器的初始化、启动与就绪，不覆盖网页首次设置、浏览器操作、
竞价正确性、数据库并发、邮件/论坛业务、备份恢复演练、负载性能或完整生产验收；默认测试政策不变。

#### 后续 Admin 登录来源修正

同日公网登录返回 `403 origin_forbidden`：Admin runtime 的来源白名单仍只有 loopback，
请求在凭据校验前被拒绝。用户在获知明文风险后明确选择临时公网 HTTP，不选择 SSH 隧道或 HTTPS。
备份 runtime 配置并通过严格预检后，仅补入指定管理入口的精确 Origin；不使用通配符、不禁用
Origin/CSRF/认证、不修改凭据、Public 白名单或可信代理配置。临时明文配置不属于生产安全基线；
切换 HTTPS 后须移除该 HTTP 来源并启用 Secure Cookie。

操作前预计只重启 Admin，但实际 unit 的 Public `Requires=Admin` 依赖联动重启了 Public；
Worker InvocationID 未变。已明确记录这项计划偏差，后续单角色操作须先检查 systemd 依赖传播，
不能将命令只点名一个 unit 当作只影响该角色。两端重新 ready，三角色 adopted 回执重新匹配。
不含凭据的请求确认指定来源进入 `401 invalid_credentials`、未授权来源保持 `403 origin_forbidden`；
外部探测同样到达凭据校验。这不证明真实密码或浏览器登录流程已经验证。
原配置备份和前后状态保留在目标机私有 `admin-origin-fix-*` 目录，未将凭据写入取证输出。

### 后续 Cookie 校验候选更新

同日用户另行授权重编并推送。沿用 Ryzen CPU 白名单、Fat LTO、单 CGU 与 PostgreSQL/Redis
feature，重建四程序及两端页面，包含论坛/游戏 Cookie 手动只读校验。新候选为
`target/amd-artifacts/auction-ryzen-7950x-20261012T130030Z-dirty/`；同级 `.tar.gz`
为 28,702,541 字节，SHA-256 为
`a3599e89c19cc9f45e5c8eac853d5f38c93f4062d610d2f10c196c2f1cbf3ee1`。
参数、ELF ABI、内嵌资源隔离和构建阶段源码身份均核对通过；仍是脏工作树候选，不属于正式归档门禁。

上传超时发生在停机前；核对部分文件前缀后续传，再验证整包及逐文件哈希，未安装额外系统工具。
新程序严格配置预检通过后，持有既有部署控制锁，按 Public → Admin → Worker 停止并确认
全部 `inactive`、`MainPID=0`。旧四程序保留并核对哈希；使用新 `auctionctl` 创建及离线校验
PostgreSQL custom-format 备份，manifest 的 schema 版本为 39。本次没有迁移、恢复或配置修改。

随后原子替换四程序，按 Worker → Admin → Public 启动并分别等待 readiness。三个运行中制品
与新候选哈希一致，自动重启为 0，adopted 回执全部匹配。两端各 6 个实际 HTTP 静态资源
逐项匹配构建清单，外部两端 readiness 均为 200；后续同一 invocation 的 26 条日志未见 WARN/ERROR。
角色、公共、runtime 和敏感 TOML 的内容及权限指纹未变；Admin 启动仍按既有契约撤销旧会话。

旧程序、数据库备份及原始运维证据留在目标私有 `update-20261012T130030Z/`，脱敏结果副本位于
`target/operations-evidence/ryzen-7950x-cookie-update-20261012T130030Z/`。
本次仅确认制品切换、资源身份与就绪，不使用真实 Cookie 校验远端页面，也不执行业务流程、
浏览器测试或恢复演练。临时 HTTP 入口未改为 HTTPS；生产安全及完整验收边界不变。
本节为候选封存后的运维记录，不纳入包内构建前源码身份。
