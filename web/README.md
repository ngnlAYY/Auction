# web — 浏览器应用工作区

Public 与 Admin 两个浏览器应用的 TypeScript 源码、构建配置与编码规则。两个应用都是纯客户端渲染的单页应用，用 SolidJS 2.0 编写，由 Vite 打包，产物由后端以 `web-embed` 内嵌分发。

本文是 `web/` 下全部代码的规则来源。业务规则、路由与数据契约仍以 [docs/contracts](../docs/contracts) 与 [docs/business-rules.md](../docs/business-rules.md) 为准。

## 目录

| 目录 | 导入名 | 职责 | 不得越过的边界 |
| --- | --- | --- | --- |
| `web/contracts` | `@auction/contracts/…` | 与后端 `auction-contracts` 一一对应的 DTO 类型与运行期 schema | 不依赖 Solid，不访问 DOM，不含页面逻辑 |
| `web/shared` | `@auction/web-shared/…` | 两端通用、无业务偏向的浏览器能力：响应式内核、传输、存储、定时、路由、i18n 装配与通用组件 | 不出现业务键名、业务路由与业务文案 |
| `web/public` | 包内相对路径 | Public 应用的入口、API 封装、页面、交互与译文装配 | 不导入 `web/admin`；跨包只用登记的别名 |
| `web/admin` | 包内相对路径 | Admin 应用的入口、API 封装、页面、交互与译文装配 | 不导入 `web/public`；跨包只用登记的别名 |
| `web/tooling` | 包内相对路径 | 构建配置、构建期类型声明与包边界检查脚本 | 不进入浏览器产物 |

- 包内用相对路径导入，跨包只能用[`web/tooling/boundary-rules.ts`](tooling/boundary-rules.ts) 里登记的别名；`web/contracts` 与 `web/shared` 互不反向依赖页面包。
- 每个包的源码都在自己的 `src/` 下。目录名按职责命名，不用 `common`、`helpers`、`shared`、`misc`、`utils`。
- 运行期依赖只有 `solid-js`、`@solidjs/web`、`valibot` 与 `@fluent/bundle`。新增依赖要先得到维护者同意。

## 命令

工具版本由仓库根的 `mise.toml` 固定，依赖由 `web/bun.lock` 固定。下表命令除注明“仓库根”外均在 `web/` 下执行。门禁使用已安装的 `web/node_modules`，自身不安装依赖、不联网；首次或锁文件变化后先在仓库根运行 `mise run web-prepare`。该任务安装成功后写入 `web/node_modules/.auction-prepared` 凭据，发布构建只接受凭据相符的依赖；直接运行 `bun install` 不会写凭据，随后的构建会被拒绝。

| 命令 | 作用 |
| --- | --- |
| `mise run web-prepare`（仓库根） | 按锁文件执行 `bun install --frozen-lockfile` 并写入依赖准备凭据，是唯一会安装 Web 依赖的入口 |
| `mise run web-check`（仓库根） | 依次运行 `bun run typecheck`、`bun run lint` 与包边界检查，已包含在 `mise run verify` 中 |
| `mise run web-test`（仓库根） | 在 `web/` 下运行 `bun run test`，不并入默认门禁；CI 的 `test` 作业运行它 |
| `bun run typecheck` | 检查四个包、Vite 构建配置和测试。`typecheck:contracts`、`typecheck:shared`、`typecheck:public`、`typecheck:admin` 只查单个包 |
| `bun run lint` | Biome 的格式与静态规则检查，不写文件 |
| `bun run test` | 单元测试（`bun test`）；测试文件与被测模块同目录，以 `.test.ts` 结尾 |
| `bun run format` | 按 Biome 规则改写格式并应用安全修复 |
| `bun run build:public`、`bun run build:admin` | 生产构建，产物写入 `target/web/<surface>/public` |
| `bun run dev:public` | 启动 Public 开发服务器；`/favicon.ico` 由仅对 `serve` 生效的插件映射到共享 SVG |

