// 检查 web/{contracts,shared,public,admin}/src 的模块边界。TypeScript 解析器只负责给出
// 源码语法树与已解码的模块说明符；判定规则在 boundary-rules.ts 的纯函数里。
//
// 说明符的合法形态是闭集：以 ./ 或 ../ 开头的相对路径、写死的两个 @auction 别名、
// web/package.json 登记的第三方依赖，以及测试文件里的 node:/bun: 内置模块。别名只能
// 登记在 web/tsconfig.base.json；配置只用于核对写死的别名表，不参与解析，出现多余条目、
// 多个候选、条件映射或子配置 paths 时以退出码 2 失败。子配置经 extends（含数组与省略
// .json 的相对路径）引入的文件同样逐层核对；包名 extends 不在闭集内。受检源码不允许
// 符号链接，允许目标的真实路径也必须等于词法路径。
//
// 退出码：1 表示发现越界导入；2 表示工具、配置、读取或解析失败。解析器和清理阶段共用
// 同一个 watchdog，初始化、扫描或 close() 失联都不会让门禁无限等待。
// 非字面量 import() 只按完整文件路径放行下面两个测试夹具；其他文件与所有非字面量
// require() 一律报错。新增同类夹具必须同步更新 boundary-rules.ts 的清单。

import {
  type Dirent,
  existsSync,
  lstatSync,
  readdirSync,
  readFileSync,
  realpathSync,
  statSync,
} from "node:fs";
import { dirname, relative, resolve, sep } from "node:path";
import type { Node, SourceFile } from "typescript/unstable/ast";
import {
  isCallExpression,
  isExportDeclaration,
  isExternalModuleReference,
  isIdentifier,
  isImportDeclaration,
  isImportEqualsDeclaration,
  isImportExpression,
  isImportTypeNode,
  isLiteralTypeNode,
  isNoSubstitutionTemplateLiteral,
  isParenthesizedExpression,
  isStringLiteral,
} from "typescript/unstable/ast/is";
import { API, type Snapshot } from "typescript/unstable/async";
import {
  classifySpecifier,
  isNamedVirtualModuleFile,
  PACKAGES,
  packageOfPath,
  REGISTERED_ALIASES,
  resolveSpecifierTarget,
} from "./boundary-rules";

interface Configuration {
  readonly dependencies: readonly string[];
}

interface SpecifierLocation {
  readonly node: Node;
  readonly sourceFile: SourceFile;
}

const diagnostics: string[] = [];
let activeRepositoryRoot = process.cwd();

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function readJson(path: string, repositoryRoot: string): Record<string, unknown> {
  try {
    const value: unknown = JSON.parse(readFileSync(path, "utf8"));
    if (typeof value !== "object" || value === null || Array.isArray(value)) {
      throw new Error("JSON 顶层必须是对象");
    }
    return value as Record<string, unknown>;
  } catch (error) {
    throw new Error(`无法读取 ${relative(repositoryRoot, path)}：${errorMessage(error)}`);
  }
}

function isMissingDirectory(error: unknown): boolean {
  return (
    error instanceof Error &&
    "code" in error &&
    (error as { readonly code?: unknown }).code === "ENOENT"
  );
}

function listSourceFiles(
  directory: string,
  repositoryRoot: string,
  allowMissing: boolean,
): string[] {
  try {
    if (lstatSync(directory).isSymbolicLink()) {
      throw new Error(
        `受检源码目录里不允许符号链接：${relative(repositoryRoot, directory).split(sep).join("/")}`,
      );
    }
  } catch (error) {
    if (allowMissing && isMissingDirectory(error)) {
      return [];
    }
    if (error instanceof Error && error.message.startsWith("受检源码目录里不允许符号链接")) {
      throw error;
    }
    throw new Error(
      `无法读取源码目录 ${relative(repositoryRoot, directory)}：${errorMessage(error)}`,
    );
  }
  let entries: Dirent[];
  try {
    entries = readdirSync(directory, { withFileTypes: true });
  } catch (error) {
    if (allowMissing && isMissingDirectory(error)) {
      return [];
    }
    throw new Error(
      `无法读取源码目录 ${relative(repositoryRoot, directory)}：${errorMessage(error)}`,
    );
  }
  const files: string[] = [];
  for (const entry of entries) {
    const path = resolve(directory, entry.name);
    if (entry.isSymbolicLink()) {
      throw new Error(
        `受检源码目录里不允许符号链接：${relative(repositoryRoot, path).split(sep).join("/")}`,
      );
    }
    if (entry.isDirectory()) {
      files.push(...listSourceFiles(path, repositoryRoot, false));
    } else if (entry.isFile() && (entry.name.endsWith(".ts") || entry.name.endsWith(".tsx"))) {
      files.push(path);
    }
  }
  return files.sort();
}

