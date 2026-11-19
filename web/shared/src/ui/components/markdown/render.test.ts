// 将实际 Markdown 表格组件挂载到可改变横向尺寸的 DOM 替身。

import { expect, test } from "bun:test";
import { createRoot, For, flush, Match, Switch } from "solid-js";

import { parseBlocks } from "./blocks";
import { MarkdownBlockView } from "./render";

type FixtureProps = Record<string, unknown>;

test("只有正在横向滚动的 Markdown 表格容器可聚焦并拥有名称", () => {
  const attributes = new Map<string, string>();
  const wrapper = {
    clientWidth: 250,
    scrollWidth: 250,
    firstElementChild: {},
    setAttribute(name: string, value: string) {
      attributes.set(name, value);
    },
    removeAttribute(name: string) {
      attributes.delete(name);
    },
  };
  let overflowX = "auto";
  const window = Object.assign(new EventTarget(), { getComputedStyle: () => ({ overflowX }) });
  const globals = {
    window,
    ResizeObserver: undefined,
    jsx(tag: unknown, props: FixtureProps | null, ...children: unknown[]) {
      if (tag === Match) return { when: props?.when, child: children[0] };
      if (tag === Switch) {
        for (const match of children as { when: unknown; child: unknown }[]) {
          if (match.when && typeof match.child === "function") return match.child(() => match.when);
        }
        return null;
      }
      if (tag === For) return null;
      if (typeof tag === "function") return tag(props ?? {});
      if (tag === "div" && props?.class === "shared-markdown__table-wrap") {
        if (typeof props.ref === "function") props.ref(wrapper);
        for (const name of ["tabindex", "role", "aria-label"]) {
          if (props[name] !== undefined) wrapper.setAttribute(name, String(props[name]));
        }
      }
      return null;
    },
  };
  const previousGlobals = Object.keys(globals).map(
    (key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)] as const,
  );
  for (const [key, value] of Object.entries(globals)) {
    Object.defineProperty(globalThis, key, { configurable: true, value });
  }
  const block = parseBlocks("| **Item** | Amount |\n| --- | --- |\n| Long plain text | 10 |")[0];
  if (block?.kind !== "table") throw new Error("缺少 Markdown 表格");
  const dispose = createRoot((dispose) => {
    MarkdownBlockView({ block });
    return dispose;
  });
  try {
    flush();
    expect(attributes.size).toBe(0);

    wrapper.scrollWidth = 340;
    window.dispatchEvent(new Event("resize"));
    flush();
    expect(attributes.get("tabindex")).toBe("0");
    expect(attributes.get("role")).toBe("region");
    expect(attributes.get("aria-label")).toBe("Item, Amount");

    overflowX = "clip";
    window.dispatchEvent(new Event("resize"));
    flush();
    expect(attributes.size).toBe(0);
  } finally {
    dispose();
    for (const [name, descriptor] of previousGlobals) {
      if (descriptor === undefined) Reflect.deleteProperty(globalThis, name);
      else Object.defineProperty(globalThis, name, descriptor);
    }
  }
});