包边界由 `web/tooling/check-boundaries.ts` 用 TypeScript 编译器解析语法树检查，规则见上文目录表：`contracts` 不导入其他三个包，`shared` 只可导入 `contracts` 与自身，`public`/`admin` 互不导入且只能再导入 `contracts` 与 `shared`，四个包都不导入 `web/tooling/**`、`web/` 里四个包之外的源码，以及仓库里 `web/` 之外的源码。说明符只接受闭集：相对路径（`./`、`../` 开头，或表示目录入口的 `.`、`..`）（含反斜杠的路径违规）、[`web/tooling/boundary-rules.ts`](tooling/boundary-rules.ts) 中登记的 `@auction/contracts/*` 与 `@auction/web-shared/*` 两个别名、`web/package.json` 的 `dependencies`/`devDependencies` 中登记的第三方包（含子路径），以及测试文件里的 `node:`/`bun:` 内置模块。跨包只能用登记的别名，相对路径跨包同样违规；别名只允许登记在 `web/tsconfig.base.json`，其他 tsconfig 出现 `paths`（包括空对象）或基配置的别名表与检查器不一致时，检查器以退出码 2 失败；`web/package.json` 的 `imports` 只用于核对。

资产按归属放行：`crates/auction-web-shared/{assets,style}` 四个包都可引用，`crates/auction-web-public/{assets,i18n}` 只给 `public`，`crates/auction-web-admin/{assets,i18n}` 只给 `admin`。普通 `.css` 可直接引用，其他资产须带 `?raw` 或 `?url`；查询参数不改变目标路径的归属。受检源码目录不允许符号链接；允许目标在磁盘上存在时，其真实路径必须等于规范化后的词法路径，经过符号链接即违规。动态 `import()` 只接受字符串字面量或无插值模板字面量，带插值或非字面量参数报错；只有[`web/tooling/boundary-rules.ts`](tooling/boundary-rules.ts) 的 `VIRTUAL_MODULE_TEST_FILES` 点名的测试夹具里的非字面量 `import()` 例外（`${NAMESPACE}:…` 虚拟模块；新增同类夹具要加进这张清单），其他文件与所有非字面量 `require()` 都报错；括号包裹的 `require` 仍按 `require` 识别。检查器退出码 1 表示发现越界，2 表示工具、配置、读取或解析失败；纯规则测试用 `bun test tooling/check-boundaries.test.ts`（在 `web/` 下），回归用例用仓库根的 `bash ci/checks/web-boundaries-test.sh` 运行。

除 `test` 外，脚本用 bun 直接执行 `node_modules/.bin/` 下的 `tsc`、`biome` 与 `vite`；本地工具缺失时立即以“找不到模块”失败，不经 `bun x` 临时解析或下载，也不额外要求 Node。`bunfig.toml` 的 `install.auto = "disable"` 关闭自动安装，工具或其传递依赖缺失时都不会联网补装。

生产构建的输入只有仓库源码、样式与译文，根 `Cargo.toml` 的 `workspace.package.version`，以及调用方显式传入的 `AUCTION_BUILD_ID`（取值合法时作为构建号，否则退回工作区版本）。构建不读取 `.env*`：`bunfig.toml` 的 `env = false` 同时关闭外层 `bun run` 与脚本内层 Bun 的自动加载，Vite 配置再以 `envDir: false` 关闭 Vite 自己的加载（Vite 8 中 `envFile: false` 已弃用）；环境文件里的 `AUCTION_BUILD_ID` 与 `VITE_` 变量都不会进入产物。发布入口固定以 `NODE_ENV=production` 调用 Vite，外部同名变量不能改变 `import.meta.env.DEV`。

发布构建经 `ci/release/web-bundle.sh` 选择 Solid 时，会核对 `mise run web-prepare` 留下的 `web/node_modules/.auction-prepared` 凭据（锁文件摘要与 bun 版本），再直接调用本地 `web/node_modules/.bin/vite` 构建到临时目录，校验通过后换入 `target/web/<surface>/public`；该阶段不联网，也不自动安装依赖。首次、锁文件变化或 bun 版本变化后先运行 `mise run web-prepare`。`AUCTION_WEB_STACK=solid` 当前只切换 Public，Admin 仍是 Rust 版；发布入口除 `verify-web-size` 外仍会拒绝 Solid。

