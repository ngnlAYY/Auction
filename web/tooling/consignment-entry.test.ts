// 用源码静态导入图锁定 Public 与 Admin 首屏可达的寄售模块。

import { expect, test } from "bun:test";
import { dirname, relative, resolve } from "node:path";
import { NodeFlags, type SourceFile, SyntaxKind } from "typescript/unstable/ast";
import {
  isExportDeclaration,
  isImportDeclaration,
  isNamedImports,
  isStringLiteral,
  isTypeAliasDeclaration,
  isVariableStatement,
} from "typescript/unstable/ast/is";
import { API, type Snapshot } from "typescript/unstable/async";

import { packageOfPath, resolveSpecifierTarget } from "./boundary-rules";

const repositoryRoot = resolve(import.meta.dir, "../..");
const publicEntry = resolve(repositoryRoot, "web/public/src/main.tsx");
const adminEntry = resolve(repositoryRoot, "web/admin/src/main.ts");
const publicContract = "web/contracts/src/dto/public/consignment";
const publicPages = "web/public/src/surface/consignment";
const capabilityDeclarationsAndExports = {
  [`${publicContract}/listing-options.ts`]: [
    'import { maxValue, picklist, pipe, type InferOutput } from "valibot"',
    'import { unsignedInteger } from "../../../wire"',
    "export const LISTING_PRICE_MAX",
    "export const ListingFeeBpsSchema",
    "export const ListingModeSchema",
    "export const ListingWorldSchema",
    "export type ListingMode",
    "export type ListingWorld",
  ],
  [`${publicContract}/status.ts`]: [
    'import { array, boolean, check, literal, pipe, type InferOutput } from "valibot"',
    'import { strictStruct } from "../../../wire"',
    'import { LISTING_PRICE_MAX, ListingFeeBpsSchema, ListingModeSchema, ListingWorldSchema } from "./listing-options"',
    "export const ConsignmentStatusSchema",
    "export type ConsignmentStatus",
  ],
};

function declarationsAndExports(source: SourceFile): string[] {
  return source.statements.flatMap((statement) => {
    if (isImportDeclaration(statement)) {
      const clause = statement.importClause;
      const bindings = clause?.namedBindings;
      if (
        bindings !== undefined &&
        isNamedImports(bindings) &&
        clause?.name === undefined &&
        clause?.phaseModifier === undefined &&
        statement.attributes === undefined
      ) {
        const names = bindings.elements.map((binding) => binding.getText(source)).sort();
        const specifier = statement.moduleSpecifier.getText(source);
        return [`import { ${names.join(", ")} } from ${specifier}`];
      }
    }
    if (!isVariableStatement(statement) && !isTypeAliasDeclaration(statement)) {
      // 保留其他顶层语句供精确比较，包括默认导出、export = 和重导出。
      return [statement.getText(source)];
    }

    const modifiers = statement.modifiers?.map((modifier) => modifier.getText(source)) ?? [];
    if (isTypeAliasDeclaration(statement)) {
      return [[...modifiers, "type", statement.name.text].join(" ")];
    }
    const { declarations, flags } = statement.declarationList;
    const kind = (flags & NodeFlags.Const) !== 0 ? "const" : "其他变量声明";
    return declarations.map(({ name }) => [...modifiers, kind, name.getText(source)].join(" "));
  });
}

async function staticModules(snapshot: Snapshot, entry: string) {
  const project = await snapshot.getDefaultProjectForFile(entry);
  if (project === undefined) throw new Error(`无法确定入口 ${entry} 的 TypeScript 项目`);

  const reached = new Map<string, SourceFile>();
  const pending = [entry];
  for (const file of pending) {
    const path = relative(repositoryRoot, file);
    if (reached.has(path)) continue;
    const source = await project.program.getSourceFile(file);
    if (source === undefined) throw new Error(`无法解析静态导入 ${path}`);
    reached.set(path, source);

    for (const statement of source.statements) {
      const isStaticImport =
        isImportDeclaration(statement) &&
        statement.importClause?.phaseModifier !== SyntaxKind.TypeKeyword;
      const isStaticExport = isExportDeclaration(statement) && !statement.isTypeOnly;
      if (!isStaticImport && !isStaticExport) continue;
      const specifier = statement.moduleSpecifier;
      if (specifier === undefined || !isStringLiteral(specifier)) continue;
      const specifierText = specifier.text;
      const target =
        specifierText === "." || specifierText === ".."
          ? resolve(dirname(file), specifierText)
          : resolveSpecifierTarget({ sourceFile: file, specifier: specifierText, repositoryRoot });
      if (target === undefined || packageOfPath(target, repositoryRoot) === undefined) continue;
      const dependency = Bun.resolveSync(target, dirname(file));
      if (/\.tsx?$/u.test(dependency)) pending.push(dependency);
    }
  }
  return reached;
}

// TypeScript 需要解析两个入口及其所在的包，单独放宽本用例的超时。
test("Public 和 Admin 的静态入口只可达约定的寄售模块与能力原语", async () => {
  const api = new API({ cwd: resolve(repositoryRoot, "web") });
  try {
    const snapshot = await api.updateSnapshot({ openFiles: [publicEntry, adminEntry] });
    try {
      const publicModules = await staticModules(snapshot, publicEntry);
      expect(
        [...publicModules.keys()].filter((path) => path.startsWith(`${publicContract}/`)).sort(),
      ).toEqual(Object.keys(capabilityDeclarationsAndExports).sort());
      expect(
        [...publicModules.keys()].filter((path) => path.startsWith(`${publicPages}/`)),
      ).toEqual([`${publicPages}/capability.ts`]);

      // 仅检查文件路径无法发现把其他 schema 搬进已可达的窄模块。
      for (const [path, expected] of Object.entries(capabilityDeclarationsAndExports)) {
        const source = publicModules.get(path);
        if (source === undefined) throw new Error(`无法检查静态模块 ${path}`);
        expect(declarationsAndExports(source).sort()).toEqual([...expected].sort());
      }

      const adminModules = await staticModules(snapshot, adminEntry);
      expect(
        [...adminModules.keys()].filter((path) =>
          /^web\/(?:contracts\/src\/dto\/(?:public|admin)|admin\/src\/(?:api|surface))\/consignment\//u.test(
            path,
          ),
        ),
      ).toEqual([]);
    } finally {
      await snapshot.dispose();
    }
  } finally {
    await api.close();
  }
}, 30_000);
