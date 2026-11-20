// 恢复原因按裁剪后的 UTF-8 字节计数，保留内部换行。

import { expect, test } from "bun:test";
import { utf8ByteLength } from "@auction/web-shared/text";

import { validateRecoveryReason } from "./recovery-reason";

test("recovery_reasons_count_utf8_bytes_and_allow_internal_newlines", () => {
  const limit = `${"拍".repeat(170)}ab`;
  const tooLong = "拍".repeat(171);
  expect(utf8ByteLength(limit)).toBe(512);
  expect(utf8ByteLength(tooLong)).toBe(513);
  expect(validateRecoveryReason("cancelled", limit)).toEqual({ ok: true, value: limit });
  expect(validateRecoveryReason("equipment_recovered", tooLong)).toEqual({
    ok: false,
    error: "too_long",
  });
  expect(validateRecoveryReason("cancelled", " 原因\n第二行 ")).toEqual({
    ok: true,
    value: "原因\n第二行",
  });
  expect(validateRecoveryReason("cancelled", " ")).toEqual({ ok: false, error: "required" });
  expect(validateRecoveryReason("reclaimed", "")).toEqual({ ok: true, value: "" });
});
