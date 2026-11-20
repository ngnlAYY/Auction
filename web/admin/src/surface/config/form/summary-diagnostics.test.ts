// 数字错误摘要在真实译文路径下展示完整的字段标签与错误文案。

import { expect, spyOn, test } from "bun:test";
import { createRoot, flush } from "solid-js";
import { cleanupScope } from "../../../test-scope-fixture";
import { adminBundle, adminFtl } from "../../admin-locale-fixture.test";
import { removeNode } from "../../solid-dom-fixture";
import { settle } from "../config-page-fixture.test";
import { defaultProjection } from "../config-projection-fixture.test";
import { acquireConfigTranslation, setConfigLocale } from "../config-test-scope-fixture.test";
import { readEditorForm, renderEditor } from "./editor-fixture.test";

test("static_number_summary_renders_the_full_text_with_real_translations", async () => {
  const scope = cleanupScope();
  try {
    // 带查询标记的真实模块拥有自己的当前语言包，不覆盖其他用例的会话。
    const locale = await import("@auction/web-shared/ui/i18n/session.ts?config-diagnostic");
    const language = createRoot((stop) => {
      scope.defer(stop);
      return locale.createLocaleSession(adminBundle("zh-CN"), "zh-CN");
    });
    scope.defer(await acquireConfigTranslation(locale.t));
    const editor = renderEditor(await readEditorForm(defaultProjection()));
    scope.defer(editor.close);
    const warn = spyOn(console, "warn");
    scope.defer(() => warn.mockRestore());
    const input = editor.field("equipment_filter.level.min");
    input.input("1001");
    flush();

    editor.submit();
    const row = editor.summaryRow();
    expect(row.textContent).toBe("装备收录 / 装备过滤 / 最低等级: 请输入 0 至 1000 之间的整数。");
    setConfigLocale("ja");
    language.switch({ defaultLocale: "ja", defaultFtl: adminFtl("ja"), sources: [] }, "ja");
    await settle();
    expect(editor.summaryRow()).toBe(row);
    expect(row.textContent).toBe(
      "装備受付 / 装備フィルター / 最小レベル: 0 から 1000 までの整数を入力してください。",
    );

    removeNode(input);
    expect(editor.document.getElementById(input.id)).toBeNull();
    setConfigLocale("zh-CN");
    language.switch(
      { defaultLocale: "zh-CN", defaultFtl: adminFtl("zh-CN"), sources: [] },
      "zh-CN",
    );
    await settle();
    expect(editor.summaryRow()).toBe(row);
    expect(row.textContent).toBe("装备收录 / 装备过滤 / 最低等级: 请输入 0 至 1000 之间的整数。");
    expect(
      warn.mock.calls
        .flat()
        .map(String)
        .filter((message) => message.includes("[STRICT_READ_UNTRACKED]")),
    ).toEqual([]);
  } finally {
    scope.dispose();
  }
});
