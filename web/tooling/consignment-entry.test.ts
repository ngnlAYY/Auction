// 用源码静态导入图锁定 Public 与 Admin 首屏可达的寄售模块。

import { expect, test } from "bun:test";
import { dirname, relative, resolve } from "node:path";
import { type SourceFile, SyntaxKind } from "typescript/unstable/ast";
import {
  isExportDeclaration,
  isImportDeclaration,
  isStringLiteral,
  isVariableStatement,
} from "typescript/unstable/ast/is";
import { API, type Snapshot } from "typescript/unstable/async";

import { packageOfPath, resolveSpecifierTarget } from "./boundary-rules";

const repositoryRoot = resolve(import.meta.dir, "../..");
const publicEntry = resolve(repositoryRoot, "web/public/src/main.tsx");
const adminEntry = resolve(repositoryRoot, "web/admin/src/main.ts");
const publicContract = "web/contracts/src/dto/public/consignment";
const publicPages = "web/public/src/surface/consignment";
const capabilityDeclarations = {
  [`${publicContract}/listing-options.ts`]: [
    "LISTING_PRICE_MAX",
    "ListingFeeBpsSchema",
    "ListingModeSchema",
    "ListingWorldSchema",
  ],
  [`${publicContract}/status.ts`]: ["ConsignmentStatusSchema"],
};

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

test("Public 和 Admin 的静态入口只可达约定的寄售模块与能力原语", async () => {
  const api = new API({ cwd: resolve(repositoryRoot, "web") });
  try {
    const snapshot = await api.updateSnapshot({ openFiles: [publicEntry, adminEntry] });
    try {
      const publicModules = await staticModules(snapshot, publicEntry);
      expect(
        [...publicModules.keys()].filter((path) => path.startsWith(`${publicContract}/`)).sort(),
      ).toEqual(Object.keys(capabilityDeclarations).sort());
      expect(
        [...publicModules.keys()].filter((path) => path.startsWith(`${publicPages}/`)),
      ).toEqual([`${publicPages}/capability.ts`]);

      // 仅检查文件路径无法发现把其他 schema 搬进已可达的窄模块。
      for (const [path, expected] of Object.entries(capabilityDeclarations)) {
        const source = publicModules.get(path);
        const declarations = source?.statements.flatMap((statement) =>
          isVariableStatement(statement)
            ? statement.declarationList.declarations.map(({ name }) => name.getText(source))
            : [],
        );
        expect(declarations?.sort()).toEqual(expected);
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
});
