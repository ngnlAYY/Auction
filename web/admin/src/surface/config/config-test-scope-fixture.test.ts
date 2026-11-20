// 配置用例隔离共享的译文入口，并在用例退出时恢复原实现。

import { expect, test } from "bun:test";
import { createCell } from "@auction/web-shared/reactive/cell";
import type { LocaleCode } from "@auction/web-shared/ui/i18n";
import type { LocaleSession, t } from "@auction/web-shared/ui/i18n/session";
import {
  type Context,
  createComponent,
  createContext,
  createRoot,
  type Element,
  flush,
  getObserver,
  useContext,
} from "solid-js";
import { cleanupScope } from "../../test-scope-fixture";
import { createAdminTranslator, installAdminTranslation } from "../admin-locale-fixture.test";
import { createElement, mountNode, TestDocument } from "../solid-dom-fixture";

let releaseCurrent: (() => void) | null = null;
let queue = Promise.resolve();
const language = createCell<LocaleCode>("zh-CN");

/** 接入真实 FTL，并在同一用例内支持响应式语言切换。 */
export function setConfigLocale(locale: LocaleCode): void {
  language.set(locale);
}

/** 由测试局部提供的语言会话。 */
export const configLocaleSession: LocaleSession = {
  current: language,
  selected: language,
  isReady: () => true,
  error: () => null,
  bootstrap() {},
  switch() {},
};

/** 串行安装真实 FTL 的局部翻译入口，返回本次用例的恢复函数。 */
export async function acquireConfigTranslation(translateMessage?: typeof t): Promise<() => void> {
  const { promise, resolve } = Promise.withResolvers<void>();
  const previous = queue;
  queue = promise;
  await previous;
  const scope = cleanupScope();
  scope.defer(resolve);
  try {
    const priorLanguage = language.peek();
    scope.defer(() => setConfigLocale(priorLanguage));
    setConfigLocale("zh-CN");
    scope.defer(
      installAdminTranslation(
        translateMessage ??
          createAdminTranslator(() => (getObserver() === null ? language.peek() : language())),
      ),
    );
    return scope.dispose;
  } catch (error) {
    scope.dispose();
    throw error;
  }
}

/** 为普通配置用例安装译文替身，不改共享语言包。 */
export async function beginConfigTestLock(): Promise<void> {
  releaseCurrent = await acquireConfigTranslation();
}

/** 恢复译文入口后再允许下一个用例安装自己的替身。 */
export function endConfigTestLock(): void {
  const release = releaseCurrent;
  releaseCurrent = null;
  release?.();
}

/** 通过真实 Provider 提供上下文，测试间不改动默认值。 */
export function provide<T>(context: Context<T>, value: T, render: () => Element): Element {
  return createComponent(context, {
    value,
    get children() {
      return render();
    },
  });
}

test("配置作用域夹具使子节点读取局部上下文，外部仍使用原值", () => {
  const context = createContext("outer");
  const document = new TestDocument();
  createRoot((dispose) => {
    try {
      mountNode(document, () =>
        provide(context, "inner", () => createElement("span", { id: useContext(context) })),
      );
      flush();
      expect(document.querySelector("span")?.id).toBe("inner");
      expect(useContext(context)).toBe("outer");
    } finally {
      dispose();
    }
  });
});
