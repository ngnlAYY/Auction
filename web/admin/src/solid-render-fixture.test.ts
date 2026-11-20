// 在独立命名空间内编译已登记的真实 TSX，普通导入继续由工作区预加载处理。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { parseJson } from "@auction/web-shared/net/wire";
import { transform } from "@solidjs/compiler";
import { ADMIN_RENDER_SOURCES, SHARED_RENDER_SOURCES } from "./solid-render-sources-fixture";

const NAMESPACE = "auction-admin-render";
const ADMIN_ROOT = import.meta.dir;
const SHARED_ROOT = resolve(ADMIN_ROOT, "../../shared/src");
const RUNTIME = resolve(ADMIN_ROOT, "surface/solid-dom-fixture.ts");
const SOURCES = new Set([
  ...ADMIN_RENDER_SOURCES.map((path) => resolve(ADMIN_ROOT, path)),
  ...SHARED_RENDER_SOURCES.map((path) => resolve(SHARED_ROOT, path)),
]);
const stripTypes = new Bun.Transpiler({ loader: "ts" });
let registered = false;

function requireSource(path: string): void {
  if (!SOURCES.has(path)) throw new Error(`未登记 Admin 渲染源码 ${path}`);
}

function registerRenderer(): void {
  if (registered) return;
  Bun.plugin({
    name: NAMESPACE,
    setup(build) {
      build.onResolve({ filter: /.*/u, namespace: NAMESPACE }, ({ path }) => ({
        path,
        namespace: NAMESPACE,
      }));
      build.onLoad({ filter: /.*/u, namespace: NAMESPACE }, ({ path }) => {
        requireSource(path);
        const { code } = transform(readFileSync(path, "utf8"), {
          filename: path,
          generate: "universal",
          moduleName: RUNTIME,
        });
        const contents = stripTypes
          .transformSync(code)
          .replace(
            /(\bfrom\s+|\bimport\()("(?:[^"\\\n]|\\.)*")/gu,
            (_, prefix: string, raw: string) => {
              const specifier = parseJson(raw);
              if (!specifier.ok || typeof specifier.value !== "string") {
                throw new Error(`无法解析 JSX 导入 ${raw}`);
              }
              const dependency = Bun.resolveSync(specifier.value, dirname(path));
              const target = SOURCES.has(dependency) ? `${NAMESPACE}:${dependency}` : dependency;
              return prefix + JSON.stringify(target);
            },
          );
        return { contents, loader: "js" };
      });
    },
  });
  registered = true;
}

function hasRenderExports<T>(value: unknown, names: readonly string[]): value is T {
  return (
    typeof value === "object" &&
    value !== null &&
    names.every((name) => typeof Reflect.get(value, name) === "function")
  );
}

/** 加载调用方指定的已登记源码；类型取自原模块，并在运行期核对所用的函数导出。 */
export async function loadRenderModule<T extends object>(
  source: string,
  firstExport: keyof T & string,
  ...otherExports: (keyof T & string)[]
): Promise<T> {
  const path = resolve(source);
  requireSource(path);
  registerRenderer();
  const module: unknown = await import(`${NAMESPACE}:${path}`);
  const names = [firstExport, ...otherExports];
  if (!hasRenderExports<T>(module, names)) {
    throw new Error(`渲染模块 ${path} 缺少导出 ${names.join("、")}`);
  }
  return module;
}

test("渲染夹具拒绝未登记的源码路径", async () => {
  await expect(
    loadRenderModule<{ readonly step: unknown }>(
      resolve(ADMIN_ROOT, "surface/auth/session/transition.ts"),
      "step",
    ),
  ).rejects.toThrow("未登记 Admin 渲染源码");
});
