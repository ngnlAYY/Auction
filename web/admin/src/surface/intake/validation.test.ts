// 对照 Rust 手工录入对 canonical 装备 JSON 的校验。

import { expect, spyOn, test } from "bun:test";
import {
  equipmentJson,
  equipmentJsonWithBase,
  OVERWRITTEN_INVALID_STATS,
} from "../../api/intake-fixture";
import { loadRenderModule } from "../../solid-render-fixture.test";
import {
  type EditorTest,
  editField,
  present,
  settleEditor,
  submitForm,
  withEditor,
} from "../auctions/editor-fixture.test";
import { createComponent } from "../solid-dom-fixture";
import { validCanonicalEquipmentJson } from "./validation";

function equipment() {
  return {
    external_equipment_id: "123",
    name: "Legendary Phase Robe",
    category: "Cloth",
    equipment_type: "Phase Robe",
    quality: "Legendary",
    prefix: "",
    suffix: "",
    level: null,
    tradeable: true,
    stats: { Defense: { value: "1" } },
    key: "0123456789",
  };
}

test("accepts_empty_stats_and_affixes", () => {
  expect(validCanonicalEquipmentJson(JSON.stringify({ ...equipment(), stats: {} }))).toBe(true);
});

test("rejects_empty_quality", () => {
  const valid = equipment();
  expect(validCanonicalEquipmentJson(JSON.stringify(valid))).toBe(true);
  expect(validCanonicalEquipmentJson(JSON.stringify({ ...valid, quality: "" }))).toBe(false);
});

for (const [reason, stats] of OVERWRITTEN_INVALID_STATS) {
  test(`录入表单校验同名属性的最后一项（${reason}）`, () => {
    expect(validCanonicalEquipmentJson(equipmentJson())).toBe(true);
    expect(validCanonicalEquipmentJson(equipmentJson(stats))).toBe(true);
  });
}

test.each(["1000000.01", "-0.1"])("录入表单拒绝超出业务范围的 base %s", (base) => {
  expect(validCanonicalEquipmentJson(equipmentJsonWithBase("0.1"))).toBe(true);
  expect(validCanonicalEquipmentJson(equipmentJsonWithBase(base))).toBe(false);
});

async function withJsonIntake(
  text: string,
  run: (page: EditorTest) => Promise<void>,
): Promise<void> {
  const { AdminIntakeForm } = await loadRenderModule<typeof import(".")>(
    `${import.meta.dir}/index.tsx`,
    "AdminIntakeForm",
  );
  await withEditor(
    () => createComponent(AdminIntakeForm, { worldId: "main" }),
    async (page) => {
      const source = page.field("auction-intake-source");
      expect(typeof source.attributes.onChange).toBe("function");
      source.change("json");
      await settleEditor();
      await editField(page.field("auction-intake-json"), text);
      await editField(page.field("auction-intake-price"), "1000");
      await editField(page.field("auction-intake-player"), "Player");
      await run(page);
    },
  );
}

// Rust 表单按 Value 末值放行，随后 API 在具名 DTO 解码时拒绝并显示整表错误。
test("重复装备字段通过表单校验，再显示录入 API 拒绝", () =>
  withJsonIntake(equipmentJson(undefined, '"prefix":"旧","prefix":""'), async (page) => {
    const field = page.field("auction-intake-json");
    const intakeApi = await import("../../api/intake");
    const add = spyOn(intakeApi, "addAdminIntake");
    try {
      await submitForm(page.form());
      expect(page.takeWrites()).toEqual([]);
      expect(field.getAttribute("aria-invalid")).toBeNull();
      expect(field.getAttribute("aria-describedby")).toBe("auction-intake-json-hint");
      expect(add).toHaveBeenCalledTimes(1);
      expect(await add.mock.results[0]?.value).toEqual({
        ok: false,
        error: { kind: "not-sent", code: "invalid_request" },
      });
      expect(page.hasText("请输入有效的规范装备 JSON 对象。")).toBe(false);
      const alerts = page.find(
        (node) => node.getAttribute("role") === "alert" && node.matches(".action-feedback--error"),
      );
      expect(alerts).toHaveLength(1);
      const alert = present(alerts[0], "录入失败提示");
      expect(alert.querySelector("strong")?.textContent).toBe("装备录入失败");
      expect(alert.querySelector("span")?.textContent).toBe("请求参数无效。请检查输入内容后重试。");
    } finally {
      add.mockRestore();
    }
  }));

// Rust 由 DTO 解码拒绝属性内的重复字段，Solid 表单也会校验它。
test("录入拒绝被覆盖属性内的重复具名字段且不发出请求", () =>
  withJsonIntake(
    equipmentJson('"Defense":{"base":0,"base":1},"Defense":{"value":"2"}'),
    async (page) => {
      const fetch = spyOn(globalThis, "fetch");
      try {
        await submitForm(page.form());
        const alerts = page.find((node) => node.getAttribute("role") === "alert");
        expect(
          alerts.some(
            (alert) =>
              alert.textContent === "请输入有效的规范装备 JSON 对象。" ||
              alert.textContent.includes("请求参数无效。请检查输入内容后重试。"),
          ),
        ).toBe(true);
        expect(fetch).not.toHaveBeenCalled();
      } finally {
        fetch.mockRestore();
      }
    },
  ));
