# CI 与发布

[docs/README.md](../docs/README.md) · [docs/development.md](../docs/development.md) · [ci/tools/README.md](tools/README.md) · [ops/README.md](../ops/README.md)

| 目录 | 内容 |
| --- | --- |
| [ci/checks](checks) | [ci/checks/README.md](checks/README.md)：[ci/checks/check.sh](checks/check.sh)（策略、格式、编译、lint、审计）与对应脚本/基线 |
| [ci/release](release) | 发布入口 [ci/release/release.sh](release/release.sh)、Web payload 构建 [ci/release/web-bundle.sh](release/web-bundle.sh)、共用身份函数 [ci/release/build-identity.sh](release/build-identity.sh)、部署制品来源校验 [ci/release/verify-deployment-package.sh](release/verify-deployment-package.sh)、体积预算 [ci/release/web-size-budget.v1](release/web-size-budget.v1) |
| [ci/tools](tools) | 受控工具链与构建工具的安装、校验与本地 Blaze 试验入口 |

发布工具面向 Linux x86_64；装包还需 Perl（重定位文档链接）。CI 的 Cargo 编译、Clippy 与依赖图解析命令使用 `--locked`。格式检查和独立审计工具使用各自的参数。

## 静态门禁：`ci/checks/check.sh`

```bash
bash ci/checks/check.sh <检查项>...
```

