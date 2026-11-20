// 仪表盘、告警及未决面板的译文、真实控件事件与可控轮询时钟。

import { expect, spyOn, test } from "bun:test";
import { LocaleSessionContext } from "@auction/web-shared/ui/i18n";
import { createComponent, createRoot } from "solid-js";
import { adminLocaleSession, installAdminTranslation } from "../admin-locale-fixture.test";
import {
  createElement,
  insert,
  installTestDocument,
  settle,
  type TestElement,
} from "../solid-dom-fixture";

/** 挂载一次，测试完成或异常时依次释放响应式作用域、文档与翻译替身。 */
export async function withT11View<T>(
  children: () => ReturnType<typeof LocaleSessionContext>,
  run: (tree: TestElement, document: ReturnType<typeof installTestDocument>) => Promise<T> | T,
): Promise<T> {
  const restoreLocale = installAdminTranslation();
  try {
    const document = installTestDocument();
    try {
      const tree = createElement("div");
      let dispose = (): void => {};
      try {
        createRoot((disposeRoot) => {
          dispose = disposeRoot;
          insert(
            tree,
            createComponent(LocaleSessionContext, {
              value: adminLocaleSession,
              get children() {
                return children();
              },
            }),
          );
        });
        document.add(tree);
        await settleT11View();
        return await run(tree, document);
      } finally {
        dispose();
        await settleT11View();
      }
    } finally {
      document.restore();
    }
  } finally {
    restoreLocale();
  }
}

/** 排空微任务与 Solid 更新，无真实定时等待。 */
export async function settleT11View(): Promise<void> {
  await settle(64);
}

/** 调用编译后节点绑定的 DOM 事件，缺少绑定即让用例失败。 */
export function fireT11Event(
  element: TestElement,
  name: "onClick" | "onChange" | "onBlur" | "onInput",
  value?: string,
): void {
  if (value !== undefined) {
    element.value = value;
  }
  const handler = element.attributes[name];
  if (typeof handler !== "function") {
    throw new Error(`测试控件 <${element.tag}> 没有 ${name} 处理器`);
  }
  handler({ currentTarget: element, target: element, preventDefault() {}, stopPropagation() {} });
}

/** 找到恰好一个节点，避免重复控件时误触发其他分支。 */
export function oneT11Element(elements: readonly TestElement[], label: string): TestElement {
  expect(elements, label).toHaveLength(1);
  const element = elements[0];
  if (element === undefined) {
    throw new Error(`未找到控件：${label}`);
  }
  return element;
}

/** 内存计时器由用例推进指定延时，取消后的任务不会被唤醒。 */
export async function installT11Clock() {
  interface Waiter {
    readonly milliseconds: number;
    finish(completed: boolean): void;
  }
  const waiting = new Set<Waiter>();
  const timer = await import("@auction/web-shared/platform/timer");
  const sleep = spyOn(timer, "sleep").mockImplementation((milliseconds, signal) => {
    if (signal?.aborted) {
      return Promise.resolve(false);
    }
    return new Promise<boolean>((resolveWait) => {
      const abort = () => waiter.finish(false);
      const waiter: Waiter = {
        milliseconds,
        finish(completed) {
          waiting.delete(waiter);
          signal?.removeEventListener("abort", abort);
          resolveWait(completed);
        },
      };
      waiting.add(waiter);
      signal?.addEventListener("abort", abort, { once: true });
    });
  });
  return {
    pending(milliseconds: number): number {
      return [...waiting].filter((waiter) => waiter.milliseconds === milliseconds).length;
    },
    async advance(milliseconds: number): Promise<void> {
      const due = [...waiting].filter((waiter) => waiter.milliseconds === milliseconds);
      expect(due.length).toBeGreaterThan(0);
      for (const waiter of due) {
        waiter.finish(true);
      }
      await settleT11View();
    },
    restore() {
      try {
        for (const waiter of waiting) {
          waiter.finish(false);
        }
      } finally {
        sleep.mockRestore();
      }
    },
  };
}

test("仪表盘时钟夹具只唤醒指定延时，并从队列移除已取消的等待", async () => {
  const timer = await import("@auction/web-shared/platform/timer");
  const original = timer.sleep;
  const clock = await installT11Clock();
  const controller = new AbortController();
  try {
    const due = timer.sleep(10);
    const cancelled = timer.sleep(10, controller.signal);
    const later = timer.sleep(20);
    expect(clock.pending(10)).toBe(2);
    controller.abort();
    expect(await cancelled).toBe(false);
    expect(clock.pending(10)).toBe(1);
    await clock.advance(10);
    expect(await due).toBe(true);
    expect(clock.pending(10)).toBe(0);
    expect(clock.pending(20)).toBe(1);
    await clock.advance(20);
    expect(await later).toBe(true);
  } finally {
    clock.restore();
  }
  expect(timer.sleep).toBe(original);
});
