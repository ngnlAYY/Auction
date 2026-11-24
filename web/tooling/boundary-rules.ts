// 判定 web/ 四个包的模块说明符是否越过包边界。这里只做纯函数判定，不读取文件或启动
// TypeScript 解析器，便于在受限环境里用 bun test 覆盖闭集；解析器相关的扫描在
// check-boundaries-impl.ts。
//
// 说明符只接受四类形态：以 ./ 或 ../ 开头的相对路径、下面写死的包别名、web/package.json
// 登记的第三方依赖，以及测试文件里的 node:/bun: 内置模块。其余形态一律违规；反斜杠、
// 裸的 . / .. 也不在闭集内。需要新形态时由人扩展这张闭集，不在运行时猜测 tsconfig 的
// 映射规则。

import { dirname, extname, relative, resolve, sep } from "node:path";

/** web/ 下与四个浏览器 crate 对应的包。 */
export const PACKAGES = ["contracts", "shared", "public", "admin"] as const;

export type PackageName = (typeof PACKAGES)[number];

export interface RegisteredAlias {
  readonly pattern: string;
  readonly target: string;
}

// 只登记现有代码实际使用的两个包名。配置里的 paths/imports 只用于核对，不参与解析；
// 新增别名必须同时修改这里、tsconfig 与自测。
export const REGISTERED_ALIASES: readonly RegisteredAlias[] = [
  { pattern: "@auction/contracts/*", target: "contracts/src/*" },
  { pattern: "@auction/web-shared/*", target: "shared/src/*" },
];

// 这几个测试文件用 Bun 插件加载运行期虚拟模块，模块名由 ${NAMESPACE}:… 拼接而成。
// 只点名放行它们的非字面量 import()；新增同类夹具必须把完整路径加进这张清单。
export const VIRTUAL_MODULE_TEST_FILES: ReadonlySet<string> = new Set([
  "web/public/src/surface/shell/navigation-fixture.test.ts",
  "web/public/src/surface/catalog/command-scope-fixture.test.ts",
  "web/public/src/surface/consignment/render-fixture.test.ts",
  "web/public/src/surface/reads/bid-log/page-tree-fixture.test.ts",
  "web/public/src/surface/consignment/private-fixture.test.ts",
  "web/admin/src/solid-render-fixture.test.ts",
]);

export interface SpecifierInput {
  readonly sourceFile: string;
  readonly specifier: string;
  readonly repositoryRoot: string;
  readonly dependencies: readonly string[];
}

export type SpecifierDecision =
  | { readonly kind: "ok" }
  | {
      readonly kind: "violation";
      readonly reason: string;
    };

const ASSET_EXTENSIONS = new Set([
  ".css",
  ".ftl",
  ".svg",
  ".ico",
  ".png",
  ".jpg",
  ".jpeg",
  ".webp",
  ".gif",
  ".woff",
  ".woff2",
  ".ttf",
  ".otf",
]);

function normalizeRepositoryPath(path: string, repositoryRoot: string): string {
  return relative(resolve(repositoryRoot), resolve(repositoryRoot, path)).split(sep).join("/");
}

/** 判断源文件属于哪个包；不在四个包的 src 下时返回 undefined。 */
export function packageOfPath(path: string, repositoryRoot: string): PackageName | undefined {
  const normalized = normalizeRepositoryPath(path, repositoryRoot);
  for (const packageName of PACKAGES) {
    const root = `web/${packageName}/src`;
    if (normalized === root || normalized.startsWith(`${root}/`)) {
      return packageName;
    }
  }
  return undefined;
}

/** 测试文件按现有命名约定识别，内置模块只在这里放行。 */
export function isTestFile(path: string, repositoryRoot: string): boolean {
  return /\.test\.tsx?$/.test(normalizeRepositoryPath(path, repositoryRoot));
}

/** 测试虚拟模块例外按仓库相对路径点名，不能按目录或后缀泛化。 */
export function isNamedVirtualModuleFile(path: string, repositoryRoot: string): boolean {
  return VIRTUAL_MODULE_TEST_FILES.has(normalizeRepositoryPath(path, repositoryRoot));
}

function isInside(path: string, directory: string): boolean {
  return path === directory || path.startsWith(`${directory}${sep}`);
}