生产构建校验最终 HTML 恰有一个模块入口脚本，且 `data-cfasync="false"` 位于 `src` 之前。Public 入口以 `?url` 引入[共享 SVG](../crates/auction-web-shared/assets/favicon.svg)，产物因此有唯一的 `assets/favicon-<指纹>.svg`；入口 HTML 的图标链接固定为 `/favicon.ico`，生产由后端映射到该文件，开发服务器则由仅对 `serve` 生效的插件提供同样的映射。发布校验要求入口恰好有一个该图标链接、`assets/` 恰好有一个带指纹的 `favicon-*.svg`，两者缺一即失败。

## 文件与命名

- 文件与目录一律小写连字符（`browser-lock.ts`）。有子模块的模块以 `name/index.ts` 为入口，叶子模块是单个 `name.ts`，两者不并存。含 JSX 的文件用 `.tsx`。
- 源码导入省略扩展名与 `/index`。Vite 入口配置导入共用配置时显式写 `.ts`，供 Vite 原生加载器解析。
- 每个源码文件以 `//` 注释开头说明模块职责；导出的类型、函数与组件用 `/** … */` 说明。注释与文档用中文，只写本地的不变量与理由，不复述代码。
- 类型、接口与组件用 `PascalCase`；函数、变量与非 DTO 字段用 `camelCase`；模块级常量用 `SCREAMING_SNAKE_CASE`。DTO 字段与线上 JSON 完全一致，保持 `snake_case`。
- 单个文件不超过 600 行；超过时按职责拆成子模块（`name.ts` 变成 `name/index.ts` 加若干子模块，对外导入路径不变）。不靠删除空行、合并语句或压缩注释来凑行数；接近上限就应当拆分。
- 只用具名导出与具名导入，不写 `export default`（Vite 配置文件除外），不写 `export *`。类型导入用 `import type`。

## TypeScript 规则

- 不用 `any`、非空断言 `!`、`@ts-ignore` 与 `@ts-expect-error`。类型断言 `as` 只用于已经由运行期检查证明的收窄，并在同一处写明依据。
- 不用 `enum` 与 `namespace`。闭集取值用字符串字面量联合：`type Theme = "dark" | "light";`。带数据的多种形态用带 `kind` 字段的可辨识联合。
- `switch` 覆盖联合类型时，`default` 分支调用 `unreachable(value)`（`@auction/web-shared/result`），漏掉成员时编译失败。
- “没有值”统一用 `null`。`undefined` 只表示“尚未加载”（`createTaskResource` 的结果）与组件的可选 props。
- 预期内的失败作为值返回：`Result<T, E>`、`ok(value)`、`err(error)`（`@auction/web-shared/result`）。不用异常表达网络失败、解码失败或校验失败；`try`/`catch` 只包住会抛异常的浏览器接口，`catch` 分支要么处理、要么转成 `Result`；确实可以忽略的失败（例如移除监听、释放观察器）在分支里用注释写明忽略的理由，不留没有说明的空分支。
- 整数金额、版本与序号是 `number`，取值必须是安全整数，由解码器保证。UUID 是 `string`，新 `command_id` 用 `@auction/web-shared/platform/uuid` 的 `createUuidV4()` 生成。明文环境缺少 `crypto.randomUUID()` 时使用 `getRandomValues`。
- 文本长度上限若在后端按 UTF-8 字节计，这里也按字节计：`utf8ByteLength(text)`（`@auction/web-shared/text`），不用 `text.length` 代替。
- 字符串的大小比较与排序用 `compareText(a, b)`（同一模块，按 Unicode 码点）。JS 的 `<`、`>` 与默认 `sort()` 按 UTF-16 码元比较，对 U+10000 以上的字符与后端顺序相反；相等判断仍用 `===`。
- 解析 JSON 文本统一用 `parseJson(text)`（`@auction/web-shared/net/wire`），不直接调用 `JSON.parse`：后者会接受超出范围的数与孤立代理项，后端的解析器会拒绝。
- 秘密值（登录 key、Cookie、口令、CSRF token、完整装备链接）不写入 `console`、错误信息、`localStorage` 与 URL。

## 响应式内核

`@auction/web-shared/reactive/*` 是全部状态代码的地基，页面代码不直接调用 `createSignal`。

