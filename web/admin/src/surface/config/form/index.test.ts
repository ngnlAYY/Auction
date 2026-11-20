// 通过实际配置字段和表单事件核对历史数值、修复控件及跨语言错误摘要。

import { afterEach, beforeEach, expect, test } from "bun:test";
import { flush } from "solid-js";
import { accepted, defaultProjection, objectAt, valueAt } from "../config-projection-fixture.test";
import {
  beginConfigTestLock,
  endConfigTestLock,
  setConfigLocale,
} from "../config-test-scope-fixture.test";
import { runtimeConfigFormSetBoolean, runtimeConfigFormSetInput } from "../model";
import { runtimeConfigFormToDocument } from "../model/document";
import { colorLevelPath } from "../validation";
import { readEditorForm, renderEditor } from "./editor-fixture.test";

function colorRule(minimum: number) {
  return {
    category: "Cloth",
    equipment_type: "Phase",
    quality: "Legendary",
    level: { min: minimum, max: 500 },
    prefixes: ["Radiant"],
    suffixes: [],
    prefix_color: "#ff0000",
    suffix_color: "#00aa00",
    combined_color: "#aa00aa",
  };
}

function checkUnrelatedEdit(editor: ReturnType<typeof renderEditor>, document: unknown): void {
  const expected = structuredClone(document);
  const metrics = objectAt(expected, "metrics");
  expect(typeof metrics.enabled).toBe("boolean");
  metrics.enabled = !metrics.enabled;
  editor.form.update((model) =>
    runtimeConfigFormSetBoolean(model, "metrics.enabled", metrics.enabled === true),
  );
  expect<unknown>(accepted(runtimeConfigFormToDocument(editor.form.peek()))).toEqual(expected);
}

beforeEach(beginConfigTestLock);
afterEach(endConfigTestLock);

test("submitted_color_level_summary_retranslates_without_resubmitting", async () => {
  const projection = defaultProjection();
  objectAt(projection.data.document, "equipment_colors").rules = [colorRule(0)];
  const editor = renderEditor(await readEditorForm(projection));
  try {
    const input = editor.field(colorLevelPath(0, "min"));
    input.input("1001");
    flush();
    expect(input.getAttribute("aria-invalid")).toBe("false");
    editor.submit();
    const row = editor.summaryRow();
    const mutations = editor.document.watchMounts(row);
    expect(row.textContent).toBe("规则 1 / 最低等级: 请输入 0 至 1000 之间的整数。");
    setConfigLocale("ja");
    flush();
    expect(editor.summaryRow()).toBe(row);
    expect(mutations).toEqual([]);
    expect(row.textContent).toBe(
      "ルール 1 / 最小レベル: 0 から 1000 までの整数を入力してください。",
    );
    expect(input.getAttribute("aria-invalid")).toBe("true");
  } finally {
    editor.close();
  }
});

test("static_number_summary_starts_with_the_displayed_label_and_retranslates", async () => {
  const path = "equipment_filter.level.min";
  const editor = renderEditor(await readEditorForm(defaultProjection()));
  try {
    const input = editor.field(path);
    input.input("1001");
    flush();
    expect(input.getAttribute("aria-invalid")).toBe("false");
    editor.submit();
    const row = editor.summaryRow();
    const mutations = editor.document.watchMounts(row);
    expect(row.textContent).toBe("装备收录 / 装备过滤 / 最低等级: 请输入 0 至 1000 之间的整数。");
    expect(row.textContent).not.toContain(path);
    setConfigLocale("ja");
    flush();
    expect(editor.summaryRow()).toBe(row);
    expect(mutations).toEqual([]);
    expect(row.textContent).toBe(
      "装備受付 / 装備フィルター / 最小レベル: 0 から 1000 までの整数を入力してください。",
    );
  } finally {
    editor.close();
  }
});

test("negative_threshold_is_rendered_with_a_field_error_and_round_trips_unedited", async () => {
  const projection = defaultProjection();
  const alerts = objectAt(projection.data.document, "operations.alerts");
  alerts.managed = true;
  alerts.delivery_pending_count_threshold = -1;
  const path = "operations.alerts.delivery_pending_count_threshold";
  const editor = renderEditor(await readEditorForm(projection));
  try {
    editor.expectHistoricalError(path, "-1");
    checkUnrelatedEdit(editor, projection.data.document);
    editor.form.update((model) => runtimeConfigFormSetInput(model, path, "12"));
    expect(valueAt(accepted(runtimeConfigFormToDocument(editor.form.peek())), path)).toBe(12);
  } finally {
    editor.close();
  }
});

test("nonnegative_historical_number_outside_the_range_is_marked_and_preserved", async () => {
  const projection = defaultProjection();
  objectAt(projection.data.document, "tracing").queue_capacity = 0;
  const editor = renderEditor(await readEditorForm(projection));
  try {
    editor.expectHistoricalError("tracing.queue_capacity", "0");
    checkUnrelatedEdit(editor, projection.data.document);
  } finally {
    editor.close();
  }
});

test("conflicting_color_selections_and_extra_mailboxes_open_with_repair_controls", async () => {
  const projection = defaultProjection();
  objectAt(projection.data.document, "equipment_colors").rules = [
    { ...colorRule(0), categories: ["Light"] },
  ];
  objectAt(projection.data, "document").dashboard_mailboxes = [
    { world_id: "main", mailbox_key: "mail-1" },
    { world_id: "isekai", mailbox_key: "mail-2" },
    { world_id: "main", mailbox_key: "mail-3" },
  ];
  const model = await readEditorForm(projection);
  expect<unknown>(accepted(runtimeConfigFormToDocument(model))).toEqual(projection.data.document);
  const editor = renderEditor(model);
  try {
    for (const expected of [
      "旧单值和新数组同时存在",
      "保留旧单值",
      "保留新数组",
      "仪表盘邮箱超过允许的",
    ]) {
      expect(editor.root.textContent).toContain(expected);
    }
    expect(editor.root.querySelectorAll("input").map((element) => element.value)).toContain(
      "mail-3",
    );
  } finally {
    editor.close();
  }
});

test("clearing_an_edited_level_waits_for_blur_or_submit_and_valid_input_clears_the_error", async () => {
  for (const trigger of ["blur", "submit"]) {
    const projection = defaultProjection();
    objectAt(projection.data.document, "equipment_colors").rules = [colorRule(-1)];
    const editor = renderEditor(await readEditorForm(projection));
    try {
      const input = editor.field(colorLevelPath(0, "min"));
      expect(input.value).toBe("-1");
      expect(input.getAttribute("aria-invalid")).toBe("true");
      input.input("5");
      flush();
      expect(input.getAttribute("aria-invalid")).toBe("false");
      input.input("");
      flush();
      expect(input.getAttribute("aria-invalid")).toBe("false");
      if (trigger === "submit") {
        editor.submit();
      } else {
        input.blur();
        flush();
      }
      expect(input.getAttribute("aria-invalid")).toBe("true");
      input.input("-2");
      flush();
      expect(input.getAttribute("aria-invalid")).toBe("true");
      input.input("7");
      flush();
      expect(input.getAttribute("aria-invalid")).toBe("false");
    } finally {
      editor.close();
    }
  }
});