function listConfigFiles(webRoot: string, repositoryRoot: string): string[] {
  const configs = new Set<string>();
  const addConfigs = (directory: string): void => {
    let entries: Dirent[];
    try {
      entries = readdirSync(directory, { withFileTypes: true });
    } catch (error) {
      throw new Error(
        `无法读取配置目录 ${relative(repositoryRoot, directory)}：${errorMessage(error)}`,
      );
    }
    for (const entry of entries) {
      if (entry.isDirectory()) {
        if (entry.name === "node_modules") {
          continue;
        }
        addConfigs(resolve(directory, entry.name));
        continue;
      }
      if (entry.isFile() && /^tsconfig.*\.json$/.test(entry.name)) {
        configs.add(resolve(directory, entry.name));
      }
    }
  };
  addConfigs(webRoot);
  return [...configs].sort();
}

// 配置可以经 extends 引入仓库内任意相对路径文件（含省略 .json 的写法与数组形式）；
// 只扫描 tsconfig*.json 会漏掉扩展文件里的 paths。包名 extends 需要 Node 解析规则，
// 当前仓库没有这种配置，出现时失败关闭而不是假装已核对。
function extendsTargets(
  configPath: string,
  config: Record<string, unknown>,
  repositoryRoot: string,
): string[] {
  const value = config.extends;
  if (value === undefined) {
    return [];
  }
  const entries = Array.isArray(value) ? value : [value];
  const targets: string[] = [];
  for (const entry of entries) {
    if (typeof entry !== "string" || entry === "") {
      throw new Error(
        `别名登记与检查器不一致：${relative(repositoryRoot, configPath)} 的 extends 格式无效`,
      );
    }
    if (!entry.startsWith("./") && !entry.startsWith("../")) {
      throw new Error(
        `别名登记与检查器不一致：${relative(repositoryRoot, configPath)} 的 extends 不是相对路径：${entry}`,
      );
    }
    const base = resolve(dirname(configPath), entry);
    const candidate = [base, `${base}.json`].find((path) => {
      try {
        return statSync(path).isFile();
      } catch {
        return false;
      }
    });
    if (candidate === undefined) {
      throw new Error(
        `别名登记与检查器不一致：无法解析 ${relative(repositoryRoot, configPath)} 的 extends：${entry}`,
      );
    }
    targets.push(candidate);
  }
  return targets;
}

function collectDependencies(
  packageJson: Record<string, unknown>,
  packageJsonPath: string,
): string[] {
  const dependencies = new Set<string>();
  for (const field of ["dependencies", "devDependencies"] as const) {
    const value = packageJson[field];
    if (value === undefined) {
      continue;
    }
    if (typeof value !== "object" || value === null || Array.isArray(value)) {
      throw new Error(`别名登记与检查器不一致：${packageJsonPath} 的 ${field} 不是对象`);
    }
    for (const name of Object.keys(value)) {
      dependencies.add(name);
    }
  }
  return [...dependencies];
}