| 接口 | 用途 |
| --- | --- |
| `createCell(initial)` | 本地状态。`cell()` 响应式读取，`cell.peek()` 不订阅并立即看到最近一次写入，`cell.set(value)` 写入，`cell.update(fn)` 以当前值计算新值 |
| `createValueMemo(compute)` | 按结构相等去重的派生值；结果与上次按值相等时不通知下游 |
| `createTaskScope()` | 与组件同生命周期的任务集合；`scope.spawn(async (context) => …)` |
| `createTaskResource(dependencies, task)` | 随依赖重跑的异步读取，`resource()` 取结果，`resource.state()` 取阶段。`restart()` 与本轮依赖变化合并，另有 `cancel()`、`clear()` |
| `structuralEquals(a, b)` | 按值比较 JSON 形状的数据 |

必须遵守的语义：

1. **组件函数体只执行一次。** 函数体里读取 `cell()` 或 `props.x` 得到的是一次性的值，之后不会更新。凡是依赖状态或 props 的值，都写成 `createValueMemo(() => …)` 或箭头函数，并在 JSX、另一个 memo 或 effect 的 compute 里调用。
2. **不解构 props。** 写 `props.lot.name`，不写 `const { lot } = props`。调用组件时传值不传访问器：`<LotRow lot={lot()} />`。
3. **写入始终通知。** `cell.set` 即使新旧值相等也会让订阅者重跑；需要“相等则不传播”时，让下游读取一个 `createValueMemo`。
4. **effect 分两段。** `createEffect(compute, apply)`：`compute` 只读取依赖并返回它们，`apply` 收到返回值后执行副作用（写状态、操作 DOM、发请求）。`apply` 里读取其他状态用 `peek()`。`apply` 可以返回清理函数。
5. **await 之后先检查存活。** 任务体每次 `await` 之后、写状态之前执行 `if (!context.alive()) return;`。请求把 `context.signal` 传给传输层。`createTaskResource` 已经替被取代的任务丢弃结果，任务体本身仍不得在 `await` 之后直接写其他状态。
6. **不使用 Solid 的异步派生与过渡机制。** 不写返回 Promise 的 `createMemo`，不用 `<Loading>`、`<Errored>`、`action`、`createOptimistic`、`isPending`、`refresh`。加载中、失败与结果未知都是显式状态。
7. **状态对象不可变更新。** `cell.update((current) => ({ ...current, page: 2 }))`，不原地修改 `cell.peek()` 返回的对象或数组。
8. **清理。** 组件卸载时要释放的资源（事件监听、定时器、观察器）在创建处用 `onCleanup` 登记；需要等首次渲染完成后再做的事（测量、聚焦）放进 `onSettled`，并从中返回清理函数。

## 组件

```tsx
// 黑名单页面：分页读取并展示被封禁的用户。

import type { JSX } from "@solidjs/web";
import { createEffect, For, Match, Show, Switch } from "solid-js";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";

/** 黑名单表格的一行。 */
function BlacklistRow(props: {
  entry: PublicBlacklistEntryResponse;
  selected: boolean;
  onSelect: (userName: string) => void;
}): JSX.Element {
  return (
    <tr
      class={["public-blacklist__row", { "public-blacklist__row--selected": props.selected }]}
      onClick={() => props.onSelect(props.entry.user_name)}
    >
      <td>{props.entry.user_name}</td>
      <td>{props.entry.reason}</td>
    </tr>
  );
}

/** 黑名单页面。 */
export function Blacklist(): JSX.Element {
  const pageNumber = createCell(1);
  const selected = createCell<string | null>(null);
  const blacklist = createTaskResource(
    () => pageNumber(),
    (requested, context) => fetchBlacklist(requested, context.signal),
  );
  const title = createValueMemo(() => t("blacklist-title", { page: pageNumber() }));

  // 换页后清除选中行，避免沿用上一页的选择。
  createEffect(
    () => pageNumber(),
    () => {
      selected.set(null);
    },
  );

  return (
    <section class="public-blacklist" aria-busy={blacklist.state() === "pending" ? "true" : "false"}>
      <h1>{title()}</h1>
      <Switch fallback={<p role="status">{t("loading")}</p>}>
        <Match when={blacklist()?.ok === false}>
          <p role="alert">{t("load-failed")}</p>
        </Match>
        <Match when={blacklist()}>
          {(loaded) => (
            <table>
              <tbody>
                <For each={entriesOf(loaded())} keyed={(entry) => entry.user_name}>
                  {(entry) => (
                    <BlacklistRow
                      entry={entry()}
                      selected={selected() === entry().user_name}
                      onSelect={(userName) => selected.set(userName)}
                    />
                  )}
                </For>
              </tbody>
            </table>
          )}
        </Match>
      </Switch>
    </section>
  );
}
```

