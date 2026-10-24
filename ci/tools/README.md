# 构建工具与独立依赖锁

[CI 入口](ci/README.md) · [技术栈](docs/design/tech-stack.md)

本目录拥有构建工具的来源、依赖锁、安全补丁及安装回执。应用根 `Cargo.lock` 不覆盖已编译的 dx、sccache、
cargo-audit 或 rustup；更新应用依赖后仍须分别处理这些工具。脚本只在 Linux x86_64 工作，生成文件进入
`target/`，不修改 Auction 业务源码或用户全局工具配置。上游工具的最小源码补丁集中在 `patches/`，随固定源码与锁图审阅。

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
| sccache 0.18.0 | 固定 crates.io 源码包 SHA-256，使用 `locks/sccache.Cargo.lock`；只启用 GHA 后端及默认本地缓存 |
| cargo-audit 0.22.2 | 固定 crates.io 源码包 SHA-256，使用 `locks/cargo-audit.Cargo.lock`；保留默认的 binary-scanning 能力 |
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

dx 编译期关闭自动下载，Web 入口同时设 `NO_DOWNLOADS=1`；补丁让 esbuild 和 wasm-opt 的所有查询/执行入口优先
受控 PATH，忽略历史下载缓存。wasm-bindgen 原有入口已遵守此顺序；缺配套工具时明确失败。配套下载先校验归档哈希再解包，
不用旧 dx 自身的 TLS 下载器。源码工具采用独立 GNU release profile、最多两个编译任务，并清除继承的 CPU、wrapper
和 profile 参数；不改变 Auction 原生/WASM 的 Fat LTO、单代码生成单元或 CPU 基线。

## 身份与缓存

`target/build-tools/` 与 `target/audit-tools/` 各自包含 `bin/`、`lib/`、`BUILD-INFO` 和 `SHA256SUMS`。
回执绑定安装脚本、固定源码补丁、独立锁图、Rust 工具链、目标及精确文件清单；缓存恢复后重新验证每个二进制的内容。
安装开始冻结配方，完成时再次核对配方及实际源码锁；编译期间有变化则保留增量缓存并拒绝签发回执。
脚本、补丁、锁图或编译器变化会重建，工具安装目录采用临时目录完成后整体替换。

`bootstrap.sh fingerprint <档位>` 输出配方身份；`identity` 先验证已装工具再输出配方及二进制身份。release 的
identity 加入应用 `BUILD-INFO` 和 Web `WEB-BUILD-INFO` 的构建环境摘要，`ci/tools/` 同时纳入构建输入指纹，
修复后的同版本工具不会复用旧工具产物。发布/Web 共用 `configure_build_tools`；启用 sccache 时固定其路径，并按
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
| sccache 的 ruzstd 0.8.1，RUSTSEC-2026-0344 | object 只读取静态库归档的原始成员用于哈希，未调用压缩节解码；缓存解压另用 zstd |
| sccache 的 zip 0.6.6，RUSTSEC-2026-0368/0369 | 按编译器预期键读取，写入普通临时文件后放到编译器指定路径；不调用归档 extract，也不创建归档提供的符号链接 |
| sccache 的 zip 0.6.6，RUSTSEC-2026-0367 | 正常写入将权限限制在 `0o777`，自定义读取仍会原样恢复 `unix_mode`；伪造缓存加高权限恢复可保留 SUID/SGID。当前本机及 CI 以非 root 用户构建并使用受控缓存，后续须保持此条件，升级时重新核对 |

公告可从 [RustSec 数据库](https://rustsec.org/advisories/)及对应上游 security advisory 核对；构建工具升级需重新检查
上述调用边界。维护停止和 yanked 提示仍作为上游工具后续升级事项保留，不加入应用审计忽略列表。

2026-10-23 复审使用 1350 条 RustSec 公告：应用锁与 cargo-audit 独立锁均无漏洞或警告，
dx 完整锁有 13 个漏洞匹配项、12 个警告，sccache 完整锁有 5 个漏洞匹配项、12 个警告。
本轮 GNU 普通/构建依赖树仍包含上述 dx/sccache 漏洞包，不能把受限调用路径等同于“未编译”或“依赖安全”；
sccache 的 `scc` 仅位于测试图，另外三个 yanked 包仍在实际 GHA 构建图内。
安装回执只证明固定配方与二进制一致，不消除这些公告。未跨不兼容版本强行替换工具内部库，也未加入公告忽略项；
受限用途、非 root 构建和受控缓存条件继续有效，检查证据见[本轮状态](docs/status.md)。

升级工具或应用 wasm-bindgen 时，核对官方源码/制品身份与依赖公告，更新对应锁图、固定哈希及配套版本，运行
`bash -n`、ShellCheck、Actions 静态检查和相关工具构建，再按项目默认范围执行编译门禁。不得通过忽略公告或降低
应用 `cargo audit --deny warnings` 门禁完成升级。
