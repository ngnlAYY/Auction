# AMD KVM 定向编译候选

[文档导航](../../docs/README.md) · [开发指南](../../docs/development.md) · [部署手册](../deploy/README.md) · [验证状态](../../docs/status.md)

两份 Cargo 配置仅供指定的 Linux x86_64 Debian 13/glibc 2.41 KVM，以 --config 显式叠加目标、CPU 参数和构建并发；不改变 WASM/正式发布流程，不保证通用兼容性或最优性能。

## 两份目标配置

| 配置 | 已登记目标 | 指令集策略 |
| --- | --- | --- |
| [ryzen-7950x.toml](ryzen-7950x.toml) | Ryzen 9 7950X，2 vCPU、约 2 GiB RAM | x86-64-v4 加快照扩展，含 RDSEED |
| [epyc-9645.toml](epyc-9645.toml) | EPYC 9645，4 vCPU、约 8 GiB RAM | v4 加快照扩展、AVX-VNNI/VP2INTERSECT，不启用 RDSEED |

兼容性取决于 VM CPUID，不是型号；EPYC 快照无 rdseed，不能启用完整 znver5。
禁用 native、不稳定 tune-cpu 和型号隐含 LLVM 特性，不声称 Zen 专用调度优化。
迁移宿主机/改变 CPU 模式须重新核对，否则可能非法指令退出。

两份配置继承[根 profile](../../Cargo.toml)：opt-level=3、Fat LTO、单 CGU、非增量、panic=unwind，保留后端任务展开语义，不重复定义。
构建并发 2 仅限制编译内存，不改变运行线程/池；约 2 GiB Ryzen VM 不宜承担完整 LTO 构建。

## 在构建机生成候选

从仓库根执行，Web 先独立构建，后端 CPU 参数不得传给 WASM。
不要另加 CARGO_PROFILE_RELEASE_*、CFLAGS/CXXFLAGS 或全局 CPU 覆盖，尤其 -march=native。