- 条件渲染用 `<Show>`、`<Switch>`/`<Match>`，列表用 `<For>`；不在组件函数体里用 `if` 提前返回不同的 JSX。`<Show>` 与 `<Match>` 的函数子节点收到的是访问器，要调用：`{(value) => <p>{value()}</p>}`。
- 列表项有稳定标识时写 `keyed={(item) => item.id}`，回调收到的是访问器 `item()`；没有稳定标识时写 `keyed={false}`，按下标复用节点。不使用默认的按对象引用配对，否则每次重新取数都会重建整张表。
- `class` 用字符串、数组或对象形式组合：`class={["a", { "a--active": active() }]}`。不拼接字符串。
- 属性名用小写的 HTML 原名（`tabindex`、`for`、`maxlength`、`colspan`）。`disabled`、`hidden`、`checked` 等布尔属性传布尔值；`aria-*` 与 `data-*` 属性传字符串（`aria-busy={busy() ? "true" : "false"}`），不存在时传 `undefined`。
- 事件处理器写成稳定的函数：`onClick={(event) => …}`。处理器表达式只在挂载时求值一次，需要按状态选择行为时把判断写进函数体。
- 元素引用用 `ref`：`let input: HTMLInputElement | undefined;` 加 `<input ref={input} />`，或回调形式 `ref={(element) => …}`。`ref` 回调执行时元素尚未挂到文档，量尺寸与聚焦放进 `onSettled`。
- 受控输入用 `value={draft()}` 加 `onInput={(event) => draft.set(event.currentTarget.value)}`。
- 上下文用 `createContext<T>()` 定义并与类型放在同一模块；提供方直接把上下文当组件用：`<SessionContext value={session}>…</SessionContext>`，使用方 `useContext(SessionContext)`。
- 新写的浮层挂到 `document.body` 或所属弹窗时用 `Portal`（`@solidjs/web`）。从现有实现迁移来的、靠移动已渲染节点实现的浮层保持原来的命令式做法，不改用 `Portal`，以免多出一层容器改变 DOM 结构。
- 界面文案全部走 `t("message-id", { name: value })`，不写字面量文案。

## 契约

`web/contracts/src` 的目录结构与 `crates/auction-contracts/src` 相同。两边的字段名、可空性、默认值与取值集合必须保持一致；后端 DTO 变化时在同一个提交里同步。

- 浏览器接收的 DTO：导出 `XxxSchema`（valibot）与 `type Xxx = v.InferOutput<typeof XxxSchema>`。
- 浏览器发出的 DTO：只导出 `interface`。后端带 `skip_serializing_if` 的字段写成可选属性并在值缺失时省略，其余可空字段写成 `T | null` 并原样发送 `null`。
- 结构体用 `@auction/contracts/wire` 的 `struct({...})`；后端带 `deny_unknown_fields` 时用 `strictStruct({...})`。不直接用 `v.object`、`v.strictObject`：它们会把数组当成对象解码。
- `flatten`：把被平铺结构体的字段表单独导出为 `XxxEntries`，在外层写 `struct({ ...XxxEntries, 其余字段 })`。`untagged` 枚举用 `v.union` 并保持声明顺序。

字段原语在 `@auction/contracts/wire`，按后端属性选择：

| 后端字段 | 原语 | 读到的类型 |
| --- | --- | --- |
| `i64`、`isize` | `integer` | `number` |
| 只用于展示的非负 `i64` 纳秒时长 | `displayDurationNanos` | `number` |
| `u64`、`usize` | `unsignedInteger` | `number` |
| `i32`、`u32`、`i16`、`u16`、`u8` | `int32`、`uint32`、`int16`、`uint16`、`uint8` | `number` |
| `f64` | `float` | `number` |
| 普通 `Option<T>` | `optionalNullable(T)` | `T \| null` |
| `deserialize_required_option`（键必须出现） | `requiredNullable(T)` | `T \| null` |
| `default` + `deserialize_optional_field`（可缺省、不可为 `null`） | `optionalField(T)` | `T \| null` |
| `default`（缺省取默认值） | `withDefault(T, value)` | `T` |
| `deserialize_null_default` | `nullAsDefault(T, value)` | `T` |
| `BTreeMap<String, T>`、`HashMap<String, T>` | `stringMap(T)` | `Readonly<Record<string, T>>` |
| `serde_json::Value` | `jsonValue` | `unknown` |