| 检查项 | 内容 |
| --- | --- |
| `repository` | `AGENTS.md`、`.gitignore`、`.editorconfig`、`README.md`、`Cargo.lock` 必须纳入版本控制且未被忽略 |
| `layout` | `src/` 根只能有 `lib.rs`/`main.rs`；每个 crate 的 `src/` 必须有职责子目录；同目录不得有 ≥3 个同前缀源码文件；目录模块入口为 `mod.rs`，不与同名 `.rs` 并存；文件与目录名 snake_case，不用 common/helpers/shared/misc/utils 兜底命名；生产源码不超过 600 行、不用私有 glob 导入；全部源码不用 `use { … }` 总括分组 |
| `import-groups` | 导入块按 std → 外部 crate → 本 crate 三组排列，口径见[ci/checks/README.md#导入分组](checks/README.md#导入分组) |
| `module-docs` | `apps/*/src` 与 `crates/*/src` 的每个非测试 `.rs` 文件首行必须是 `//!` 模块说明（`tests/`、`tests.rs`、`*_tests.rs` 除外） |
| `hot-fields` | Admin 参数页的热字段镜像（`crates/auction-web-admin/src/surface/config/hot_fields.rs`）必须与应用层闭集 `WORKER_HOT_FIELD_PATHS` 逐项同序 |
| `systemd-templates` | [ops/deploy/systemd](../ops/deploy/systemd) 的 Public/Admin unit 模板须以 `After=`/`Wants=` 引用 Worker（Public 另含 Admin），角色之间不得用 `Requires=` 传播停止与失败 |
| `config` | `ops/config/examples/**/*.toml` 每个赋值字段（含注释掉的示例字段）前必须紧邻含“可选”或“必填”的注释 |
| `i18n` | [crates/auction-web-public/i18n](../crates/auction-web-public/i18n) 与 [crates/auction-web-admin/i18n](../crates/auction-web-admin/i18n) 恰好包含 en、ja、ko、zh-CN、zh-TW 五个 `.ftl`；各语言键集合与 `en.ftl` 一致；代码中 `t!("…")` 引用的键必须存在；键名不得含点号、不得重复；`en.ftl` 的每个键必须在同一 surface 源码的字符串字面量中可达（含设置表单 `{id}-help` 拼接规则），规则见 [ci/checks/i18n-unused-keys.pl](checks/i18n-unused-keys.pl) |
| `equipment-contract` | 装备解析诊断原因在 Rust、SQL 约束与 Admin 文案之间的闭集一致 |
| `ratchet` | 源码计数不得超过仓库基线；下降时提示更新，口径与更新方式见[ci/checks/README.md#源码计数棘轮](checks/README.md#源码计数棘轮) |
| `ratchet-counts` | 仅输出当前 15 项计数，不修改基线；供人工审阅下调 |
| `work-item-ids` | `apps/`、`crates/`、`ci/` 与 `ops/` 不得引用已删除计划台账的工作项编号（`W<数字>.<数字>`）；`W11.4` 是迁移演练回执的协议固定值，不在此列 |
| `test-module-paths` | `apps/*/tests` 与 `crates/*/tests` 不得出现 `#[path]`：`src` 部分已由计数棘轮 `all:#[path]`（基线 0）覆盖，本项只兜底测试树；规则见[ci/checks/README.md#模块层级门禁](checks/README.md#模块层级门禁) |
| `port-coverage` | 只读报告 `crates/auction-storage` 中存储端口方法的测试调用覆盖与未覆盖清单；不并入 `all`/`just verify`，不使命令失败，见[ci/checks/README.md#存储端口覆盖率报告不并入-just-verify](checks/README.md#存储端口覆盖率报告不并入-just-verify) |
| `markdown-links` | 离线检查全部 Markdown 的相对链接目标、同页与跨页锚点，规则见[ci/checks/README.md#markdown-相对链接](checks/README.md#markdown-相对链接) |
| `migration-compatibility` | 075 起每个迁移末尾恰好登记自身一次，闭集等级、版本及事务规范见[ci/checks/README.md#迁移兼容登记](checks/README.md#迁移兼容登记) |
| `published-migrations` | 每个发布 tag（`v<版本>`）中存在的 `migrations/postgres/*.up.sql` 必须在当前工作树逐字节不变；禁止修改、重命名或删除已发布文件，只允许追加新编号 |
| `all` | `repository layout import-groups module-docs hot-fields config systemd-templates i18n equipment-contract ratchet test-module-paths work-item-ids markdown-links migration-compatibility published-migrations` |
| `fmt` | `cargo fmt --all -- --check` |
| `check` | `cargo check --locked --workspace --all-targets` |
| `lint` | `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` |
| `dependency-versions` | `auction-server` 正常依赖图只能包含一个 `tungstenite` 版本；Axum 的 WebSocket 错误按版本 downcast，跨版本并存会静默丢失 1009 close 契约 |
| `wasm` | 以 `wasm32-unknown-unknown` 目标运行 `cargo clippy -D warnings`：Public（连同 `auction-web-shared`、`auction-contracts`）与 Admin 分开检查，各自保留独立 feature 图；`cfg(target_arch = "wasm32")` 代码只在此项被 lint |
| `rust` | `fmt check dependency-versions lint wasm` |
| `dependency-audit` | `cargo audit --file Cargo.lock --deny warnings`（需要受控 cargo-audit，联网读取 RustSec）；已核实不进入编译图的例外登记在 [.cargo/audit.toml](../.cargo/audit.toml)，每条写明原因与解除条件，定时审计复查 |
| `release-dependency-audit` | 取最高版本的发布 tag（`v<semver>`，口径同发布守卫），把其中的 `Cargo.lock` 读到临时目录后以同样参数审计；没有发布 tag 时提示并通过。需要该 tag 的提交对象，见[ci/checks/README.md#每周依赖审计](checks/README.md#每周依赖审计) |
| `dependency-unused` | `cargo machete`（0.9.2） |

`just verify` = `all fmt check dependency-versions lint wasm`。默认门禁不运行测试。

## GitHub Actions

### 普通 CI：`.github/workflows/ci.yml`

| 作业 | 触发 | 内容 |
| --- | --- | --- |
| `docs` | 所有分支 push / PR / 手动 | 策略门禁与格式（`ci/checks/check.sh all fmt`）；完整历史与 tag 用于 `published-migrations`；同一提交已有 PR 时跳过重复的分支 push。tag 推送只运行发布工作流，不重复普通 CI |
| `verify` | push / PR / 手动，`docs` 通过后 | `ci/checks/check.sh check dependency-versions lint wasm`（工具链 1.99.0） |
| `test` | push / PR / 手动，`docs` 通过后 | 安装 PostgreSQL 18 后依次执行 `just test`、`just test-db`（PostgreSQL 18 集成测试）与 `just test-db-compat`（迁移兼容声明验证）；不并入默认 `just verify` |
| `security-audit` | push / PR / 手动中 `docs` 通过后；每周一 04:17 UTC 独立调度 | 受控 cargo-audit 的 `dependency-audit`；定时与手动运行另以完整历史执行 `release-dependency-audit`；非定时事件另执行 `dependency-unused`，见[ci/checks/README.md#每周依赖审计](checks/README.md#每周依赖审计) |
| `msrv` | push / PR / 手动，`docs` 通过后 | `cargo +1.94.1 check --locked --workspace --all-targets --all-features` |
| `artifacts` | 默认分支 push 或手动 | 调用可复用工作流 `.github/workflows/release-build.yml`：安装受控发布工具，`ci/release/release.sh release-check` 构建并校验完整发布包，上传 1 天候选包；候选包不可部署 |

### 依赖升级窗口提醒：`.github/workflows/dependency-cadence.yml`

每周一 **05:07 UTC** 与手动触发时运行 [ci/checks/dependency-cadence.pl](checks/dependency-cadence.pl)，以 [ops/dependencies/upgrade-windows.json](../ops/dependencies/upgrade-windows.json) 为准核对季度窗口。日历结构错误、窗口逾期仍为 `scheduled` 或缺少下一次窗口时作业失败；窗口进入 14 天提前提醒期或正在进行时输出 warning，并把窗口表写入作业摘要。作业只读取仓库文件与 UTC 日期，不修改日历、不创建 issue、不升级依赖、不联网。节奏、冻结日历与门禁见[ops/dependencies.md](../ops/dependencies.md)。

### 版本发布：`.github/workflows/release.yml`

只由 `v<版本>` 形式的 tag 推送触发（tag 必须等于 `Cargo.toml` 的 workspace version）。流程：

1. `guard`：校验 tag 名与工作区版本一致；完整 checkout 后确认 tag 提交是 `origin/main` 的祖先；运行 `published-migrations`，任何已发布迁移被改写或删除都拒绝发布。
2. `verify`：在 tag 上重跑 `ci/checks/check.sh all fmt check dependency-versions lint wasm`、`just test`、PostgreSQL 18 `just test-db` 与 `just test-db-compat`，测试入口与 PostgreSQL 18 准备方式和 `ci.yml` 的 `test` 作业一致，不依赖分支保护的历史运行记录。
3. `build`：复用 `.github/workflows/release-build.yml`，与普通 CI 完全相同的发布构建、缓存与门禁，产出候选包。
4. `publish`：复核候选包 SHA-256，用 `actions/attest-build-provenance` 生成绑定本仓库、本工作流、tag 与构建提交的 SLSA 来源证明，再用部署端同一脚本自检，最后 `gh release create --verify-tag` 创建 GitHub Release 并附带 `*.tar.gz`、`SHA256SUMS`、`DEPENDENCIES.md`。

Release 资产永久保留；同一 tag 的发行一经创建不会覆盖（重复运行在 `gh release create` 前失败）。发布构建的候选 Actions artifact 仍只保留 1 天，正式资产只存在于 GitHub Release。

缓存：Rust 编译缓存按作业与工具链隔离，PR 不写缓存；发布产物与 `target/dx` 以源码身份指纹为键单独缓存；受控工具以配方（bootstrap 脚本、锁文件、补丁、工具链）为键缓存，命中后仍以安装回执逐字节校验。

发布构建开始前，使用与发布入口相同的受控 sccache 和 daemon 地址执行 `sccache rustc -vV`。缓存服务启动失败（例如 GitHub 缓存存储返回 `503 ServerBusy`）时，步骤发出 warning 并清空 `RUSTC_WRAPPER`，由 rustc 直接完成后续构建；实际编译错误与发布门禁失败仍使作业失败。受控工具回执校验在探测前执行，校验失败直接终止。sccache 0.18.0 的 `SCCACHE_IGNORE_SERVER_IO_ERROR` 不覆盖启动失败，因此不能替代此探测。

## 发布：`ci/release/release.sh`

```bash
bash ci/release/release.sh release-check [--single-binary] [--allow-dirty]   # 完整构建 + 门禁 + 装包
bash ci/release/release.sh package [--single-binary] [--allow-dirty]         # 只装配已构建制品
bash ci/release/release.sh build-api | build-embedded | verify-artifacts
bash ci/release/release.sh build-single | verify-single
bash ci/release/release.sh verify-web-size | source-fingerprint
bash ci/release/verify-deployment-package.sh <*.tar.gz> --tag <v版本>        # 部署前来源校验
```

对应 Justfile：`just build-api`、`just build-release`、`just build-single`、`just verify-artifacts`、`just web-size`、`just release-check`、`just package`、`just verify-package`。

### 制品变体

| 变体 | feature | 程序 | 用途 |
| --- | --- | --- | --- |
| API-only | （无） | Public、Admin | 不发布；证明未启用 `web-embed` 时二进制不含任何页面（资源目录固定为空，页面路由 503） |
| embedded | 两端 `web-embed` | Public、Admin、Worker、auction-cli | 分体部署发布包 |
| single | `auction-server/web-embed` | auction-server、auction-cli | 合并部署发布包（`-single` 后缀） |

### 门禁

- **页面 payload**：先用 `ci/release/web-bundle.sh` 构建两端 payload，校验入口页标题（“Auction 公开站点” / “Auction 管理站点”）、跨端隔离、只含普通文件与唯一一组 JS/WASM、payload 清单与哈希。目录遍历或摘要生成失败即拒绝复用，不把失败留下的部分输出当作有效清单。
- **体积预算**：两端 payload 的 WASM 与总量按 [ci/release/web-size-budget.v1](release/web-size-budget.v1) 的基线比较；明显增长只告警，超过硬上限使发布失败。见 [Web payload 体积预算](#web-payload-体积预算)。
- **二进制标记**：Public 只含 Public payload 的标记与哈希，Admin 只含 Admin 的，Worker 与 auction-cli 不含任何页面标记；API-only 二进制含“资源缺失”提示而不含 payload。
- **依赖边界**（`cargo tree`）：Worker 不依赖任何前端 crate；Public 不依赖 Admin 前端；Admin 不依赖 Public 前端；`auction-server` 不依赖 `auction-cli`。
- **BUILD-INFO 与校验和**：每个制品目录记录源码身份、构建输入与环境指纹、feature 和文件摘要；工具链经受控工具身份绑定。复用判定比较这些指纹、锁文件、feature 和制品摘要；仅仓库提交或不在构建输入范围内的文档变化，不必重新编译。
- **已发布迁移**：发布 tag 中存在的 `migrations/postgres/*.up.sql` 必须在当前工作树逐字节不变；修改、重命名或删除已发布文件使 `published-migrations` 与发布工作流失败，只允许追加新编号。
- **装包**：文件白名单与权限固定（程序 755、文档 644、配置示例 640、敏感示例 600），禁止符号链接、日志与真实敏感文件名；归档确定性生成（固定排序、mtime、owner），解包复核后输出 `SHA256SUMS` 与 `DEPENDENCIES.md`。正式发布要求干净工作树（`--allow-dirty` 只用于本地证据，包名带 `-dirty`）。

发布包名为 `auction-rust-<版本>-<提交前 12 位>[-single][-dirty].tar.gz`，内容见[ops/README.md#部署形态](../ops/README.md#部署形态)。仓库 Markdown 链接相对各文档目录；装包时按同一文件安装清单重定位全部已打包文档（含 `config/README.md`），保留标题锚点，并映射敏感模板的 `.toml.example` 文件名。已打包的目标使用包内相对链接；未打包的源码、其他变体模板和文档链接到本次源码提交的 GitHub 页面，需要联网浏览。

### 版本 tag 与部署制品

只有从 `main` 上的 `v<版本>` tag 产生的 GitHub Release 资产可以部署。tag 的工作区版本必须与 `Cargo.toml` 的 `[workspace.package].version` 完全一致：

```bash
git switch main && git pull --ff-only
git tag -a v0.1.0 -m "Auction 0.1.0"    # tag 名必须等于 v + 工作区版本
git push origin v0.1.0                  # 触发 auction-release
```

tag 推送后由 `auction-release` 完成守卫、完整门禁、构建、来源证明和 Release 发布；不要手工上传资产，也不要移动或强推已发布 tag。已发布迁移文件由 `just verify` 中的 `published-migrations` 保护：标签所包含的任何 `*.up.sql` 都不得再修改、重命名或删除。远端还应启用 tag 保护与 Release 不可变设置（GitHub 仓库设置，不由本仓库代码强制）；发布工作流只保证同一 tag 不会产生第二个 Release。

部署端从 GitHub Release 下载 `*.tar.gz`、`SHA256SUMS`、`DEPENDENCIES.md`，然后运行：

```bash
bash ci/release/verify-deployment-package.sh auction-rust-<版本>-<12位>.tar.gz --tag v<版本>
# 或
just verify-package auction-rust-<版本>-<12位>.tar.gz --tag v<版本>
```

脚本失败关闭地核对：外层与包内 `SHA256SUMS`、归档成员无路径穿越/链接、`RELEASE-MANIFEST.v1` 与 `DEPENDENCIES.md` 的源码提交一致、工作树为 `clean`、构建提交可从指定发布 tag 到达，并通过 `gh attestation verify` 绑定 `release.yml` 工作流、tag 与提交的来源证明。`--offline` 只跳过来源证明，输出明确警告，不能作为部署依据。

普通 CI 的 1 天候选 Actions artifact 只用于构建核对，没有 Release 资产身份与来源证明，不满足部署条件。`auction-cli deploy` 会复用同一校验；不含该命令的版本由部署人员手工执行该脚本。

### 构建 profile

| profile | 设置 |
| --- | --- |
| `release`（后端） | 优化级别 3、fat LTO、单代码生成单元、unwind、溢出检查开启 |
| `wasm-release`（前端） | 继承溢出检查，优化级别 z、panic 为 abort |
| `dev` | `debug = "line-tables-only"`（需要完整调试信息时设 `CARGO_PROFILE_DEV_DEBUG=2`） |

release 保持 `panic = "unwind"`，不启用增量或 `target-cpu=native`。
`overflow-checks = true` 使整数溢出失败关闭，避免静默回绕。
WASM 继承开启检查，保留 abort，由 `install_panic_hook` 显示错误横幅。
回退时只恢复 release 的该声明，并重建原生及 WASM 制品。
`just verify` 不构建 release；该 profile 变更只作静态核对，未作性能结论。

### Web payload：`ci/release/web-bundle.sh`

```bash
bash ci/release/web-bundle.sh public
bash ci/release/web-bundle.sh admin
```

以 `wasm-release` profile 运行受控 `dx bundle`，复制到稳定目录 `target/dx/build/<包名>/web/public`（后端 `web-embed` 从这里内嵌），写入 payload 清单与哈希。生成清单前依次完成两步改写：在入口 `index.html` 的脚本预加载之前声明带指纹的 WASM 预加载（`<link rel="preload" as="fetch" type="application/wasm" crossorigin>`），让 WASM 与加载器 JS 并行下载；再为 `assets/` 下可压缩资源生成 brotli 副本（`*.br`）。找不到唯一的 WASM 或唯一的脚本预加载锚点时构建失败，不产出未声明预加载的 payload。预压缩要求 PATH 中有 `brotli`（release 构建工作流已安装），版本写入 `WEB-BUILD-INFO` 参与复用判定。复用要求本 surface、共享层、契约、工具配方及构建环境指纹一致，且现有 payload 再次通过完整门禁；仅比较 `dx` 与 `rustc` 版本不足以证明可复用。

### Web payload 体积预算

```bash
bash ci/release/release.sh verify-web-size   # 只读既有 payload 与预算，不构建原生程序
just web-size                                # 同上
```

[ci/release/web-size-budget.v1](release/web-size-budget.v1) 是刻意维护的数据文件（`schema=auction-web-size-budget/1`），每个 surface 记录两对数字：

- `<surface>_wasm_baseline_bytes` / `<surface>_payload_baseline_bytes`：基线实测值。
- `<surface>_wasm_max_bytes` / `<surface>_payload_max_bytes`：硬上限。

当前数值只记录在该数据文件中。体积按原始表示统计：构建期 brotli 副本（`*.br`）不计入 `wasm`/`payload`，只影响实际传输量。`payload` 取 `target/dx/build/<包名>/web/public` 下全部普通文件（含内嵌标记 `auction-web-manifest.v1`）的字节合计，`wasm` 取其中唯一一组 `auction-web-<surface>_bg*.wasm`；测量前的 payload 必须先通过完整门禁，测量失败（目录缺失、摘要或体积无法计算）同样失败关闭。

判定：超过 `baseline × (1 + warn_growth_basis_points/10000)`（当前 `warn_growth_basis_points=500`，即 5%）只打印 `WARN`，发布继续；超过 `_max_bytes` 打印实测值、基线与上限并使命令失败。检查点在 `build-embedded`/`release-check`（Web payload 生成后、链接原生程序前）、`verify-artifacts`/`verify-single`（复用制品同样受限）与独立的 `verify-web-size`。该检查只读既有文件，不改变运行时行为，也不写入基线。

刷新基线必须刻意进行：先确认体积增长来自有意的前端改动，运行 `just web-build`（或 `just build-release`）得到新 payload，再运行 `just web-size` 抄录输出的 `wasm=` 与 `payload=` 实测字节，只更新对应 surface 的 baseline；按预期余量调整 `_max_bytes` 时不能低于 baseline。基线改动与引起增长的前端改动同批提交；预算失败应先排查体积回归，不得直接把上限抬到实测值以上来让发布通过。

### 构建身份：`ci/release/build-identity.sh`

`ci/release/release.sh` 与 `ci/release/web-bundle.sh` 共用：选择受控工具（`target/build-tools/bin` 优先、`NO_DOWNLOADS=1`）、计算源码身份指纹（已跟踪与未忽略的新文件、删除与 mode 变化）、计算前后端共享构建号、读取 BUILD-INFO、校验 Web payload，以及只读的 payload 体积测量与预算判定。未通过环境变量指定时，显式清空 Cargo 配置中的两类 rustc 包装器；环境变量指定的 `sccache` 替换为受控版本，其他显式包装器保留并计入构建环境指纹。

两个入口都在构建前把 `web_build_identity`（两个 Web surface 输入并集指纹的前 32 位）导出为 `AUCTION_BUILD_ID`。原生程序把它写进每个不可被共享缓存重放的 HTTP 响应的 `x-auction-build` 头与实时 WebSocket 首帧，浏览器把同一值编译进 payload 并与自身比较；不一致时只提示刷新，不自动重载。该值只随浏览器 payload 输入变化，后端专属部署不会误报。
