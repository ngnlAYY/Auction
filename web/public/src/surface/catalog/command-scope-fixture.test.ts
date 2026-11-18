// 竞价宿主测试用的 Solid JSX 运行时，保留组件作用域和动态属性，只替换 DOM。

import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import type { CurrentAuctionItemResponse } from "@auction/contracts/dto/public/catalog/auction";
import { Dialog } from "@auction/web-shared/ui/components/dialog";
import { transform } from "@solidjs/compiler";
import { createMemo, type Element, createComponent as mountComponent, untrack } from "solid-js";

export { createEffect as effect, createMemo as memo } from "solid-js";

const NAMESPACE = "public-command-test";
export const TEST_AUCTION: CurrentAuctionItemResponse = {
  id: "00000000-0000-4000-8000-000000000021",
  world: "main",
  number: 12,
  status: "live",
  opens_at: "2026-11-01T00:00:00Z",
  scheduled_ends_at: "2037-12-01T00:00:00Z",
  presentation_mode: "live",
  bidding_open: true,
  preview_revision: 0,
  proxy_default_mode: "fast",
};
const SOURCES = new Set(
  ["../board/index.tsx", "./command-scope.tsx", "../commands/bid.tsx", "../commands/retry.tsx"].map(
    (path) => resolve(import.meta.dir, path),
  ),
);
const stripTypes = new Bun.Transpiler({ loader: "ts" });

Bun.plugin({
  name: NAMESPACE,
  setup(build) {
    build.onResolve({ filter: /.*/u, namespace: NAMESPACE }, ({ path }) => ({
      path,
      namespace: NAMESPACE,
    }));
    build.onLoad({ filter: /.*/u, namespace: NAMESPACE }, ({ path }) => {
      if (!SOURCES.has(path)) {
        throw new Error(`竞价测试未登记源码 ${path}`);
      }
      const { code } = transform(readFileSync(path, "utf8"), {
        filename: path,
        generate: "universal",
        moduleName: import.meta.path,
      });
      // 测试命名空间没有真实父目录；只重定向指定 JSX，其余导入复用正常模块实例。
      const contents = stripTypes
        .transformSync(code)
        .replace(/^(import .* from )("[^"\n]+");$/gmu, (_line, prefix, specifier) => {
          const dependency = Bun.resolveSync(JSON.parse(specifier), dirname(path));
          const target = SOURCES.has(dependency) ? `${NAMESPACE}:${dependency}` : dependency;
          return `${prefix}${JSON.stringify(target)};`;
        });
      return { contents, loader: "js" };
    });
  },
});

/** 加载实际组件；同一次编译共享同一个命令上下文。 */
export async function loadCommandScopeFixture() {
  const boardModule = `${NAMESPACE}:${resolve(import.meta.dir, "../board/index.tsx")}`;
  const scopeModule = `${NAMESPACE}:${resolve(import.meta.dir, "./command-scope.tsx")}`;
  return {
    board: (await import(boardModule)) as typeof import("../board"),
    scope: (await import(scopeModule)) as typeof import("./command-scope"),
  };
}

/** 替身节点保留测试可访问的提交处理器与动态属性。 */
export class TestElement {
  readonly nodes: (() => unknown)[] = [];

  constructor(
    readonly tag: string,
    readonly attributes: Record<string, unknown>,
  ) {}

  submit(): void {
    const submit = this.attributes.onSubmit;
    if (typeof submit !== "function") {
      throw new Error("竞价表单没有绑定提交处理器");
    }
    submit({ preventDefault() {} });
  }

  click(): void {
    const click = this.attributes.onClick;
    if (typeof click !== "function") throw new Error("控件没有绑定点击处理器");
    click(new Event("click", { cancelable: true }));
  }
}

export function createComponent<P extends object>(
  component: (props: P) => Element,
  props: P,
): Element {
  // 初始化仍只读取一次；JSX 生成的 getter 在组件内的追踪作用域求值。
  return Object.is(component, Dialog)
    ? (Reflect.get(props, "children") as Element)
    : mountComponent((input) => untrack(() => component(input)), props, component.name);
}

export function createElement(tag: string, props: Record<string, unknown> = {}): TestElement {
  return new TestElement(tag, props);
}

export function insert(parent: TestElement, node: unknown): void {
  parent.nodes.push(createMemo(() => (typeof node === "function" ? node() : node)));
}

export function insertNode(parent: TestElement, node: TestElement): void {
  parent.nodes.push(() => node);
}

export function setProp(element: TestElement, key: string, value: unknown): void {
  element.attributes[key] = value;
}

// 替身节点没有焦点。
export function ref(): void {}

/** 在已挂载的活动子树里找节点。 */
export function findElement(
  node: unknown,
  match: (element: TestElement) => boolean,
): TestElement | null {
  if (typeof node === "function") {
    return findElement(node(), match);
  }
  const elements = node instanceof TestElement ? node.nodes : Array.isArray(node) ? node : [];
  if (node instanceof TestElement && match(node)) {
    return node;
  }
  for (const element of elements) {
    const found = findElement(element, match);
    if (found !== null) {
      return found;
    }
  }
  return null;
}
