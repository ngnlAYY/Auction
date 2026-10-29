# 构建工具与独立依赖锁

[CI 入口](ci/README.md) · [技术栈](docs/design/tech-stack.md)

本目录拥有构建工具的来源、依赖锁、安全补丁及安装回执。应用根 `Cargo.lock` 不覆盖已编译的 dx、sccache、
cargo-audit 或 rustup；更新应用依赖后仍须分别处理这些工具。脚本只在 Linux x86_64 工作，生成文件进入
`target/`，不修改 Auction 业务源码或用户全局工具配置。上游工具的最小源码补丁集中在 `patches/`，随固定源码与锁图审阅。

可选的 [blaze.sh](ci/tools/blaze.sh) 只调用机器已安装的 Blaze，提供独立本地开发试验入口；不属于下面的
release/audit 安装档，不安装、更新或全局启用编译器。使用与当前兼容限制由
[开发指南](docs/guide/development.md#blaze-本地试验)维护。

## 安装与验证

```bash
bash ci/tools/rustup.sh
bash ci/tools/bootstrap.sh install release
bash ci/tools/bootstrap.sh install audit
export PATH="$PWD/target/build-tools/bin:$PWD/target/audit-tools/bin:$PATH"
bash ci/tools/bootstrap.sh verify release
bash ci/tools/bootstrap.sh verify audit
```

`release` 安装 dx、sccache、esbuild、wasm-bindgen 和 wasm-opt；`audit` 只安装 cargo-audit。正式 Web 和发布入口
会验证 release 档回执并优先选择其工具，不会隐式重建工具。首次安装需要 Cargo、C 编译器、CMake、pkg-config、
OpenSSL 开发文件、patch 以及 curl/tar/coreutils；两档在 CI 中分别构建，二进制缓存命中仍重新检查回执和哈希。

| 工具 | 固定来源与修复 |
| --- | --- |
| rustup 1.29.2 | `rustup.sh` 固定官方 x86_64 GNU 安装器 SHA-256；更新旧版本并保留已安装 Rust 工具链 |
| dx 0.7.10 | 固定 crates.io 源码包及补丁 SHA-256，使用 `locks/dioxus-cli.Cargo.lock`；开启 `disable-telemetry,no-downloads` |
| sccache 0.18.0 | 固定 crates.io 源码包、Zstandard/缓存权限/object feature 补丁及 SHA-256，使用 `locks/sccache.Cargo.lock`；只启用 GHA 后端及默认本地缓存 |
| cargo-audit 0.22.2 | 固定 crates.io 源码包 SHA-256，使用 `locks/cargo-audit.Cargo.lock`；关闭默认 feature，只保留实际使用的锁文件审计 |
| esbuild 0.27.3 | 固定 dx 预期的 npm Linux x64 包与 SHA-256；只用于打包 |
| wasm-bindgen 0.2.129 | 固定与应用锁图一致的官方 musl 包与 SHA-256 |
| wasm-opt 129 | 固定 Binaryen 官方 Linux 包与 SHA-256；只提取已静态链接 Binaryen 的 wasm-opt |

三个 Rust 工具的锁图独立于 Auction 工作区。2026-10-22 修复将 rustls 精确锁至 0.23.46、rustls-webpki 至
0.103.16、webpki-roots/webpki-root-certs 至 1.0.10；保留只转发至 1.x 的 webpki-roots 0.26.11 包装。sccache
另需同步兼容的 aws-lc-rs、aws-lc-sys 和 pkg-config；锁图同时补齐 RustSec/OSV 命中的可兼容安全补丁，覆盖 h2、
quinn-proto、tar 等依赖。安装先校验 TLS 基线，再以 `--locked` 构建，正常发布不运行 `cargo update`。
完整上游锁还包含其他命令、平台或关闭功能的历史依赖，需结合实际调用路径评审；应用的零忽略审计门禁独立保留。

sccache 独立锁图另将 `ctutils` 从 0.4.2 更新至 0.4.4，应用根锁同步从 0.4.3 更新至 0.4.4，
修复 [RUSTSEC-2026-0385](https://rustsec.org/advisories/RUSTSEC-2026-0385.html)。只更新该包及其校验和，
不改变 sccache feature 或密码学协议。锁图变化会使旧 release 工具回执失效；使用前须经上述安装入口重建并校验，
不能手工改写回执来认可旧二进制。

三个工具独立锁另将 Tokio 统一到 1.53.3，修复
[RUSTSEC-2026-0386](https://rustsec.org/advisories/RUSTSEC-2026-0386.html) 的 `AsyncFd` I/O 安全公告；
应用根锁原已使用该版本。dx 的 `auth-git2` 升至 0.5.9；sccache 的 `chacha20`、`iana-time-zone`、
`spin` 0.9 分支分别升至 0.10.2、0.1.65、0.9.9，移除实际构建图中的 yanked 版本。
时区补丁同步解析其 Windows 依赖闭包，不扩展 Linux 以外的发布支持。工具版本、feature 与受控安装流程不变。

dx 编译期关闭自动下载，Web 入口同时设 `NO_DOWNLOADS=1`；补丁让 esbuild 和 wasm-opt 的所有查询/执行入口优先
受控 PATH，忽略历史下载缓存。wasm-bindgen 原有入口已遵守此顺序；缺配套工具时明确失败。配套下载先校验归档哈希再解包，
不用旧 dx 自身的 TLS 下载器。源码工具采用独立 GNU release profile、最多两个编译任务，并清除继承的 CPU、wrapper
和 profile 参数；不改变 Auction 原生/WASM 的 Fat LTO、单代码生成单元或 CPU 基线。

## 身份与缓存

工具源码构建清除应用的 profile、rustflags、编译器/包装器及增量覆盖环境变量，包括 Cargo 的
`CARGO_BUILD_RUSTC*`、`CARGO_BUILD_INCREMENTAL` 别名与 `CARGO_INCREMENTAL`，避免环境别名绕过独立构建配方。
发布制品的复用指纹则显式绑定这些输入，见[CI 构建身份](ci/README.md#发布边界)。

`target/build-tools/` 与 `target/audit-tools/` 各自包含 `bin/`、`lib/`、`BUILD-INFO` 和 `SHA256SUMS`。
回执绑定安装脚本、固定源码补丁、独立锁图、Rust 工具链、目标及精确文件清单；缓存恢复后重新验证每个二进制的内容。
安装开始冻结配方，完成时再次核对配方及实际源码锁；编译期间有变化则保留增量缓存并拒绝签发回执。
脚本、补丁、锁图或编译器变化会重建，工具安装目录采用临时目录完成后整体替换。

`bootstrap.sh fingerprint <档位>` 输出配方身份；`identity` 先验证已装工具再输出配方及二进制身份。release 的
identity 加入应用 `BUILD-INFO` 和 Web `WEB-BUILD-INFO` 的构建环境摘要，`ci/tools/` 同时纳入构建输入指纹，
修复后的同版本工具不会复用旧工具产物。发布/Web 共用 `configure_build_tools`；只接受环境变量启用的 sccache，
Cargo 配置文件中的包装器用空 `RUSTC_WRAPPER` 覆盖；启用 sccache 时固定其路径，并按
用户、仓库和工具身份配置 Linux 抽象 Unix socket，隔离默认端口上的旧 daemon。CI 的 sccache GHA 前缀为
`auction-tls-20261022`，隔离修复前的缓存对象。

## 上游公告与实际使用范围

2026-10-22 对修复锁同时核对 RustSec 和 OSV；cargo-audit 工具自身与 Auction 应用锁的漏洞/警告均为零。
dx/sccache 的完整发布锁仍有跨不兼容版本的公告与维护警告，下列判断限定于本仓库 Linux Web bundle 和 GHA 缓存路径，
不扩展到 `dx new/init/self-update`、Apple/Windows 打包或 sccache 分布式服务。

| 工具依赖与公告 | 当前路径依据 |
| --- | --- |
| dx 的 quick-xml 0.37.5/0.39.2，RUSTSEC-2026-0194/0195 | 分别来自 self_update 的 S3 XML 后端和 plist；`dx bundle --platform web` 不调用这些解析入口，版本更新提示使用 GitHub JSON |
| dx 的 rkyv 0.7.46，RUSTSEC-2026-0235 | 来自 parcel_sourcemap；`src/opt/css.rs` 只解析/压缩 CSS，未调用 source map 的归档反序列化；其 SourceMapInner 也不包含公告所需 Rc/Arc |
| dx 的 zip 2.4.2/4.6.1，RUSTSEC-2026-0367/0368/0369 | 前者由显式 self-update 使用；后者解包仅在 NSIS/WiX 工具下载，编译期 no-downloads 已阻断；Web bundle 不执行这些解包路径 |
| dx 的 ruzstd 0.8.2，RUSTSEC-2026-0344 | object/backtrace 的压缩调试数据解码；`src/build/assets.rs` 对本地编译 WASM 读取字节和符号，未调用压缩数据解码；backtrace 读取自身本地调试资料 |
| dx 的 git2、faster-hex 和 gix 旧分支 | cargo-generate 模板命令引入；Web bundle 不克隆模板。gix 的 checkout/submodule 漏洞所需执行组件未进入锁图，Windows 管理员专属公告不适用 Linux；gix-date 0.9.4 源码不含公告中的 TimeBuf API |
| dx 的 lru 0.12.5，RUSTSEC-2026-0002/0253 | ratatui 只用 `LruCache::get_or_insert` 缓存 `(Rect, Layout)`；未调用问题 IterMut/pop，键类型也无可抛 panic 的自定义析构 |
| sccache 的 jsonwebtoken 9.2.0，GHSA-h395-gr6q-cpjc | 由关闭的 dist-server feature 启用，`--no-default-features --features gha` 的 GNU 依赖图不编入它 |
| sccache 的 rsa 0.9.10，RUSTSEC-2023-0071 | 仍在 GHA 的传递依赖图，但实际认证使用 bearer 和预签名 SAS；不调用 RSA 私钥签名/解密，缺少 Marvin 所需私钥操作 |
| sccache 的 scc 2.1.0，RUSTSEC-2026-0205 | 由测试依赖 serial_test 引入，不在本次 release 普通依赖图中 |
| sccache 的 zip 0.6.6，RUSTSEC-2026-0368/0369 | 按编译器预期键读取，写入普通临时文件后放到编译器指定路径；不调用归档 extract，也不创建归档提供的符号链接 |
| sccache 的 zip 0.6.6，RUSTSEC-2026-0367 | 正常写入限制在 `0o777`；本仓库补丁让自定义读取也先掩码 `unix_mode`，阻止缓存恢复 SUID/SGID/sticky。版本仍匹配公告，继续以非 root 用户构建并使用受控缓存，升级时重新核对 |

公告可从 [RustSec 数据库](https://rustsec.org/advisories/)及对应上游 security advisory 核对；构建工具升级需重新检查
上述调用边界。维护停止和 yanked 提示仍作为上游工具后续升级事项保留，不加入应用审计忽略列表。

2026-10-23 复审使用 1350 条 RustSec 公告：应用锁与 cargo-audit 独立锁均无漏洞或警告，
dx 完整锁有 13 个漏洞匹配项、12 个警告，sccache 完整锁有 5 个漏洞匹配项、12 个警告。
本轮 GNU 普通/构建依赖树仍包含上述 dx/sccache 漏洞包，不能把受限调用路径等同于“未编译”或“依赖安全”；
sccache 的 `scc` 仅位于测试图，另外三个 yanked 包仍在实际 GHA 构建图内。
安装回执只证明固定配方与二进制一致，不消除这些公告。未跨不兼容版本强行替换工具内部库，也未加入公告忽略项；
受限用途、非 root 构建和受控缓存条件继续有效，检查证据见[本轮状态](docs/status.md)。

最终使用 1352 条公告（数据库提交 `8a5b8090ef20c9c1b425f3373f28b0b23cc5c7a0`）复审上述兼容更新；应用锁在 WebSocket 类型对齐后为 536 个条目：

| 独立锁 | 漏洞匹配项 | 警告 | GNU 普通/构建图 |
| --- | ---: | ---: | --- |
| Auction 应用 | 0 | 0 | 三角色、server、auctionctl 与双端 WASM 分别核对 |
| cargo-audit | 0 | 0 | 未命中公告或 yanked 包 |
| dx | 13 | 11 | 漏洞包仍编入；没有 yanked 包 |
| sccache | 5 | 9 | rsa、ruzstd、zip 仍编入；仅完整锁的测试依赖 scc 仍 yanked |

两档工具已按更新后的锁重建并校验回执。cargo-audit 的上游宏仍产生 21 条 future-incompatible 编译警告，
sccache 仍有一条 dead-code 编译警告；它们不是应用 Clippy 结果，也不是 RustSec 公告数量。
剩余跨不兼容版本问题未通过强制 patch 或忽略公告掩盖，上表及受限调用边界不构成工具链无风险声明。

2026-10-24 在独立新公告库重新执行四份锁审计：1352 条公告，提交
`21d909e4c8bd4e14c7aea58bf980c881a0053ab1`，漏洞/警告结果与上表一致，四份锁均未修改。
不复用用户公告工作树中的未跟踪文件，也不把公告通过扩展为运行或供应链无风险证明。
2026-10-25 全仓复审重新获取独立公告库，仍为同一提交与 1352 条公告；四份锁结果与上表一致。
本轮没有修改工具锁、安装回执或公告忽略项；应用功能裁剪不能消除 dx/sccache 的剩余风险。

许可元数据与工具用途也须分开：应用 516 个外部锁包均声明许可证，原生活跃图中的四个 MPL 包仍需履行分发义务，
现有 `DEPENDENCIES.md` 不自动提供许可证文本或对应源码。dx 的实际 GNU 构建图另含
`imagequant 4.4.1`（`GPL-3.0-or-later`）、`mozjpeg 0.10.13`（`IJG`）及 13 个 MPL 包；
这些不进入应用/WASM 依赖图，但分发 dx 二进制或工具缓存时必须独立审查对应许可与源码义务。

升级工具或应用 wasm-bindgen 时，核对官方源码/制品身份与依赖公告，更新对应锁图、固定哈希及配套版本，运行
`bash -n`、ShellCheck、Actions 静态检查和相关工具构建，再按项目默认范围执行编译门禁。不得通过忽略公告或降低
应用 `cargo audit --deny warnings` 门禁完成升级。

### 2026-10-26 Zstandard 公告

本节结合 [RustSec 的 zstd-safe 公告](https://rustsec.org/packages/zstd-safe.html)、已锁版本、离线 Linux
feature 图及源码调用路径；版本匹配不能单独证明实际入口可触发漏洞。

- 升级前 dx 锁定 `zstd-safe 7.2.4`，版本匹配 RUSTSEC-2026-0395–0406 共 12 项，其中 0398、0403 为漏洞；
  sccache 仍锁定 7.1.0，匹配其中 9 项，其中 0403 为漏洞。修复版本为 `zstd-safe >=8.2.0`。
  两个工具实际的 GNU feature 图均未启用 0398 所需的 `seekable`。0403 需要 legacy 帧及受影响的输出缓冲区；
  dx 图未启用 `legacy`，sccache 虽启用该 feature，缓存读取实际使用 `zstd::stream::copy_decode`，
  它使用公告明确排除的 `read::Decoder`。这些限定不证明两个工具的所有调用路径安全。
- 升级前 dx 的 `zstd 0.13.3` 还匹配 [RUSTSEC-2026-0391](https://rustsec.org/advisories/RUSTSEC-2026-0391.html)，
  触发条件是复用 `with_context`。当前 `dx bundle --platform web` 源码路径和已查依赖未发现该调用；
  其 feature 图经 `tower-http`、`async-compression` 和 `compression-codecs` 引入 zstd。sccache 的
  `zstd 0.13.1` 不在 0391 受影响版本范围。
- 升级前 dx 的 `zstd-sys 2.0.16` 与 sccache 的 `2.0.10` 匹配
  [RUSTSEC-2026-0410](https://rustsec.org/advisories/RUSTSEC-2026-0410.html) 的版本范围，但该公告限定
  wasm32；本入口只构建 Linux x86_64 工具，不能将这一结论扩展至其他平台。
- dx 锁已按受控候选升级：`compression-codecs` 0.4.38 → 0.4.40、`compression-core` 0.4.32 → 0.4.34，
  `zstd` 0.13.3 → 0.14.3、`zstd-safe` 7.2.4 → 8.2.0、`zstd-sys` 2.0.16 → 2.1.2；
  离线 GNU feature 图其余包及 feature 未变。新 Zstandard 包声明 BSD-3-Clause 许可，分发工具缓存时须纳入许可审查。
  sccache 0.18.0 及 0.19.0 均要求 `zstd ^0.13`，不能只通过锁文件升级到修复版。

本轮使用受控 cargo-audit 0.22.2、`--deny warnings` 和独立公告库执行完整锁审计。公告库包含 1378 条公告，
提交为 `3279c99911354c400e1d41e47c7d60e39258b147`；数字包含未启用 feature、其他平台及测试依赖，不能直接等同可利用漏洞。

| 锁图 | 漏洞匹配项 | 被拒绝的警告 | 结果 |
| --- | ---: | ---: | --- |
| Auction 应用 | 0 | 0 | 通过 |
| cargo-audit | 0 | 0 | 通过 |
| dx 升级前 | 17 | 21 | 失败 |
| sccache | 7 | 17 | 失败 |
| dx 升级后 | 13 | 11 | 失败 |

应用锁和工具锁分别审计，不为工具公告设置忽略项。dx 的完整锁仍命中其他公告；受限调用路径不等于零风险。
受控 cargo-audit 已重建；dx 和 sccache 按 release 配方重建并签发回执，sccache 锁没有变化。

### 2026-10-27 公告复核

使用已校验回执的 cargo-audit 0.22.2，刷新 RustSec 至提交 `99670cfc0bc574433034b75876a4b791ccaa2efb`
（1,380 条公告），应用和三个工具锁共用该快照执行 `--deny warnings`：应用与 cargo-audit 均为零漏洞、零警告；
dx 仍为 13 项漏洞匹配、11 项警告，sccache 仍为 7 项漏洞匹配、17 项警告。四份锁文件未调整，工具公告未设忽略项；
受影响 API、平台和现有受控调用范围继续见上两节。本轮只刷新审计记录，未重建工具或执行发布。

同日后续[边界复核](docs/status.md#2026-10-27-批量持久化与边界复核)在独立干净快照重跑四锁，结果不变。
补齐 Cargo 环境别名隔离后，audit/release 两档工具已重建并通过回执校验；工具版本和锁图保持原样，
cargo-audit 的 21 条上游 future-incompatible 警告及 sccache 的 1 条 dead-code 警告仍保留，未执行应用发布。

### 同日并行审计：锁文件扫描与 sccache Zstandard 修复

重新从上游拉取独立 RustSec 工作树，仍为提交 `99670cfc0bc574433034b75876a4b791ccaa2efb`、1,380 条公告；
四份完整锁均使用同一快照、`--deny warnings` 和默认 yanked 检查，无忽略项。另从锁定源码重建 Linux GNU 普通/构建图，
dx 采用 `disable-telemetry,no-downloads`、sccache 采用 `gha`，均关闭默认 feature；不把测试和其他平台包算入活跃图。

| 锁图 | 完整锁条目 | 完整锁漏洞 / 警告 | GNU 活跃包 | 活跃图漏洞 / 警告 |
| --- | ---: | ---: | ---: | ---: |
| Auction 应用 | 535 | 0 / 0 | 五个程序及双端 WASM 分别核对 | 0 / 0 |
| cargo-audit | 405 | 0 / 0 | 293 | 0 / 0 |
| dx | 810 | 13 / 11 | 641 | 13 / 10 |
| sccache 修复前 | 532 | 7 / 17 | 298 | 7 / 10 |
| sccache 修复后 | 532 | 5 / 9 | 298 | 5 / 2 |

活跃图数字是完整锁报告与实际包/版本集合的交集，仍包含所用命令未调用的 API；不按公告数量推断可利用性。
dx 的 `instant` 警告未启用；sccache 的 `buf_redux/multipart/paste/safemem/twoway` 警告及测试依赖 `scc` 的
unsound/yanked 警告未启用。修复后 sccache 的两项活跃警告来自 bincode 与 number_prefix 的维护状态。

`cargo-audit` 仅用于 Cargo.lock，关闭未使用的
[`binary-scanning`](https://docs.rs/crate/cargo-audit/0.22.2/features)，GNU 图从 302 减至 293 个包，移出
auditable-extract/info/serde、binfarce、object 0.39、quitters、topological-sort 和两代 wasmparser。
RustSec/yanked 审计和全部警告门禁保留，完整独立锁仍保留可选包。

sccache 上游清单限制 `zstd ^0.13`，本入口通过固定哈希、零模糊匹配的
[清单补丁](ci/tools/patches/sccache-zstd.patch)改用 0.14.3；独立锁仅替换 zstd、zstd-safe、zstd-sys 三项，
分别锁至 0.14.3、8.2.0、2.1.2。其余包和全部 feature 不变，包含 legacy 支持。缓存路径仅调用
`stream::copy_encode/copy_decode`，两版接口及包装实现一致，保持 Stored ZIP 内的 Zstandard 流和原压缩等级配置。
完整锁消除 RUSTSEC-2026-0403、0410 及八项 Zstandard unsound 匹配；0410 原限 wasm32，本工具只编译 GNU x86_64。

另以[缓存权限补丁](ci/tools/patches/sccache-cache-permissions.patch)将 `get_object` 返回的 `unix_mode` 限制为
`0o777`；所有缓存恢复经此入口取得权限，避免把伪造条目的高权限位传给 `set_permissions`。此边界缓解保留普通文件的
rwx 位，不依赖 ZIP 提取函数，不消除 zip 0.6.6 在完整锁/活跃图中的公告匹配。

剩余 dx 的 13 项和 sccache 的 5 项漏洞匹配均仍在实际工具图中：dx 的 XML、自更新/安装器 ZIP、模板 Git、CSS rkyv，
以及两工具的 object/ruzstd 路径限制见上文；sccache 仍保留 RSA 与 ZIP。修复版本跨越上游约束或尚无修复，
本轮未强改这些 API，也未忽略公告；工具须继续以非 root 用户和受控缓存运行。

audit/release 两档已按最终脚本、补丁和锁图串行重建，每次最多两个 Cargo 编译任务；冻结配方、源码锁比对、安装回执及
两档独立 verify 均通过。cargo-audit 保留 17 条上游 future-incompatible 编译警告，sccache 保留 1 条上游 dead-code
编译警告；这些不计入上表的 RustSec 警告。本轮未运行应用发布构建、功能流程或行为测试。

### object 压缩依赖裁剪

后续全仓审计核对 sccache 0.18.0 的全部 `object` 调用：仅 `src/util.rs` 使用 `ArchiveFile`、
`MachOFatFile32/64`，按归档成员名称与原始字节计算摘要，不解压目标文件节。固定的
[object 0.37.1 feature](https://docs.rs/crate/object/0.37.1/features) 将 `compression` 独立于 `read`；
`ArchiveMember::data` 直接读取成员的原始字节。通过
[受控补丁](ci/tools/patches/sccache-object-features.patch)关闭默认 feature，显式保留 `read/std`，
后者保留既有错误类型和标准库能力；补丁 SHA-256 参与来源校验及构建配方身份。

Cargo 重新解析后，独立锁只删除 `ruzstd 0.8.1`、`twox-hash 2.1.1` 和 `object` 的压缩依赖边：
完整锁 532 → 530，GNU 普通/构建图 298 → 296。其余包版本不变；`flate2` 仍由其他用途引入，
缓存自己的 `zstd::stream` 编解码、原始归档哈希顺序、缓存格式与权限掩码保持不变，应用锁不变。

本次独立拉取的 RustSec 仍为 `99670cfc0bc574433034b75876a4b791ccaa2efb`、1,380 条公告。
同快照、`--deny warnings` 下最终 sccache 完整锁由 5 项漏洞匹配降至 4 项，警告仍为 9 项；
剩余 RSA、ZIP 及维护/yanked 警告仍使审计失败，未设置忽略项。应用和 cargo-audit 锁继续零漏洞/警告，
dx 完整锁仍为 13 项漏洞匹配、11 项警告；它们的锁字节未变，历史审计数字保留为当时记录。

最终 audit/release 两档已重新构建并通过各自安装回执校验，sccache/dx 版本命令正常返回；
固定哈希补丁以零模糊匹配应用，构建使用 `--locked`。cargo-audit 仍有 17 条上游宏兼容警告，
sccache 仍有 1 条上游 dead-code 警告。上述证据不包含应用发布、运行流程或缓存性能基准。
