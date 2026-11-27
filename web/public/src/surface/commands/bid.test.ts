// 普通出价的原生词法约束和表单金额校验。

import { expect, test } from "bun:test";
import { useConfirmCommand } from "@auction/web-shared/net/commands";
import { createCell } from "@auction/web-shared/reactive/cell";
import { children, createRoot, Show, untrack } from "solid-js";

import type { CommandError } from "../../api/commands";
import { PublicRefreshGenerationContext } from "../board/refresh";
import { BID_UNITS_PATTERN, BidForm, type BidFormInput, parseBidUnits } from "./bid";
import { type PendingBidCommand, PublicCommandIdentityContext } from "./pending";

const nativePattern = new RegExp(`^(?:${BID_UNITS_PATTERN})$`, "v");

test("原生约束与解析都允许首尾 ASCII 空白", () => {
  expect(BID_UNITS_PATTERN).toBe(String.raw`[ \x09-\x0D]*[0-9]*[ \x09-\x0D]*`);
  for (const [input, amount] of [
    [" 50 ", 50_000],
    ["\t\n\v0050\v\f\r", 50_000],
    ["1000000", 1_000_000_000],
  ] as const) {
    expect(nativePattern.test(input)).toBe(true);
    expect(parseBidUnits(input)).toBe(amount);
  }
});

test("原生约束与解析都拒绝非法金额", () => {
  for (const input of [
    "1,000",
    "1.5",
    "５００",
    "5 0",
    "5\v0",
    "\u00a050",
    "50\u00a0",
    "\u300050",
    "50\u3000",
  ]) {
    expect(nativePattern.test(input)).toBe(false);
    expect(parseBidUnits(input)).toBeNull();
  }
});

test("空值与超出数值范围由表单校验处理", () => {
  for (const input of ["", " \t\v ", "0", "1000001"]) {
    expect(nativePattern.test(input)).toBe(true);
    expect(parseBidUnits(input)).toBeNull();
  }
});

test("长空白只允许出现在金额首尾", () => {
  const whitespace = " ".repeat(30_000);
  expect(parseBidUnits(`5${whitespace}0`)).toBeNull();
  expect(parseBidUnits(`${whitespace}\v50${whitespace}`)).toBe(50_000);
});

class CapturedElement {
  constructor(
    readonly tag: string,
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
  if (typeof tag === "string") {
    return new CapturedElement(tag, props, nodes);
  }
  const child = nodes.length === 1 ? nodes[0] : nodes;
  if (tag === Show) {
    const when = props.when;
    return when ? (typeof child === "function" ? child(() => when) : child) : props.fallback;
  }
  if (typeof tag === "function") {
    return tag({ ...props, children: child });
  }
  return nodes;
}

function renderedElements(node: unknown): CapturedElement[] {
  if (node instanceof CapturedElement) {
    return [node, ...node.children.flatMap(renderedElements)];
  }
  return Array.isArray(node) ? node.flatMap(renderedElements) : [];
}

function renderForm(input: BidFormInput): CapturedElement[] {
  const original = Object.getOwnPropertyDescriptor(globalThis, "jsx");
  if (original === undefined) {
    throw new Error("测试 JSX 转译未装配");
  }
  // 收集实际 JSX 的当帧节点及属性，按 Show 的条件选择这一帧可见的分支。
  Object.defineProperty(globalThis, "jsx", { ...original, value: captureJsx });
  try {
    let elements: CapturedElement[] = [];
    const fixture = PublicRefreshGenerationContext({
      value: createCell(0),
      get children() {
        return PublicCommandIdentityContext({
          value: { userId: "viewer" },
          get children() {
            elements = untrack(() => renderedElements(BidForm({ input })));
            return null;
          },
        });
      },
    });
    children(() => fixture)();
    return elements;
  } finally {
    Object.defineProperty(globalThis, "jsx", original);
  }
}

test("二次确认时晚到的活动授权禁用确认按钮，并关联两个实际提示节点", () => {
  createRoot((dispose) => {
    try {
      const command = useConfirmCommand<PendingBidCommand, CommandError>();
      const blockedReason = createCell<string | null>(null);
      const render = () =>
        renderForm({
          lotId: "lot-1",
          nextBid: 126_000,
          lotStatus: "live",
          canBid: true,
          canSubmit: true,
          blocked: blockedReason.peek() !== null,
          blockedReason: blockedReason.peek() ?? "",
          command,
          onSubmit: () => {},
          onClose: () => {},
        });
      const isSubmitButton = (node: CapturedElement) =>
        node.tag === "button" && node.attributes.type === "submit";

      const edit = render();
      const submit = edit.find((node) => node.tag === "form")?.attributes.onSubmit;
      if (typeof submit !== "function") {
        throw new Error("表单未绑定第一次提交事件");
      }
      submit({ preventDefault() {} });
      const pending = command.confirmation.peek();
      expect(pending?.amount).toBe(126_000);
      const confirming = render().find(isSubmitButton);
      expect(confirming?.attributes.disabled).toBe(false);
      expect(confirming?.attributes["aria-describedby"]).toBe("public-bid-confirmation");

      blockedReason.set("bid-limit-note");
      const locked = render();
      const button = locked.find(isSubmitButton);
      expect(command.confirmation.peek()).toBe(pending);
      expect(button?.attributes.disabled).toBe(true);
      const description = button?.attributes["aria-describedby"];
      expect(description).toBe("public-bid-confirmation public-bid-blocked-reason");
      if (typeof description !== "string") {
        throw new Error("禁用的确认按钮未绑定提示");
      }
      const targets = description
        .split(" ")
        .map((id) => locked.find((node) => node.attributes.id === id));
      expect(targets.map((node) => node?.tag)).toEqual(["p", "p"]);
      expect(targets[1]?.children).toEqual(["bid-limit-note"]);
    } finally {
      dispose();
    }
  });
});

test("已有生效代理时普通出价被禁用，但不渲染空的说明节点，确认按钮也不引用它", () => {
  createRoot((dispose) => {
    try {
      const command = useConfirmCommand<PendingBidCommand, CommandError>();
      const render = () =>
        renderForm({
          lotId: "lot-1",
          nextBid: 126_000,
          lotStatus: "live",
          canBid: true,
          canSubmit: true,
          blocked: true,
          blockedReason: "",
          command,
          onSubmit: () => {},
          onClose: () => {},
        });
      const isSubmitButton = (node: CapturedElement) =>
        node.tag === "button" && node.attributes.type === "submit";

      const edit = render();
      expect(edit.find(isSubmitButton)?.attributes.disabled).toBe(true);
      expect(
        edit.find((node) => node.attributes.id === "public-bid-blocked-reason"),
      ).toBeUndefined();
      expect(edit.find(isSubmitButton)?.attributes["aria-describedby"]).toBeUndefined();
    } finally {
      dispose();
    }
  });
});
