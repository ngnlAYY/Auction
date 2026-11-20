// 用内存字段与局部语言会话验证语言切换只在译文生效后纠偏。

import { expect, test } from "bun:test";
import { createCell } from "@auction/web-shared/reactive/cell";
import type { LocaleCode, LocaleError } from "@auction/web-shared/ui/i18n";
import { type LocaleSession, LocaleSessionContext } from "@auction/web-shared/ui/i18n/session";
import { createRoot, flush } from "solid-js";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { installAdminTranslation } from "../admin-locale-fixture.test";
import {
  createComponent,
  mountNode,
  required,
  TestDocument,
  type TestElement,
} from "../solid-dom-fixture";
import { createSetupDraft, setupDraftSet, setupFieldId } from "./draft";
import type { SetupIssue } from "./validation";

interface LocaleSnapshot {
  readonly current: LocaleCode;
  readonly selected: LocaleCode;
  readonly error: LocaleError | null;
}

const INITIAL_LOCALE: LocaleSnapshot = { current: "zh-CN", selected: "zh-CN", error: null };
const DRAFT_VALUE = "draft-admin";
const DOM_VALUE = "browser-admin";

async function withField(
  check: (input: TestElement, setLocale: (snapshot: LocaleSnapshot) => void) => void,
): Promise<void> {
  const { SetupInput } = await loadRenderModule<typeof import("./fields")>(
    new URL("./fields.tsx", import.meta.url).pathname,
    "SetupInput",
  );
  const locale = createCell(INITIAL_LOCALE);
  const session: LocaleSession = {
    current: () => locale().current,
    selected: () => locale().selected,
    error: () => locale().error,
    isReady: () => true,
    bootstrap() {},
    switch() {},
  };
  const document = new TestDocument();
  const field = "admin_actor";
  const draft = createCell(setupDraftSet(createSetupDraft(), field, DRAFT_VALUE));
  const issues = createCell<readonly SetupIssue[]>([]);
  const restoreTranslation = installAdminTranslation((id) => id);
  let dispose = (): void => {};
  try {
    createRoot((disposeRoot) => {
      dispose = disposeRoot;
      mountNode(
        document,
        createComponent(LocaleSessionContext, {
          value: session,
          get children() {
            return createComponent(SetupInput, { field, draft, issues, busy: false });
          },
        }),
      );
    });
    flush();
    const input = required(document.getElementById(setupFieldId(field)), "首次设置账号框");
    expect(input.value).toBe(DRAFT_VALUE);
    check(input, (snapshot) => {
      locale.set(snapshot);
      flush();
    });
  } finally {
    try {
      dispose();
      flush();
    } finally {
      restoreTranslation();
    }
  }
}

test("开始加载时保留浏览器静默填入的字段值", async () => {
  await withField((input, setLocale) => {
    // 只写 DOM 值，模拟浏览器尚未派发 input 事件时的恢复或自动填入。
    input.value = DOM_VALUE;
    setLocale({ ...INITIAL_LOCALE, selected: "en" });
    expect(input.value).toBe(DOM_VALUE);
  });
});

test("加载失败时保留浏览器静默填入的字段值", async () => {
  await withField((input, setLocale) => {
    setLocale({ ...INITIAL_LOCALE, selected: "en" });
    input.value = DOM_VALUE;
    setLocale({ ...INITIAL_LOCALE, error: "language-switch-failed" });
    expect(input.value).toBe(DOM_VALUE);
  });
});

test("新语言生效后按草稿纠偏，重复通知同一语言时保留字段值", async () => {
  await withField((input, setLocale) => {
    const english: LocaleSnapshot = { current: "en", selected: "en", error: null };
    setLocale({ ...INITIAL_LOCALE, selected: "en" });
    input.value = DOM_VALUE;
    setLocale(english);
    expect(input.value).toBe(DRAFT_VALUE);

    input.value = DOM_VALUE;
    setLocale({ ...english });
    expect(input.value).toBe(DOM_VALUE);
  });
});