整数默认必须在 JavaScript 安全整数范围内。唯一例外是只用于展示的非负 `i64` 纳秒时长，目前用于 Public 补偿逐件明细、补偿流水 DTO 的 `leader_held_nanos`。`displayDurationNanos` 接受有限的非负整数 `number`，上界为 `2^63`（`i64::MAX` 解析后会舍入到该值）。展示精度差异见下方差异表。金额、计数、版本、身份、分页、排序和其他判定继续使用各自的原语。

映射字段不用 `v.record`：它会接受数组，并丢弃 `__proto__`、`constructor` 等键名。

解码统一经 `decode(schema, data)`：返回 `{ ok: true, value }` 或 `{ ok: false }`，调用方把失败映射为 `invalid_response` 并上报 `dto_decode`。不用 `null` 表示失败，因为 `data: null` 对可空 schema 是合法的成功值。路由专属的闭集键与跨字段校验留在各应用的 `api/` 下，先校验原始 JSON，再用 schema 解码。

需要按字段校验原始词法时，通过 `response.json` 显式传入规则。Public 错误信封的 `code`、`message` 和 `data` 各只能出现一次，标记读取与写回执的 `version` 使用整数词法。

## 样式与译文

- 样式文件与类名沿用现有定义：共享设计变量与控制台外壳在 `auction-web-shared` 的 `style/`，各应用的规则在各自的 `assets/`。装配顺序固定为 base → surface → desktop → mobile → console。
- 类名用小写连字符与 BEM，并带 `public-` 或 `admin-` 前缀；颜色、字号、间距只用共享变量。
- 浏览器支持口径与 Rust 版相同，见[AGENTS.md#web](../AGENTS.md#web)：较新的特性先做特性检测并保留退路，不引入 polyfill。
- Solid 版专属的样式放在各应用的 `src/styles/`，在入口里排在共用样式之后导入，规则写在 `@layer` 之外（因此优先于共用样式），只用共享变量。新增这类差异时在下表登记，并在文件头或改动处写明与 Rust 版的差别。目前与 Rust 版有意不同的地方：

  | 位置 | 差别 |
  | --- | --- |
  | `web/contracts/src/wire/index.ts`、Public 补偿逐件明细 | 超过 `2^53` 纳秒（约 104 天）的展示时长在 Solid 版是近似值，整分钟前 1–2 纳秒的值可能舍入并显示为下一分钟。Rust 版按整数精确截断。 |
  | `web/public/src/styles/mobile-topbar.css` | 手机顶栏改为两行应用栏：第一行是当前场次与所在世界，第二行是状态标签与倒计时（放不下时截断）；不显示品牌名，语言与主题移到设置页，顶栏没有任何切换器；菜单与刷新是无边框图标 |
  | `web/public/src/styles/round-pages.css` | 出价记录与履约进度的场次区：场次下拉框就是场次标题；手机上去掉卡片，下拉框与搜索框共两行。历史成交价的主筛选行少了世界一列 |
  | `web/public/src/styles/mobile-catalog-filters.css` | 手机上看板筛选区排成带标签的三行（分组按钮、窄等级输入框与复选项、类型下拉框与列设置）；列设置面板改为整宽多列网格 |
  | `web/public/src/surface/query/world-scope.ts`、`surface/board/world.tsx`、`styles/world-tabs.css` | 世界切换是页面里的按钮组（`WorldTabs`，“世界 [全部世界][主世界][异世界]”），顶栏与页面里都没有世界下拉框；状态与写回逻辑在外壳的控制器里。按钮只列配置里开放的世界，只有一个可选时整组不显示；关闭的世界不作为选项，但它的历史在跨世界页仍可见。往期拍卖、出价记录、履约进度是跨世界页：有「全部世界」，`world` 缺省 = 全部（出价记录与履约进度在 Rust 版缺省是主世界）；看板与论坛同步只有各个世界，缺省 = 主世界。导航链接把显式选择的世界带到目标页。历史成交价是例外：世界是该页“筛选条件”面板里的一项搜索条件（默认全部世界，点「应用」生效），不参与导航继承，进入该页默认不发起搜索 |
  | `web/public/src/surface/pages/history.tsx` | 往期拍卖没有状态筛选，只列已结算；地址栏里的 `status` 读到后即移除 |
  | `web/public/src/surface/reads/bid-log/`、`reads/fulfillment/` | 场次下拉框并入场次标题（全部世界时按世界分组），页面上方不再有单独的控件行；出价记录的选项只显示期数，状态由旁边的标签给出 |
  | `web/public/src/surface/query/category.ts`、`presentation/desktop.tsx`、`presentation/mobile.tsx` | 装备分类多选：地址栏 `type` 与接口 `category` 是逗号分隔的分类键列表；桌面是可多选的开关按钮，手机是多选下拉面板，面板里的勾选是草稿、关闭面板时一次生效（Rust 版单选） |
  | `web/shared/src/router/outlet.tsx`、`ui/bootstrap/page-load-recovery.ts` | 目标页面的代码尚未加载时立即卸载上一个页面。页面分块加载失败时，用当前标签页在 `sessionStorage` 保留的最近 8 条路径与时间判断是否重载，路径不含查询与片段。30 秒内已重载过同一路径或存储不可用时显示错误横幅。Rust 版是单个 WASM 包，无需分块恢复 |
  | `web/public/src/surface/settings.tsx`、`styles/settings-page.css` | 设置页 `/settings`（语言与主题）：Rust 版没有这个页面；入口只在手机导航里，桌面顶栏仍保留两个切换器 |
- 译文是各应用 `i18n/` 下的五个 `.ftl` 文件（en、zh-CN、zh-TW、ja、ko），键集合一致，键名用连字符。译文只使用 Fluent 的平铺子集：每条消息占一行，写成 `键 = 文本`，文本里的占位只有 `{ $变量 }` 与字符串字面量 `{"…"}` 两种；加载器遇到子集之外的写法会拒绝整份译文。该子集由 `i18n` 门禁的 [`i18n-flat-subset.pl`](../ci/checks/i18n-flat-subset.pl) 静态核对，扩展加载器语法时两边一起改。Public 默认 en，Admin 默认 zh-CN；偏好键 `hv.lang` 与 `hv.theme` 是固定契约。

## Public 竞价命令恢复

直接出价、创建代理和降低代理上限沿用 [Rust Public 的竞价命令规则](../crates/auction-web-public/DESIGN.md#竞价代理与标记)。表单只持有未发送的确认草稿。外壳按权威会话确认的用户在内存中持有已发送的请求、冻结目标和回执，同一拍品同时最多有一条未结记录。切换路由或进入登录页不取消请求。同一账号复核期间暂停重试，确认退出或换号时销毁原记录与在途任务。

外壳提示区和拍品弹窗只能原样重试未知命令，或由玩家二次确认后放弃本地记录。刷新或关闭标签页会清除记录。“查看拍品”按冻结的世界及场次定位。命令答复触发权威重读，价格、领先关系和代理状态均由读取投影更新。

已读取的本人代理授权按场次和拍品保存在同一身份的外壳中。目录或场次暂时读不到时，同一账号回到原目录仍沿用已有的授权判定。确认匿名、退出或换号时销毁旧账号的快照。

## 禁止事项

- 不引入服务端渲染、hydration、虚拟 DOM 库或其他 UI 框架。
- 不在浏览器重建业务状态机：金额、资格、幂等与状态迁移以服务端结果为准；改变状态的请求携带 `command_id`，结果未知时只允许原样重试。
- 不用 `innerHTML`、`eval`、`new Function` 与内联脚本；内容安全策略只放行同源脚本。
- `fetch`、`localStorage`、`sessionStorage` 与 `setTimeout` 统一经 `@auction/web-shared` 的传输、存储与定时模块。共享层内部直接 `fetch` 的两处例外是译文文件的按需加载（3 秒超时）与浏览器错误信标。
