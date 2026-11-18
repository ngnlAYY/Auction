// 分页提示 ID 在没有安全上下文 API 时也能区分组件实例。

import { expect, test } from "bun:test";
import { createRoot } from "solid-js";

import { PageSizePicker } from "./size";

test("没有 randomUUID 时两个分页控件各自关联同一实例的提示", () => {
  const crypto = globalThis.crypto;
  const randomUuid = Object.getOwnPropertyDescriptor(crypto, "randomUUID");
  const jsx = Object.getOwnPropertyDescriptor(globalThis, "jsx");
  const hints: string[] = [];
  const references: string[] = [];
  Object.defineProperty(crypto, "randomUUID", { configurable: true, value: undefined });
  Object.defineProperty(globalThis, "jsx", {
    configurable: true,
    value: (tag: unknown, props: Record<string, unknown> | null) => {
      if (tag === "span" && typeof props?.id === "string") {
        hints.push(props.id);
      }
      if (tag === "input" && typeof props?.["aria-describedby"] === "string") {
        references.push(props["aria-describedby"]);
      }
      return null;
    },
  });
  try {
    createRoot((dispose) => {
      try {
        const props = {
          pageSize: 10,
          options: [10, 20],
          label: "每页数量",
          allowCustom: true,
          customLabel: "自定义数量",
          applyLabel: "应用",
          invalidLabel: "数量无效",
        };
        PageSizePicker(props);
        PageSizePicker(props);
        expect(new Set(hints).size).toBe(2);
        expect(references).toEqual(hints);
      } finally {
        dispose();
      }
    });
  } finally {
    if (randomUuid === undefined) {
      Reflect.deleteProperty(crypto, "randomUUID");
    } else {
      Object.defineProperty(crypto, "randomUUID", randomUuid);
    }
    if (jsx === undefined) {
      Reflect.deleteProperty(globalThis, "jsx");
    } else {
      Object.defineProperty(globalThis, "jsx", jsx);
    }
  }
});
