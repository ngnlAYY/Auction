// 验证内存 DOM 的节点移动、下拉框选中状态和真实组件的绑定。

import { expect, test } from "bun:test";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createRoot, flush } from "solid-js";
import { loadRenderModule } from "../solid-render-fixture.test";
import type { FilterValue } from "./realtime/filter-field";
import {
  createComponent,
  createElement,
  insertNode,
  installTestDocument,
  mountNode,
  removeNode,
  required,
  TestDocument,
} from "./solid-dom-fixture";

test("同父节点重插记下移除与插入，移动后只有一个子节点记录", () => {
  const document = installTestDocument();
  try {
    const parent = createElement("div");
    const worker = createElement("section");
    const status = createElement("p");
    const sibling = createElement("section");
    document.add(parent);
    insertNode(worker, status);
    insertNode(parent, worker);
    insertNode(parent, sibling);
    const workerMounts = document.watchMounts(worker);
    const statusMounts = document.watchMounts(status);

    insertNode(parent, worker);

    expect(parent.nodes.map((read) => read())).toEqual([sibling, worker]);
    expect(worker.parentElement).toBe(parent);
    expect(status.parentElement).toBe(worker);
    expect(workerMounts).toEqual(["remove", "insert"]);
    expect(statusMounts).toEqual(["remove", "insert"]);
  } finally {
    document.restore();
  }
});

test("下拉框的默认首项与显式空选可区分，选项移除后重新选择", () => {
  const select = createElement("select");
  const tcp = createElement("option", { value: "tcp" });
  const unix = createElement("option", { value: "unix" });
  expect(select.value).toBe("");
  insertNode(select, tcp);
  insertNode(select, unix);
  expect(select.value).toBe("tcp");

  select.value = "missing";
  expect(select.value).toBe("");
  removeNode(tcp);
  expect(select.value).toBe("unix");
});

test("插入显式选中的选项后立即替换默认选中项", () => {
  const select = createElement("select");
  const tcp = createElement("option", { value: "tcp" });
  const unix = createElement("option", { value: "unix", selected: true });
  insertNode(select, tcp);
  expect(select.value).toBe("tcp");

  insertNode(select, unix, tcp);

  expect(select.value).toBe("unix");
  expect([tcp.selected, unix.selected]).toEqual([false, true]);

  tcp.selected = true;
  expect(select.value).toBe("tcp");
  expect([tcp.selected, unix.selected]).toEqual([true, false]);

  select.value = "unix";
  expect(select.value).toBe("unix");
  expect([tcp.selected, unix.selected]).toEqual([false, true]);

  removeNode(unix);
  expect(select.value).toBe("tcp");
  expect(tcp.selected).toBe(true);
});

test("真实 QueryFilterSelect 的响应式选项写入更新 select.value", async () => {
  const { QueryFilterSelect } = await loadRenderModule<
    Pick<typeof import("./realtime/filters"), "QueryFilterSelect">
  >(new URL("./realtime/filters.tsx", import.meta.url).pathname, "QueryFilterSelect");
  const document = new TestDocument();
  const value = createCell<FilterValue>({ kind: "text", value: "active" });
  createRoot((dispose) => {
    try {
      mountNode(
        document,
        createComponent(QueryFilterSelect, {
          id: "status",
          name: "status",
          label: "状态",
          get value() {
            return value();
          },
          choices: [
            ["active", "执行中", false],
            ["completed", "已完成", false],
          ],
          onChange() {},
        }),
      );
      flush();
      const select = required(document.querySelector("select"), "状态下拉框");
      expect(select.value).toBe("active");

      value.set({ kind: "text", value: "completed" });
      flush();

      expect(select.value).toBe("completed");
      expect(select.querySelectorAll("option").map((option) => option.selected)).toEqual([
        false,
        true,
      ]);
    } finally {
      dispose();
    }
  });
});

test("单选默认跳过禁用的选项和分组，显式选择仍可选中禁用项", () => {
  const select = createElement("select");
  const group = createElement("optgroup", { disabled: true });
  const grouped = createElement("option", { value: "grouped" });
  const disabled = createElement("option", { value: "disabled", disabled: true });
  const available = createElement("option", { value: "available" });
  insertNode(group, grouped);
  insertNode(select, group);
  insertNode(select, disabled);
  expect(select.value).toBe("");
  insertNode(select, available);
  expect(select.value).toBe("available");
  grouped.setAttribute("selected", "");
  expect(select.value).toBe("grouped");
  grouped.selected = false;
  expect(select.value).toBe("available");
  expect(grouped.hasAttribute("selected")).toBe(true);
});

test("多选框允许多个选中项，写 value 时仅选中第一个匹配项", () => {
  const select = createElement("select", { multiple: true });
  const first = createElement("option", { value: "same" });
  const second = createElement("option", { value: "same", selected: true });
  insertNode(select, first);
  expect(select.value).toBe("");
  insertNode(select, second);
  first.selected = true;
  expect([first.selected, second.selected]).toEqual([true, true]);
  select.value = "same";
  expect([first.selected, second.selected]).toEqual([true, false]);
  select.value = "missing";
  expect([first.selected, second.selected]).toEqual([false, false]);
  expect(select.value).toBe("");
});
