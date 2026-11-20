// 数字控件在实际播报节点中保留字段隔离，并按交付时的语言翻译。

import { afterEach, beforeEach, expect, jest, test } from "bun:test";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createRoot, flush } from "solid-js";
import { cleanupScope } from "../../../test-scope-fixture";
import { installTestDocument, required } from "../../solid-dom-fixture";
import { loadConfigFixture } from "../config-render-fixture.test";
import {
  beginConfigTestLock,
  endConfigTestLock,
  provide,
  setConfigLocale,
} from "../config-test-scope-fixture.test";
import type { FieldIssue } from "../validation";

const fixture = await loadConfigFixture();
const { NumberInput, FieldAnnouncerContext } = fixture.number;
const { useFieldAnnouncer, FieldLiveRegion } = fixture.announcer;
const { FormIssuesContext } = fixture.validation;

function withNumberPair(check: (page: ReturnType<typeof numberPair>) => void): void {
  const scope = cleanupScope();
  try {
    jest.useFakeTimers();
    scope.defer(() => jest.useRealTimers());
    const document = installTestDocument();
    scope.defer(document.restore);
    const page = createRoot((stop) => {
      scope.defer(stop);
      return numberPair(document);
    });
    flush();
    check(page);
  } finally {
    scope.dispose();
  }
}

function numberPair(document: ReturnType<typeof installTestDocument>) {
  const first = createCell("11");
  const second = createCell("0");
  const issues = createCell<readonly FieldIssue[]>([]);
  const announcer = useFieldAnnouncer();
  const root = fixture.createRootElement();
  fixture.insert(root, () =>
    provide(FormIssuesContext, issues, () =>
      provide(FieldAnnouncerContext, announcer, () => [
        fixture.createComponent(NumberInput, {
          path: "first",
          label: "数字一",
          minimum: "0",
          maximum: "10",
          original: "11",
          get value() {
            return first();
          },
          disabled: false,
          onInput: first.set,
        }),
        fixture.createComponent(NumberInput, {
          path: "second",
          label: "数字二",
          minimum: "0",
          maximum: "20",
          original: "0",
          get value() {
            return second();
          },
          disabled: false,
          onInput: second.set,
        }),
        fixture.createComponent(FieldLiveRegion, { announcer }),
      ]),
    ),
  );
  document.add(root);
  return {
    field(path: string) {
      return required(
        document
          .querySelectorAll("[data-runtime-number]")
          .find((node) => node.attributes["data-runtime-number"] === path),
        "数字输入",
      );
    },
    live() {
      return required(document.querySelectorAll("[aria-live]")[0], "播报区").textContent;
    },
  };
}

function deliver(): void {
  jest.advanceTimersByTime(500);
  flush();
}

beforeEach(beginConfigTestLock);
afterEach(endConfigTestLock);

test("correcting_a_number_only_cancels_its_own_pending_announcement", () => {
  for (const corrected of ["first", "second"]) {
    withNumberPair(({ field, live }) => {
      const second = field("second");
      second.input("21");
      flush();
      second.blur();
      flush();
      expect(second.getAttribute("aria-invalid")).toBe("true");
      expect(live()).toBe("");
      expect(jest.getTimerCount()).toBe(1);
      field(corrected).input("1");
      flush();
      expect(jest.getTimerCount()).toBe(corrected === "first" ? 1 : 0);
      deliver();
      expect(live()).toBe(corrected === "first" ? "请输入 0 至 20 之间的整数。" : "");
      expect(jest.getTimerCount()).toBe(0);
    });
  }
});

test("pending_number_announcement_uses_the_language_at_delivery", () => {
  withNumberPair(({ field, live }) => {
    const second = field("second");
    second.input("21");
    flush();
    second.blur();
    flush();
    expect(second.getAttribute("aria-invalid")).toBe("true");
    expect(live()).toBe("");
    expect(jest.getTimerCount()).toBe(1);
    setConfigLocale("ja");
    flush();
    deliver();
    expect(live()).toBe("0 から 20 までの整数を入力してください。");
    expect(live()).not.toContain("请输入");
  });
});
