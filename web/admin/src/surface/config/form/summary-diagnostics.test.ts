// 数字错误摘要在真实译文路径下展示完整的字段标签与错误文案。

import { expect, test } from "bun:test";
import { createRoot, flush } from "solid-js";
import { cleanupScope } from "../../../test-scope-fixture";
import { adminBundle } from "../../admin-locale-fixture.test";
import { defaultProjection } from "../config-projection-fixture.test";
import { acquireConfigTranslation } from "../config-test-scope-fixture.test";
import { readEditorForm, renderEditor } from "./editor-fixture.test";

test("static_number_summary_renders_the_full_text_with_real_translations", async () => {
  const scope = cleanupScope();
  try {
    // 带查询标记的真实模块拥有自己的当前语言包，不覆盖其他用例的会话。
    const locale = await import("@auction/web-shared/ui/i18n/session.ts?config-diagnostic");
    createRoot((stop) => {
      scope.defer(stop);
      locale.createLocaleSession(adminBundle("zh-CN"), "zh-CN");
    });
    scope.defer(await acquireConfigTranslation(locale.t));
    const editor = renderEditor(await readEditorForm(defaultProjection()));
    scope.defer(editor.close);
    editor.field("equipment_filter.level.min").input("1001");
    flush();

    editor.submit();
    expect(editor.summaryRow().textContent).toBe(
      "装备收录 / 装备过滤 / 最低等级: 请输入 0 至 1000 之间的整数。",
    );
  } finally {
    scope.dispose();
  }
});
