// 包边界检查的命令行薄壳：只解析仓库根参数，再动态加载依赖 TypeScript 解析器的实现。
// 这样实现模块或它的第三方依赖加载失败时能统一以退出码 2 报告，而不会被误当成越界。

import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptRepositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function parseTargetRoot(): string {
  const argumentsList = process.argv.slice(2);
  const unknownFlags = argumentsList.filter(
    (argument) => argument !== "--no-install" && argument.startsWith("-"),
  );
  if (unknownFlags.length > 0) {
    throw new Error(`未知命令行参数：${unknownFlags.join(" ")}`);
  }
  const positional = argumentsList.filter((argument) => argument !== "--no-install");
  if (positional.length > 1) {
    throw new Error("检查器最多接受一个仓库根路径");
  }
  return resolve(positional[0] ?? scriptRepositoryRoot);
}

if ((import.meta as ImportMeta & { readonly main?: boolean }).main === true) {
  try {
    const repositoryRoot = parseTargetRoot();
    const { runBoundaryCheck } = await import("./check-boundaries-impl.ts");
    process.exitCode = await runBoundaryCheck(repositoryRoot);
  } catch (error) {
    console.error(`Web 包边界检查无法完成：${errorMessage(error)}`);
    process.exitCode = 2;
  }
}