function validateAliasConfiguration(webRoot: string, repositoryRoot: string): void {
  const basePath = resolve(webRoot, "tsconfig.base.json");
  const baseConfig = readJson(basePath, repositoryRoot);
  const compilerOptions = baseConfig.compilerOptions;
  if (
    typeof compilerOptions !== "object" ||
    compilerOptions === null ||
    Array.isArray(compilerOptions)
  ) {
    throw new Error(`别名登记与检查器不一致：${basePath} 的 compilerOptions 不是对象`);
  }
  const options = compilerOptions as { readonly baseUrl?: unknown; readonly paths?: unknown };
  if (!Object.hasOwn(options, "paths")) {
    throw new Error(`别名登记与检查器不一致：${basePath} 缺少 paths`);
  }
  if (typeof options.paths !== "object" || options.paths === null || Array.isArray(options.paths)) {
    throw new Error(`别名登记与检查器不一致：${basePath} 的 paths 不是对象`);
  }
  let baseDirectory = dirname(basePath);
  if (options.baseUrl !== undefined) {
    if (typeof options.baseUrl !== "string") {
      throw new Error(`别名登记与检查器不一致：${basePath} 的 baseUrl 不是字符串`);
    }
    baseDirectory = resolve(dirname(basePath), options.baseUrl);
  }
  const expected = new Map<string, string>();
  for (const alias of REGISTERED_ALIASES) {
    expected.set(alias.pattern, resolve(webRoot, alias.target));
  }
  const configured = new Map<string, string>();
  for (const [pattern, values] of Object.entries(options.paths)) {
    if (!expected.has(pattern)) {
      throw new Error(`别名登记与检查器不一致：${basePath} 登记了未写死的别名 ${pattern}`);
    }
    if (!Array.isArray(values) || values.length !== 1 || typeof values[0] !== "string") {
      throw new Error(`别名登记与检查器不一致：${pattern} 有多个候选或条件映射`);
    }
    configured.set(pattern, resolve(baseDirectory, values[0]));
  }
  for (const [pattern, target] of expected) {
    const actual = configured.get(pattern);
    if (actual === undefined) {
      throw new Error(`别名登记与检查器不一致：${basePath} 缺少 ${pattern}`);
    }
    if (actual !== target) {
      throw new Error(`别名登记与检查器不一致：${pattern} 指向 ${actual}，检查器要求 ${target}`);
    }
  }

  // 跟随 extends 链逐层核对；visited 防止循环 extends 让扫描不终止。
  const pending = listConfigFiles(webRoot, repositoryRoot);
  const visited = new Set<string>();
  while (pending.length > 0) {
    const configPath = pending.pop();
    if (configPath === undefined) {
      break;
    }
    if (visited.has(configPath)) {
      continue;
    }
    visited.add(configPath);
    const config = readJson(configPath, repositoryRoot);
    pending.push(...extendsTargets(configPath, config, repositoryRoot));
    if (configPath === basePath) {
      continue;
    }
    const otherOptions = config.compilerOptions;
    if (otherOptions === undefined) {
      continue;
    }
    if (typeof otherOptions !== "object" || otherOptions === null || Array.isArray(otherOptions)) {
      throw new Error(
        `别名登记与检查器不一致：${relative(repositoryRoot, configPath)} 的 compilerOptions 不是对象`,
      );
    }
    if (Object.hasOwn(otherOptions, "paths")) {
      throw new Error(
        `别名只能登记在 web/tsconfig.base.json：${relative(repositoryRoot, configPath)}`,
      );
    }
  }
}

function loadConfiguration(webRoot: string, repositoryRoot: string): Configuration {
  const packageJsonPath = resolve(webRoot, "package.json");
  const packageJson = readJson(packageJsonPath, repositoryRoot);
  const imports = packageJson.imports;
  if (imports !== undefined) {
    if (typeof imports !== "object" || imports === null || Array.isArray(imports)) {
      throw new Error(`别名登记与检查器不一致：${packageJsonPath} 的 imports 不是对象`);
    }
    if (Object.keys(imports).length > 0) {
      throw new Error(`别名登记与检查器不一致：${packageJsonPath} 的 imports 包含未登记的导入名`);
    }
  }
  const dependencies = collectDependencies(packageJson, packageJsonPath);
  validateAliasConfiguration(webRoot, repositoryRoot);
  return { dependencies };
}

function lineOf(location: SpecifierLocation): number {
  return (
    location.sourceFile.getLineAndCharacterOfPosition(location.node.getStart(location.sourceFile))
      .line + 1
  );
}

