// 认证测试的内存文档、属性恢复与受控定时器。

import { flush } from "solid-js";
import { preserveProperty } from "../../test-scope-fixture";
import { installTestDocument, TestDocument } from "../test-dom-fixture";

/** 带会话 Cookie 与页面级事件监听的内存文档。 */
export class AuthTestDocument extends TestDocument {
  readonly visibilityState = "visible";
  readonly documentElement = null;
  readonly body = null;
  private readonly events = new EventTarget();
  readonly addEventListener = this.events.addEventListener.bind(this.events);
  readonly removeEventListener = this.events.removeEventListener.bind(this.events);

  constructor(public cookie: string) {
    super();
  }
}

/** 在挂载前安装内存文档；任一个属性安装或恢复失败也会清理另一项。 */
export function installAuthDocument(cookie = "") {
  const document = installTestDocument(new AuthTestDocument(cookie));
  return { document, restore: document.restore };
}

const realSetTimeout = globalThis.setTimeout;

/** 等一轮真实宏任务，并提交 Solid 排队的状态更新。 */
export async function settleAuth(): Promise<void> {
  await new Promise((resolve) => realSetTimeout(resolve, 0));
  flush();
}

/** 在用例内安装定时器，按精确延时手动派发并恢复原属性。 */
export function installManualTimers() {
  const timers = new Map<number, { callback: () => void; delay: number }>();
  const restoreSet = preserveProperty(globalThis, "setTimeout");
  const restoreClear = preserveProperty(globalThis, "clearTimeout");
  let next = 1;
  const restore = () => {
    try {
      restoreClear();
    } finally {
      restoreSet();
    }
  };
  try {
    Object.defineProperty(globalThis, "setTimeout", {
      configurable: true,
      writable: true,
      value: (callback: TimerHandler, delay = 0, ...args: unknown[]): number => {
        if (typeof callback !== "function") {
          throw new Error("定时器替身只接收函数");
        }
        const id = next++;
        timers.set(id, { callback: () => callback(...args), delay });
        return id;
      },
    });
    Object.defineProperty(globalThis, "clearTimeout", {
      configurable: true,
      writable: true,
      value: (id: number | undefined) => {
        if (id !== undefined) timers.delete(id);
      },
    });
    return {
      smallestDelay(): number | null {
        const delays = Array.from(timers.values(), ({ delay }) => delay).filter(
          (delay) => delay > 0,
        );
        return delays.length === 0 ? null : Math.min(...delays);
      },
      fire(delay: number): void {
        const due = [...timers.entries()].filter(([, timer]) => timer.delay === delay);
        if (due.length === 0) {
          throw new Error(`没有延时 ${delay} 的定时器`);
        }
        for (const [id, timer] of due) {
          timers.delete(id);
          timer.callback();
        }
      },
      restore,
    };
  } catch (error) {
    restore();
    throw error;
  }
}