```bash
just web-build

for machine in ryzen-7950x epyc-9645; do
  build_dir="target/amd-build/$machine"

  # Web 更新后重新展开 RustEmbed，保留其他依赖缓存。
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

输出 `target/amd-build/<machine>/x86_64-unknown-linux-gnu/release/`，分档保管制品/校验和。
命令不安装、启动、迁移或替换服务；本地候选不归 ci/release.sh 管理，禁混入通用包。
已登记构建机与目标 glibc 均为 2.41，仍须逐制品核对符号/动态库；构建机仅 AVX2，不能执行 AVX-512 候选。

## 已有测量和待补证据

以下是 2026-10-12 已登记记录，本次文档维护未重跑。授权微基准在 Intel KVM 上覆盖真实 domain 规划器
9 类规划及计划 JSON 解码/校验：10 组各 32 份输入，每轮 200,000 次，6 候选交错 9 轮，绑 CPU 2，
预热在计时外并使用 black_box。未连接数据库或外部服务。

| 本机候选 | 10 组等权几何平均相对吞吐 |
| --- | --- |
| x86-64 默认 Release / 16 CGU | 1.000× |
| x86-64 Thin LTO / 16 CGU | 1.057× |
| x86-64 Fat LTO / 1 CGU | 1.106× |
| v3 Thin LTO / 16 CGU | 1.097× |
| v3 Thin LTO / 1 CGU | 1.132× |
| v3 Fat LTO / 1 CGU | 1.158× |

每组取 9 轮批量平均耗时中位数，再对速度比作等权几何平均；不是业务流量权重、p99 或 PostgreSQL 吞吐。
只能据此保留 Fat/1 候选，不能外推 AMD 系统提升。原始输入、样本、日志和临时程序在忽略的
`target/compiler-performance/`；其中两种 AMD bin 是微基准，不是服务，已编译但未在本机执行。
其机器码哈希相同也不证明目标特性无用或已提升性能。

两档完成 PostgreSQL/Redis/Web feature 的 Release cargo check；Ryzen 另完成服务链接和获授权的目标机运维，
EPYC 没有专用服务制品/运行验证。两档均未完成 PostgreSQL 端到端、并发/p99 或 PGO 训练；
继续测量仅使用明确授权的隔离负载，纯领域训练不能代表全服务。默认测试禁令不变。

### 2026-10-12 Ryzen 7950X 重编记录

初次四程序/双端页面候选封存于
`target/amd-artifacts/auction-ryzen-7950x-20261012T044747Z-dirty/`，附 archive、SHA256SUMS、构建日志和证据。
源码身份在构建前冻结、阶段复核，内嵌标记/资源隔离、PostgreSQL/Redis 与 CPU/Fat LTO/单 CGU 指纹均核对。
三个角色最高引用 GLIBC 2.34，auctionctl 2.39；动态依赖为 libgcc_s.so.1、libm.so.6、libc.so.6。
保留第三方警告，未放松 lint。该脏树候选不是正式归档门禁产物；封存后文档变更不计入构建前身份。

### 2026-10-12 Ryzen 目标初始化与启动

后续独立授权 SSH/管理员维护后，备份并修正了沿用构建机的 systemd/TOML 路径和 control 权限，未改凭据。
确认三角色 inactive/MainPID=0、PostgreSQL 18.6 专用目标为空且可写，保存普通 pg_dump 空库快照后，
以服务账号受停机门禁执行 39 条迁移（最大版本 39、失败 0、64 张 public 表），无 DROP/重建。
Worker → Admin → Public readiness 通过，自动重启 0、adopted 回执匹配，磁盘及运行制品哈希一致。

脱敏证据在 `target/operations-evidence/ryzen-7950x-initialization-20261012/`，原空库快照在目标私有目录，
不作为正式恢复 manifest。只证明指定制品的初始化/启动/就绪，不涵盖首次网页、业务、并发、恢复演练或性能。

#### 后续 Admin 登录来源修正

同日 origin_forbidden 源于 runtime 仅允许 loopback。用户明确选择临时公网 HTTP 后，备份配置、严格预检并
仅添加指定精确 Origin，未放松 CSRF/认证或修改凭据。该例外不属于生产安全基线，切换 HTTPS 后须移除 HTTP 来源并启用 Secure Cookie。
Admin 重启经 Requires 联动 Public，Worker invocation 未变；已记录这项超出原单角色计划的影响。
两端重新 ready、回执匹配；无凭据探测仅证明允许来源到达 401、其他来源仍 403，不证明真实登录完成。
原始证据/备份留在目标私有 admin-origin-fix-* 目录。

### 后续 Cookie 校验候选更新

同日另行授权构建并推送新四程序/双端页面，沿用目标优化和 PostgreSQL/Redis feature，包含 Cookie 手动只读校验。
候选 `target/amd-artifacts/auction-ryzen-7950x-20261012T130030Z-dirty/` 的 archive SHA-256 为
`a3599e89c19cc9f45e5c8eac853d5f38c93f4062d610d2f10c196c2f1cbf3ee1`，参数/ABI/资源/源码身份核对通过。
上传超时在停机前续传并验整包/逐文件哈希；持部署锁停三角色，保留旧程序，用新 auctionctl 备份并离线校验 schema 39 恢复点。

原子替换后按顺序 ready，运行哈希、回执、两端各 6 项 HTTP 静态资源与清单匹配，自动重启 0；
配置内容/权限指纹不变，无迁移/恢复或真实 Cookie/业务验证。原备份和旧程序留在目标私有
update-20261012T130030Z/，脱敏副本在 `target/operations-evidence/ryzen-7950x-cookie-update-20261012T130030Z/`。
此记录发生在候选封存后，仅证明切换、资源身份与就绪；临时 HTTP 和完整生产验收缺口保持不变。