function stringSpecifier(node: Node): string | undefined {
  if (isStringLiteral(node) || isNoSubstitutionTemplateLiteral(node)) {
    return node.text;
  }
  return undefined;
}

function report(location: SpecifierLocation, message: string): void {
  diagnostics.push(
    `${relativeSourcePath(location.sourceFile, activeRepositoryRoot)}:${lineOf(location)}: ${message}`,
  );
}

function dynamicSpecifier(
  node: Node,
  sourceFile: SourceFile,
  allowNonLiteral: boolean,
): string | undefined {
  if (!isCallExpression(node)) {
    return undefined;
  }
  const argument = node.arguments[0];
  if (argument !== undefined) {
    const text = stringSpecifier(argument);
    if (text !== undefined) {
      return text;
    }
  }
  if (allowNonLiteral) {
    return undefined;
  }
  report(
    { node: argument ?? node, sourceFile },
    "动态导入的模块路径无法静态判定；必须使用字符串字面量或无插值模板字面量",
  );
  return undefined;
}

function importTypeSpecifier(node: Node, sourceFile: SourceFile): string | undefined {
  if (!isImportTypeNode(node)) {
    return undefined;
  }
  if (isLiteralTypeNode(node.argument)) {
    const text = stringSpecifier(node.argument.literal);
    if (text !== undefined) {
      return text;
    }
  }
  report({ node: node.argument, sourceFile }, "import 类型的模块路径无法静态判定");
  return undefined;
}

function unwrapParentheses(node: Node): Node {
  let current = node;
  while (isParenthesizedExpression(current)) {
    current = current.expression;
  }
  return current;
}

function externalModuleSpecifier(
  node: Node,
  sourceFile: SourceFile,
  allowVirtualImport: boolean,
): string | undefined {
  if (isImportDeclaration(node)) {
    return stringSpecifier(node.moduleSpecifier);
  }
  if (isExportDeclaration(node) && node.moduleSpecifier !== undefined) {
    return stringSpecifier(node.moduleSpecifier);
  }
  if (isImportEqualsDeclaration(node) && isExternalModuleReference(node.moduleReference)) {
    return stringSpecifier(node.moduleReference.expression);
  }
  if (isImportTypeNode(node)) {
    return importTypeSpecifier(node, sourceFile);
  }
  if (isCallExpression(node)) {
    if (isImportExpression(node.expression)) {
      return dynamicSpecifier(node, sourceFile, allowVirtualImport);
    }
    const callee = unwrapParentheses(node.expression);
    if (isIdentifier(callee) && callee.text === "require") {
      return dynamicSpecifier(node, sourceFile, false);
    }
  }
  return undefined;
}

function relativeSourcePath(sourceFile: SourceFile, repositoryRoot: string): string {
  return relative(repositoryRoot, sourceFile.fileName).split(sep).join("/");
}

function checkSpecifier(
  sourceFile: SourceFile,
  specifier: string,
  location: SpecifierLocation,
  repositoryRoot: string,
  dependencies: readonly string[],
): void {
  const decision = classifySpecifier({
    sourceFile: sourceFile.fileName,
    specifier,
    repositoryRoot,
    dependencies,
  });
  if (decision.kind === "violation") {
    report(location, decision.reason);
    return;
  }
  const target = resolveSpecifierTarget({
    sourceFile: sourceFile.fileName,
    specifier,
    repositoryRoot,
  });
  if (target === undefined || !existsSync(target)) {
    return;
  }
  let realTarget: string;
  try {
    realTarget = realpathSync(target);
  } catch (error) {
    throw new Error(`无法解析目标真实路径 ${specifier}：${errorMessage(error)}`);
  }
  if (realTarget !== resolve(target)) {
    report(location, `目标经过符号链接：${specifier}`);
  }
}