function boundaryAllows(from: PackageName, to: PackageName): boolean {
  if (from === to) {
    return true;
  }
  if (from === "shared" && to === "contracts") {
    return true;
  }
  return (from === "public" || from === "admin") && (to === "contracts" || to === "shared");
}

function hasAssetQuery(specifier: string): boolean {
  const queryIndex = specifier.indexOf("?");
  if (queryIndex < 0) {
    return false;
  }
  return /(?:^|[&;])(?:raw|url)(?:[&;]|$)/.test(specifier.slice(queryIndex + 1));
}

function assetAllowed(
  from: PackageName,
  target: string,
  specifier: string,
  repositoryRoot: string,
): boolean {
  const extension = extname(target).toLowerCase();
  if (!ASSET_EXTENSIONS.has(extension)) {
    return false;
  }
  if (extension !== ".css" && !hasAssetQuery(specifier)) {
    return false;
  }
  const path = normalizeRepositoryPath(target, repositoryRoot);
  if (/^crates\/auction-web-shared\/(?:assets|style)\//.test(path)) {
    return true;
  }
  if (/^crates\/auction-web-public\/(?:assets|i18n)\//.test(path)) {
    return from === "public";
  }
  if (/^crates\/auction-web-admin\/(?:assets|i18n)\//.test(path)) {
    return from === "admin";
  }
  return false;
}

function registeredAliasTarget(specifier: string, repositoryRoot: string): string | undefined {
  for (const alias of REGISTERED_ALIASES) {
    const prefix = alias.pattern.slice(0, -1);
    if (specifier.startsWith(prefix)) {
      const suffix = specifier.slice(prefix.length);
      if (
        suffix.split("/").some((segment) => segment === "" || segment === "." || segment === "..")
      ) {
        return undefined;
      }
      return resolve(resolve(repositoryRoot, "web"), alias.target.replace("*", suffix));
    }
  }
  return undefined;
}

/**
 * 返回相对路径或登记别名指向的词法目标；第三方包、内置模块与非法形态返回 undefined。
 * 解析器侧用它检查目标真实路径，纯判定不读取磁盘。
 */
export function resolveSpecifierTarget(
  input: Pick<SpecifierInput, "sourceFile" | "specifier" | "repositoryRoot">,
): string | undefined {
  if (input.specifier.includes("\\")) {
    return undefined;
  }
  const repositoryRoot = resolve(input.repositoryRoot);
  const sourceFile = resolve(repositoryRoot, input.sourceFile);
  const queryIndex = input.specifier.indexOf("?");
  const pathPart = queryIndex < 0 ? input.specifier : input.specifier.slice(0, queryIndex);
  if (pathPart.startsWith("./") || pathPart.startsWith("../")) {
    return resolve(dirname(sourceFile), pathPart);
  }
  if (pathPart.startsWith("@auction/")) {
    return registeredAliasTarget(pathPart, repositoryRoot);
  }
  return undefined;
}

function dependencyName(pathPart: string): string {
  const segments = pathPart.split("/");
  return pathPart.startsWith("@") ? segments.slice(0, 2).join("/") : (segments[0] ?? "");
}

function targetDecision(
  sourcePackage: PackageName,
  target: string,
  specifier: string,
  relativeSpecifier: boolean,
  repositoryRoot: string,
): SpecifierDecision {
  const webRoot = resolve(repositoryRoot, "web");
  const targetPackage = packageOfPath(target, repositoryRoot);
  if (targetPackage !== undefined) {
    if (targetPackage === sourcePackage) {
      return { kind: "ok" };
    }
    if (relativeSpecifier) {
      return {
        kind: "violation",
        reason: `跨包导入必须使用登记的导入名，不能使用相对路径：${specifier}`,
      };
    }
    if (!boundaryAllows(sourcePackage, targetPackage)) {
      return {
        kind: "violation",
        reason: `不允许从 ${sourcePackage} 导入 ${targetPackage}：${specifier}`,
      };
    }
    return { kind: "ok" };
  }
  if (isInside(target, resolve(webRoot, "tooling"))) {
    return { kind: "violation", reason: `不允许导入 web/tooling 的源码：${specifier}` };
  }
  if (isInside(target, webRoot)) {
    return { kind: "violation", reason: `只允许导入四个包的 src 源码：${specifier}` };
  }
  if (
    isInside(target, resolve(repositoryRoot)) &&
    assetAllowed(sourcePackage, target, specifier, repositoryRoot)
  ) {
    return { kind: "ok" };
  }
  return { kind: "violation", reason: `不允许导入 web/ 之外的源码或资产：${specifier}` };
}

