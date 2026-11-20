// 共享表格与分页的 Solid 开发期诊断和响应式交互。

import { expect, spyOn, test } from "bun:test";
import { createCell } from "@auction/web-shared/reactive/cell";
import type { JSX } from "@solidjs/web";
import { flush } from "solid-js";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { click, input, okReply, submit, withPage } from "../browser-page-fixture.test";
import { createComponent, enableSolidComponentDiagnostics } from "../solid-dom-fixture";

function diagnostics() {
  const restore = enableSolidComponentDiagnostics();
  const warn = spyOn(console, "warn").mockImplementation(() => {});
  return {
    strictReads() {
      return warn.mock.calls
        .flatMap(([value]) => (typeof value === "string" ? [value] : []))
        .filter(
          (value) =>
            value.startsWith("[STRICT_READ_UNTRACKED]") &&
            /<(TableColumn|Pager|PageSizePicker)>/u.test(value),
        );
    },
    dispose() {
      warn.mockRestore();
      restore();
    },
  };
}

test("开发模式挂载 Admin 用户页时共享列与分页无非跟踪读取诊断", async () => {
  const { page } = await loadRenderModule<Pick<typeof import("../users"), "page">>(
    new URL("../users/index.tsx", import.meta.url).pathname,
    "page",
  );
  const diagnostic = diagnostics();
  try {
    await withPage(
      "/users?page_size=10",
      page,
      ({ method, url }) => {
        expect([method, url.pathname]).toEqual(["GET", "/api/v1/admin/users"]);
        return okReply({ users: [], pagination: { page: 1, page_size: 10, total: 11 } });
      },
      ({ document }) => {
        expect(document.querySelector("col")?.getAttribute("style")).toBe("width: 12.000000%");
        expect(
          document.find((node) => node.tag === "input" && node.getAttribute("name") === "page")
            ?.value,
        ).toBe("1");
        expect(
          document.find(
            (node) => node.tag === "select" && node.getAttribute("name") === "page_size",
          )?.value,
        ).toBe("10");
        expect(diagnostic.strictReads()).toEqual([]);
      },
    );
  } finally {
    diagnostic.dispose();
  }
});

test("开发模式下列宽、页码和页长更新，纠偏只用现时回调而非法数量不提交", async () => {
  const [{ TableColumns }, { Pager }] = await Promise.all([
    loadRenderModule<typeof import("@auction/web-shared/ui/components/table-columns")>(
      new URL("../../../../shared/src/ui/components/table-columns.tsx", import.meta.url).pathname,
      "TableColumns",
    ),
    loadRenderModule<typeof import("@auction/web-shared/ui/components/pager")>(
      new URL("../../../../shared/src/ui/components/pager/index.tsx", import.meta.url).pathname,
      "Pager",
    ),
  ]);
  const widths = createCell<readonly number[]>([1, 2]);
  const page = createCell(2);
  const size = createCell(10);
  const total = createCell(100);
  const attempts: [string, number][] = [];
  const appliedSizes: number[] = [];
  const correct = createCell<(target: number) => boolean>((target) => {
    attempts.push(["旧", target]);
    return false;
  });
  const render = (): JSX.Element => [
    createComponent(TableColumns, {
      layout: "test",
      get widths() {
        return widths();
      },
    }),
    createComponent(Pager, {
      get page() {
        return page();
      },
      get pageSize() {
        return size();
      },
      get total() {
        return total();
      },
      get onPageClamp() {
        return correct();
      },
      label: "分页",
      previousLabel: "上一页",
      nextLabel: "下一页",
      jumpLabel: "跳页",
      jumpButtonLabel: "前往",
      status: ([current, pages]) => `${current}/${pages}`,
      clampFailure: { message: "纠偏失败", retryLabel: "重试纠偏" },
      pageSizeOptions: [10, 20, 50],
      allowCustomPageSize: true,
      maxPageSize: 100,
      pageSizeInvalidLabel: "无效页长",
      onPageChange: (target) => page.set(target),
      onPageSizeChange: (target) => {
        appliedSizes.push(target);
        size.set(target);
      },
    }),
  ];
  const diagnostic = diagnostics();
  try {
    await withPage(
      "/users",
      render,
      () => {
        throw new Error("共享控件不应发请求");
      },
      async ({ document }) => {
        const cols = () => document.querySelectorAll("col").map((col) => col.getAttribute("style"));
        const jump = document.find(
          (node) => node.tag === "input" && node.getAttribute("name") === "page",
        );
        const custom = document.find(
          (node) => node.tag === "input" && node.getAttribute("name") === "custom_page_size",
        );
        const form = document.querySelector(".ui-pager__custom");
        if (jump === null || custom === null || form === null) throw new Error("缺少分页表单");
        expect(cols()).toEqual(["width: 33.333333%", "width: 66.666667%"]);
        expect([jump.value, custom.value]).toEqual(["2", "10"]);

        widths.set([1, 3]);
        page.set(3);
        size.set(20);
        flush();
        expect(cols()).toEqual(["width: 25.000000%", "width: 75.000000%"]);
        expect([jump.value, custom.value]).toEqual(["3", "20"]);
        expect(attempts).toEqual([]);

        total.set(5);
        flush();
        expect(attempts).toEqual([["旧", 1]]);
        expect(jump.value).toBe("3");
        correct.set((target) => {
          attempts.push(["新", target]);
          page.set(target);
          return true;
        });
        flush();
        expect(attempts).toEqual([["旧", 1]]);
        await click(document, "重试纠偏");
        expect(attempts).toEqual([
          ["旧", 1],
          ["新", 1],
        ]);
        expect(jump.value).toBe("1");

        await input(custom, "101");
        await submit(form);
        expect(custom.getAttribute("aria-invalid")).toBe("true");
        expect(appliedSizes).toEqual([]);
        await input(custom, "37");
        await submit(form);
        expect(custom.getAttribute("aria-invalid")).toBe("false");
        expect(custom.value).toBe("37");
        expect(appliedSizes).toEqual([37]);
        expect(diagnostic.strictReads()).toEqual([]);
      },
    );
  } finally {
    diagnostic.dispose();
  }
});