function visit(
  sourceFile: SourceFile,
  repositoryRoot: string,
  dependencies: readonly string[],
): void {
  const allowVirtualImport = isNamedVirtualModuleFile(sourceFile.fileName, repositoryRoot);
  const walk = (node: Node): void => {
    const specifier = externalModuleSpecifier(node, sourceFile, allowVirtualImport);
    if (specifier !== undefined) {
      checkSpecifier(sourceFile, specifier, { node, sourceFile }, repositoryRoot, dependencies);
    }
    node.forEachChild(walk);
  };
  walk(sourceFile);
}

async function projectForFile(snapshot: Snapshot, file: string) {
  const project = await snapshot.getDefaultProjectForFile(file);
  if (project !== undefined) {
    return project;
  }
  return snapshot.getProjects().find((candidate) => candidate.rootFiles.includes(file));
}

export async function runBoundaryCheck(repositoryRoot: string): Promise<0 | 1 | 2> {
  diagnostics.length = 0;
  let api: API | undefined;
  let snapshot: Snapshot | undefined;
  let failure: string | undefined;
  let timedOut = false;
  const timeout = Promise.withResolvers<never>();
  const watchdog = setTimeout(() => {
    timedOut = true;
    timeout.reject(new Error("TypeScript 解析器在 120 秒内没有完成"));
  }, 120_000);
  const work = async (): Promise<void> => {
    activeRepositoryRoot = repositoryRoot;
    const webRoot = resolve(repositoryRoot, "web");
    const configuration = loadConfiguration(webRoot, repositoryRoot);
    const files = PACKAGES.flatMap((packageName) =>
      listSourceFiles(
        resolve(webRoot, packageName, "src"),
        repositoryRoot,
        packageName === "admin",
      ),
    );
    if (files.length === 0) {
      throw new Error(
        "扫描结果为空：没有找到 web/{contracts,shared,public,admin}/src 的 TypeScript 源码",
      );
    }
    api = new API({ cwd: webRoot });
    snapshot = await api.updateSnapshot({ openFiles: files });
    if (timedOut) {
      return;
    }
    for (const file of files) {
      if (timedOut) {
        return;
      }
      const sourcePackage = packageOfPath(file, repositoryRoot);
      if (sourcePackage === undefined) {
        throw new Error(`无法确定 ${relative(repositoryRoot, file)} 所属的包`);
      }
      const project = await projectForFile(snapshot, file);
      if (timedOut) {
        return;
      }
      if (project === undefined) {
        throw new Error(`无法确定 ${relative(repositoryRoot, file)} 所属的 TypeScript 项目`);
      }
      const sourceFile = await project.program.getSourceFile(file);
      if (timedOut) {
        return;
      }
      if (sourceFile === undefined) {
        throw new Error(`无法解析 ${relative(repositoryRoot, file)}`);
      }
      visit(sourceFile, repositoryRoot, configuration.dependencies);
    }
  };
  const workPromise = work();
  const cleanup = async (): Promise<void> => {
    if (snapshot !== undefined) {
      try {
        await snapshot.dispose();
      } catch (error) {
        failure ??= `释放 TypeScript 快照失败：${errorMessage(error)}`;
      }
    }
    if (api !== undefined) {
      try {
        await api.close();
      } catch (error) {
        failure ??= `关闭 TypeScript 解析器失败：${errorMessage(error)}`;
      }
    }
  };
  try {
    await Promise.race([workPromise, timeout.promise]);
  } catch (error) {
    failure = errorMessage(error);
  }
  if (timedOut) {
    // 工作中的解析调用可能仍在使用快照：先等它响应 timedOut 退出，再释放资源。
    // 清理不能脱离本次结果，否则清理失败会在调用方退出后才出现。
    await workPromise.catch(() => undefined);
  }
  const cleanupPromise = cleanup();
  if (!timedOut) {
    try {
      await Promise.race([cleanupPromise, timeout.promise]);
    } catch (error) {
      failure ??= errorMessage(error);
      void cleanupPromise.catch(() => undefined);
    }
  } else {
    await cleanupPromise;
  }
  clearTimeout(watchdog);
  for (const diagnostic of diagnostics) {
    console.error(diagnostic);
  }
  if (failure !== undefined) {
    console.error(`Web 包边界检查失败：${failure}`);
    return 2;
  }
  return diagnostics.length > 0 ? 1 : 0;
}
