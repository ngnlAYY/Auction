// 读取页的离线测试宿主：真实响应式状态，模拟地址栏、History 写入与 JSX 事件。

import { createCell } from "@auction/web-shared/reactive/cell";
import { createRouter, RouterContext } from "@auction/web-shared/router";
import { installFakeWindow } from "@auction/web-shared/router/test-window";
import { createRoot, flush, getOwner, runWithOwner, Show } from "solid-js";

import { PublicPageRealtimeTargetContext } from "../board/realtime";
import {
  createPublicBidRefresh,
  PublicBidRefreshContext,
  PublicRefreshGenerationContext,
} from "../board/refresh";
import { PublicContentLoadingContext } from "../loading";
import { InitialPublicLocationContext, initialPublicLocation } from "../query";
import { PUBLIC_ROUTES } from "../router/route";
import { WorldSwitchContext } from "../shell/contexts";

interface ReadPageElement {
  readonly tag: unknown;
  readonly props: Record<string, unknown>;
  readonly children: readonly unknown[];
}

/** 创建可拒绝写入的本地 History 替身；初始路由规范化完成后清空写入记录。 */
export function mountReadPage(
  initialUrl: string,
  render: () => void,
  mountElement?: (element: ReadPageElement) => void,
  rejectInitialWrites = false,
) {
  const { fake, restore } = installFakeWindow(initialUrl);
  fake.rejectWrites(rejectInitialWrites);
  const { location, writes } = fake;
  const storage = new Map<string, string>();
  Object.assign(fake, {
    localStorage: {
      getItem: (key: string) => storage.get(key) ?? null,
      setItem: (key: string, value: string) => storage.set(key, value),
    },
  });
  const previousJsx = Object.getOwnPropertyDescriptor(globalThis, "jsx");
  const elements: ReadPageElement[] = [];
  Object.defineProperty(globalThis, "jsx", {
    configurable: true,
    value: (tag: unknown, props: Record<string, unknown> | null, ...children: unknown[]) => {
      const element = { tag, props: props ?? {}, children };
      elements.push(element);
      mountElement?.(element);
      return element;
    },
  });
  const oldRouter = RouterContext.defaultValue;
  const oldInitial = InitialPublicLocationContext.defaultValue;
  const oldRefresh = PublicRefreshGenerationContext.defaultValue;
  const oldBidRefresh = PublicBidRefreshContext.defaultValue;
  const oldPageTarget = PublicPageRealtimeTargetContext.defaultValue;
  const oldLoading = PublicContentLoadingContext.defaultValue;
  const oldWorld = WorldSwitchContext.defaultValue;
  const loading = createCell<ReadonlySet<number>>(new Set());
  const initial = initialPublicLocation();
  const root = createRoot((dispose) => {
    const owner = getOwner();
    const router = createRouter({ routes: PUBLIC_ROUTES });
    writes.length = 0;
    RouterContext.defaultValue = router;
    InitialPublicLocationContext.defaultValue = initial;
    PublicRefreshGenerationContext.defaultValue = createCell(0);
    PublicBidRefreshContext.defaultValue = createPublicBidRefresh();
    PublicPageRealtimeTargetContext.defaultValue = createCell(null);
    PublicContentLoadingContext.defaultValue = loading;
    WorldSwitchContext.defaultValue = {
      options: () => ["all", "main", "isekai"].map((value) => ({ value, disabled: false })),
      current: () => "all",
      choose: () => {},
      selectable: () => true,
    };
    render();
    flush();
    // 与 Public 启动一致，交互开始前首次地址不再作为查询回退。
    initial.expire();
    return { dispose, router, run: <T>(task: () => T) => runWithOwner(owner, task) };
  });

  // 只查看已装配的小区块。条件为假的子节点即使被 JSX 替身记录，也不能参与可见性断言。
  const visible = (tree: unknown): ReadPageElement[] =>
    root.run(() => {
      const visibleElements: ReadPageElement[] = [];
      const visit = (node: unknown): void => {
        if (Array.isArray(node)) {
          node.forEach(visit);
          return;
        }
        const element = elements.find((candidate) => candidate === node);
        if (element === undefined || element.props.hidden === true) return;
        const { tag, props, children } = element;
        if (tag === Show) {
          const value = props.when;
          if (!value) visit(props.fallback);
          else
            for (const child of children)
              visit(typeof child === "function" ? child(() => value) : child);
        } else if (typeof tag !== "function") {
          if (typeof tag === "string") visibleElements.push(element);
          children.forEach(visit);
        }
      };
      visit(tree);
      return visibleElements;
    });

  return {
    location,
    writes,
    elements,
    storage,
    loading,
    router: root.router,
    run: root.run,
    visible,
    rejectWrites: fake.rejectWrites,
    back() {
      fake.history.back();
      flush();
    },
    forward() {
      fake.history.forward();
      flush();
    },
    dispose() {
      root.dispose();
      flush();
      RouterContext.defaultValue = oldRouter;
      InitialPublicLocationContext.defaultValue = oldInitial;
      PublicRefreshGenerationContext.defaultValue = oldRefresh;
      PublicBidRefreshContext.defaultValue = oldBidRefresh;
      PublicPageRealtimeTargetContext.defaultValue = oldPageTarget;
      PublicContentLoadingContext.defaultValue = oldLoading;
      WorldSwitchContext.defaultValue = oldWorld;
      restore();
      if (previousJsx === undefined) Reflect.deleteProperty(globalThis, "jsx");
      else Object.defineProperty(globalThis, "jsx", previousJsx);
    },
  };
}
