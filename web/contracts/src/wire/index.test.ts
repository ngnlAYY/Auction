// 字段原语的接受集合：与后端 serde 规则逐条对应，失败关闭。

import { expect, test } from "bun:test";
import * as v from "valibot";
import {
  decode,
  int32,
  integer,
  nullAsDefault,
  optionalField,
  optionalNullable,
  requiredNullable,
  strictStruct,
  stringMap,
  struct,
  uint8,
  unsignedInteger,
  withDefault,
} from ".";

const Sample = struct({
  id: v.string(),
  amount: integer,
  count: unsignedInteger,
  note: optionalNullable(v.string()),
  ended_at: requiredNullable(v.string()),
  lot_count: optionalField(integer),
  enabled: withDefault(v.boolean(), false),
  mode: nullAsDefault(v.string(), ""),
});
const base = { id: "a", amount: 1, count: 0, ended_at: null, mode: null };

function accepts(schema: v.GenericSchema, data: unknown): boolean {
  return decode(schema, data).ok;
}

test("缺省、可空与默认值各走各的原语", () => {
  const decoded = decode(Sample, base);
  expect(decoded).toEqual({
    ok: true,
    value: {
      id: "a",
      amount: 1,
      count: 0,
      note: null,
      ended_at: null,
      lot_count: null,
      enabled: false,
      mode: "",
    },
  });
  const present = decode(Sample, {
    ...base,
    note: "n",
    lot_count: 3,
    enabled: true,
    mode: "slow",
  });
  expect(present.ok && present.value.lot_count).toBe(3);
});

test("必填可空的键必须出现；可缺省字段不接受 null", () => {
  expect(accepts(Sample, { id: "a", amount: 1, count: 0, mode: null })).toBe(false);
  expect(accepts(Sample, { id: "a", amount: 1, count: 0, ended_at: null })).toBe(false);
  expect(accepts(Sample, { ...base, lot_count: null })).toBe(false);
  expect(accepts(Sample, { ...base, enabled: null })).toBe(false);
});

test("整数只接受安全整数并按宽度限定范围", () => {
  expect(accepts(Sample, { ...base, amount: 2 ** 53 })).toBe(false);
  expect(accepts(Sample, { ...base, amount: 1.5 })).toBe(false);
  expect(accepts(Sample, { ...base, count: -1 })).toBe(false);
  expect(accepts(int32, 2_147_483_647)).toBe(true);
  expect(accepts(int32, 2_147_483_648)).toBe(false);
  expect(accepts(int32, -2_147_483_649)).toBe(false);
  expect(accepts(uint8, 255)).toBe(true);
  expect(accepts(uint8, 256)).toBe(false);
});

test("结构体只接受 JSON 对象，数组与 null 一律拒绝", () => {
  const AllOptional = struct({ authorization: optionalNullable(v.string()) });
  expect(decode(AllOptional, {})).toEqual({ ok: true, value: { authorization: null } });
  expect(accepts(AllOptional, [])).toBe(false);
  expect(accepts(AllOptional, null)).toBe(false);
  const Outer = struct({ inner: optionalNullable(AllOptional) });
  expect(accepts(Outer, { inner: [] })).toBe(false);
});

test("未知键：普通结构体忽略，严格结构体拒绝", () => {
  const loose = decode(struct({ a: integer }), { a: 1, b: 2 });
  expect(loose).toEqual({ ok: true, value: { a: 1 } });
  expect(accepts(strictStruct({ a: integer }), { a: 1, b: 2 })).toBe(false);
  expect(accepts(strictStruct({ a: integer }), [1])).toBe(false);
});

test("映射只接受对象、逐条校验且不丢键", () => {
  const Worlds = struct({ worlds: stringMap(struct({ enabled: v.boolean() })) });
  expect(accepts(Worlds, { worlds: [] })).toBe(false);
  expect(accepts(Worlds, { worlds: [{ enabled: true }] })).toBe(false);
  expect(accepts(Worlds, JSON.parse('{"worlds":{"constructor":null}}'))).toBe(false);
  const decoded = decode(
    Worlds,
    JSON.parse('{"worlds":{"constructor":{"enabled":true},"__proto__":{"enabled":false}}}'),
  );
  expect(decoded.ok && Object.keys(decoded.value.worlds).sort()).toEqual([
    "__proto__",
    "constructor",
  ]);
});

test("可空 schema 的 null 是成功值，不是解码失败", () => {
  const Nullable = requiredNullable(struct({ id: v.string() }));
  expect(decode(Nullable, null)).toEqual({ ok: true, value: null });
  expect(accepts(Nullable, 1)).toBe(false);
});
