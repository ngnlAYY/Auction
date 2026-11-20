// 默认译文损坏时，根组件只挂载错误容器，不初始化正常外壳。

import { expect, spyOn, test } from "bun:test";
import { type Component, createComponent, createRoot, flush, Show } from "solid-js";

class CapturedElement {
  constructor(
    readonly tag: unknown,
    readonly attributes: Record<string, unknown>,
    readonly children: readonly unknown[],
  ) {}
}

function captureJsx(
  tag: unknown,
  attributes: Record<string, unknown> | null,
  ...nodes: unknown[]
): unknown {
  const props = attributes ?? {};
  if (tag === Show) {
    const when = props.when;
    if (when) {
      const child = nodes.length === 1 ? nodes[0] : nodes;
      return typeof child === "function" ? child(() => when) : child;
    }
    const fallback = props.fallback;
    return typeof fallback === "function" ? fallback() : fallback;
  }
  let children: readonly unknown[] = nodes;
  if (children.length === 0 && props.children !== undefined) {
    const value = props.children;
    children = [typeof value === "function" ? value() : value];
  }
  return new CapturedElement(tag, props, children);
}

function renderedElements(node: unknown): CapturedElement[] {
  if (node instanceof CapturedElement) {
    return [node, ...node.children.flatMap(renderedElements)];
  }
  return Array.isArray(node) ? node.flatMap(renderedElements) : [];
}

function renderedText(node: unknown): string {
  if (typeof node === "string" || typeof node === "number") {
    return String(node);
  }
  if (node instanceof CapturedElement) {
    return node.children.map(renderedText).join("");
  }
  return Array.isArray(node) ? node.map(renderedText).join("") : "";
}

/** Context.Provider 不产生 DOM；只展开它的子节点，得到可见根节点。 */
function visibleRoots(node: unknown, provider: unknown): unknown[] {
  if (node instanceof CapturedElement && node.tag === provider) {
    return node.children.flatMap((child) => visibleRoots(child, provider));
  }
  return Array.isArray(node) ? node.flatMap((child) => visibleRoots(child, provider)) : [node];
}

function assertFallback(tree: unknown, provider: unknown): void {
  const roots = visibleRoots(tree, provider);
  expect(roots).toHaveLength(1);
  const visible = roots[0];
  expect(visible instanceof CapturedElement).toBe(true);
  if (!(visible instanceof CapturedElement)) {
    throw new Error("可见根节点缺失");
  }
  expect(visible.tag).toBe("div");
  expect(visible.attributes.class).toBe("app-root");
  expect(visible.children).toEqual(["无法初始化界面译文。"]);
  expect(renderedText(tree)).toBe("无法初始化界面译文。");
  const names = renderedElements(tree).map(({ tag }) =>
    typeof tag === "function" ? tag.name : null,
  );
  expect(names).not.toContain("LocalizedApp");
  expect(names).not.toContain("AdminShell");
  expect(names).not.toContain("RouterHost");
  expect(names).not.toContain("StaleBuildBanner");
}

test("invalid_default_locale_keeps_the_fallback_through_root_rerenders：默认译文损坏时只挂载错误容器", async () => {
  const previousDocument = Object.getOwnPropertyDescriptor(globalThis, "document");
  const previousJsx = Object.getOwnPropertyDescriptor(globalThis, "jsx");
  if (previousJsx === undefined) {
    throw new Error("测试 JSX 转译未装配");
  }
  const application: { factory?: () => unknown } = {};
  const web = await import("@solidjs/web");
  const renderSpy = spyOn(web, "render").mockImplementation((factory: () => unknown) => {
    application.factory = factory;
    return () => {};
  });
  try {
    Object.defineProperty(globalThis, "document", {
      configurable: true,
      value: { getElementById: () => ({}) },
    });
    try {
      const { ADMIN_LOCALE_CATALOG, run } = await import("./index");
      const { InitialAdminLocationContext } = await import("./router");
      const originalFtl = ADMIN_LOCALE_CATALOG.defaultFtl;
      const i18n = await import("@auction/web-shared/ui/i18n");
      const createLocaleSession = spyOn(i18n, "createLocaleSession");
      let dispose = (): void => {};
      const createdComponents: string[] = [];
      try {
        expect(Reflect.set(ADMIN_LOCALE_CATALOG, "defaultFtl", "broken = {")).toBe(true);
        Object.defineProperty(globalThis, "jsx", {
          ...previousJsx,
          value: (...args: Parameters<typeof captureJsx>) => {
            const [tag] = args;
            if (typeof tag === "function") createdComponents.push(tag.name);
            return captureJsx(...args);
          },
        });
        run();
        expect(renderSpy).toHaveBeenCalledTimes(1);
        const renderApp = application.factory;
        if (renderApp === undefined) throw new Error("根组件未交给渲染入口");

        // 失败分支没有响应式依赖，同一次挂载内没有可触发的更新。
        let mountCount = 0;
        const tree = createRoot((disposeRoot) => {
          dispose = disposeRoot;
          const root = renderApp();
          if (!(root instanceof CapturedElement) || typeof root.tag !== "function") {
            throw new Error("根组件元素缺失");
          }
          mountCount += 1;
          // JSX 入口已确认根节点是可调用的组件。
          return createComponent(root.tag as Component<Record<string, unknown>>, root.attributes);
        });
        flush();
        expect(mountCount).toBe(1);
        assertFallback(tree, InitialAdminLocationContext);
        expect(createdComponents).not.toContain("LocalizedApp");
        expect(createLocaleSession).not.toHaveBeenCalled();
      } finally {
        try {
          dispose();
          flush();
        } finally {
          try {
            createLocaleSession.mockRestore();
          } finally {
            try {
              Object.defineProperty(globalThis, "jsx", previousJsx);
            } finally {
              expect(Reflect.set(ADMIN_LOCALE_CATALOG, "defaultFtl", originalFtl)).toBe(true);
            }
          }
        }
      }
    } finally {
      if (previousDocument === undefined) Reflect.deleteProperty(globalThis, "document");
      else Object.defineProperty(globalThis, "document", previousDocument);
    }
  } finally {
    renderSpy.mockRestore();
  }
});
