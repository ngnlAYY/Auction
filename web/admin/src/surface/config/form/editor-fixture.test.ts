// 通过配置 GET 装配实际字段与错误摘要，并保留表单的测试事件入口。

import { expect, test } from "bun:test";
import { createCell } from "@auction/web-shared/reactive/cell";
import { LocaleSessionContext } from "@auction/web-shared/ui/i18n/session";
import { createRoot, flush } from "solid-js";
import { fetchAdminRuntimeConfig } from "../../../api/runtime-config";
import { cleanupScope } from "../../../test-scope-fixture";
import { createElement, installTestDocument, required, setProp } from "../../solid-dom-fixture";
import { configPage } from "../config-page-fixture.test";
import {
  accepted,
  CONFIG_PROJECTION_QUERY,
  configReadReply,
  defaultProjection,
} from "../config-projection-fixture.test";
import { loadConfigFixture } from "../config-render-fixture.test";
import { configLocaleSession, provide } from "../config-test-scope-fixture.test";
import {
  type RuntimeConfigForm,
  runtimeConfigFormEditedInput,
  runtimeConfigFormFromResponse,
  runtimeConfigFormInput,
} from "../model";
import type { FieldIssue } from "../validation";

const fixture = await loadConfigFixture();
const { collectIssues, ErrorSummary, FormIssuesContext } = fixture.validation;

/** 像 Rust 的 read_form 一样先走真实配置 GET，再用解码结果装配表单。 */
export async function readEditorForm(
  projection: ReturnType<typeof defaultProjection>,
): Promise<RuntimeConfigForm> {
  const page = configPage((request) => {
    expect(request.method).toBe("GET");
    expect(request.path).toBe("/api/v1/admin/config");
    expect(new URL(request.url).search).toBe(
      `${CONFIG_PROJECTION_QUERY}&include_worker_restart_state=true`,
    );
    return configReadReply(projection);
  });
  try {
    const decoded = accepted(
      await fetchAdminRuntimeConfig(page.pending.host.sessionHost, new AbortController().signal),
    );
    expect(page.requests).toHaveLength(1);
    return accepted(runtimeConfigFormFromResponse(decoded));
  } finally {
    page.dispose();
  }
}

/** 对应 Rust 的 EditorFields，只装配实际字段和收集错误的表单提交入口。 */
export function renderEditor(model: RuntimeConfigForm) {
  const scope = cleanupScope();
  try {
    const document = installTestDocument();
    scope.defer(document.restore);
    const modelCell = createCell(model);
    const issues = createCell<readonly FieldIssue[]>([]);
    const formNode = createElement("form");
    createRoot((stop) => {
      scope.defer(stop);
      document.add(formNode);
      setProp(formNode, "onSubmit", (event: { preventDefault(): void }) => {
        event.preventDefault();
        issues.set(collectIssues(modelCell.peek()));
      });
      fixture.insert(formNode, () =>
        provide(LocaleSessionContext, configLocaleSession, () =>
          provide(FormIssuesContext, issues, () =>
            fixture.createComponent(fixture.form.RuntimeConfigFields, {
              form: modelCell,
              disabled: false,
              onDirty() {},
            }),
          ),
        ),
      );
      fixture.insert(formNode, () =>
        provide(LocaleSessionContext, configLocaleSession, () =>
          issues().length === 0
            ? null
            : fixture.createComponent(ErrorSummary, {
                get issues() {
                  return issues();
                },
              }),
        ),
      );
    });
    flush();

    return {
      document,
      form: modelCell,
      root: formNode,
      field(path: string) {
        return required(
          document
            .querySelectorAll("input, textarea")
            .find((node) => node.attributes.name === path),
          `字段 ${path}`,
        );
      },
      summaryRow() {
        const summary = required(document.getElementById("runtime-errors"), "提交摘要");
        const rows = summary.querySelectorAll("li");
        expect(rows).toHaveLength(1);
        return required(rows[0], "摘要行");
      },
      submit() {
        expect(formNode.submit()).toBe(true);
        flush();
      },
      expectHistoricalError(path: string, value: string) {
        const field = this.field(path);
        const errorId = `runtime-field-${path.replaceAll(".", "-")}-error`;
        expect(field.value).toBe(value);
        expect(field.getAttribute("aria-invalid")).toBe("true");
        expect(field.getAttribute("aria-describedby")).toBe(errorId);
        expect(required(document.getElementById(errorId), "字段错误").textContent).toContain(
          "之间的整数",
        );
        expect(runtimeConfigFormEditedInput(modelCell.peek(), path)).toBeNull();
      },
      close: scope.dispose,
    };
  } catch (error) {
    scope.dispose();
    throw error;
  }
}

test("编辑器读取夹具以契约样本装配初值并恢复请求替身", async () => {
  const original = globalThis.fetch;
  const result = await readEditorForm(defaultProjection());
  expect(runtimeConfigFormInput(result, "listener.mode")).toBe("tcp");
  expect(globalThis.fetch).toBe(original);
});