/** 判定一个模块说明符；返回值只区分放行与越界，配置错误由调用方另行处理。 */
export function classifySpecifier(input: SpecifierInput): SpecifierDecision {
  const repositoryRoot = resolve(input.repositoryRoot);
  const sourceFile = resolve(repositoryRoot, input.sourceFile);
  const sourcePackage = packageOfPath(sourceFile, repositoryRoot);
  if (sourcePackage === undefined || !isInside(sourceFile, resolve(repositoryRoot, "web"))) {
    return { kind: "violation", reason: `源文件不在 web/ 的四个包中：${input.sourceFile}` };
  }
  if (input.specifier === "") {
    return { kind: "violation", reason: "模块说明符为空" };
  }
  if (input.specifier.includes("\\")) {
    return { kind: "violation", reason: `说明符含反斜杠：${input.specifier}` };
  }

  const queryIndex = input.specifier.indexOf("?");
  const pathPart = queryIndex < 0 ? input.specifier : input.specifier.slice(0, queryIndex);
  if (pathPart === "") {
    return { kind: "violation", reason: "模块说明符为空" };
  }
  if (pathPart.startsWith("#")) {
    return { kind: "violation", reason: `不允许使用包内导入：${pathPart}` };
  }
  if (pathPart === "bun" || pathPart.startsWith("node:") || pathPart.startsWith("bun:")) {
    if (!isTestFile(sourceFile, repositoryRoot)) {
      return {
        kind: "violation",
        reason: `运行时内置模块只能在测试文件中使用：${pathPart}`,
      };
    }
    return { kind: "ok" };
  }
  if (pathPart.startsWith("/") || /^[A-Za-z][A-Za-z0-9+.-]*:/.test(pathPart)) {
    return {
      kind: "violation",
      reason: `模块说明符不能是绝对路径或 URL：${pathPart}`,
    };
  }

  // 裸的 `.` 与 `..` 是目录入口的相对导入（现有源码在用），与 `./`、`../` 同样按目标路径判定。
  const relativeSpecifier =
    pathPart === "." ||
    pathPart === ".." ||
    pathPart.startsWith("./") ||
    pathPart.startsWith("../");
  if (relativeSpecifier) {
    return targetDecision(
      sourcePackage,
      resolve(dirname(sourceFile), pathPart),
      input.specifier,
      true,
      repositoryRoot,
    );
  }

  if (pathPart.startsWith("@auction/")) {
    const target = registeredAliasTarget(pathPart, repositoryRoot);
    if (target === undefined) {
      return { kind: "violation", reason: `未登记的 @auction 包名：${pathPart}` };
    }
    return targetDecision(sourcePackage, target, input.specifier, false, repositoryRoot);
  }

  if (pathPart.startsWith("@")) {
    const segments = pathPart.split("/");
    const scope = segments[0] ?? "";
    const packageSegment = segments[1] ?? "";
    if (
      scope.length < 2 ||
      !scope.startsWith("@") ||
      packageSegment === "" ||
      packageSegment === "." ||
      packageSegment === ".."
    ) {
      return { kind: "violation", reason: `第三方包名无效：${pathPart}` };
    }
  }
  const dependency = dependencyName(pathPart);
  if (input.dependencies.includes(dependency)) {
    if (queryIndex >= 0) {
      return { kind: "violation", reason: `第三方裸包导入不能附带 query：${input.specifier}` };
    }
    const segments = pathPart.split("/").slice(dependency.split("/").length);
    if (segments.some((segment) => segment === "" || segment === "." || segment === "..")) {
      return {
        kind: "violation",
        reason: `第三方包子路径不能包含空段、. 或 ..：${pathPart}`,
      };
    }
    return { kind: "ok" };
  }
  return { kind: "violation", reason: `未登记的裸包名：${pathPart}` };
}
