import { expect, test } from "bun:test";
import { parseJson } from "./index";

const accepts = (text: string, policy: Parameters<typeof parseJson>[1]) =>
  parseJson(text, policy).ok;

test("uniqueKeys=all 拒绝任意一层的重复键", () => {
  const policy = { uniqueKeys: "all" } as const;
  expect(accepts('{"code":"ok","data":{"state":"blocked"}}', policy)).toBe(true);
  expect(accepts('{"code":"ok","code":"ok"}', policy)).toBe(false);
  expect(accepts('{"data":{"state":"blocked","state":"eligible"}}', policy)).toBe(false);
  expect(accepts('{"data":[{"id":1,"id":2}]}', policy)).toBe(false);
  // 不同对象里的同名键互不影响，包括数组里的并列对象与嵌套对象。
  expect(accepts('{"id":1,"data":[{"id":1},{"id":2}],"next":{"id":3}}', policy)).toBe(true);
  // 键名的转义形式不同、解码后相同，仍是重复键。
  expect(accepts('{"data":{"a":1,"\\u0061":2}}', policy)).toBe(false);
});

test("不设 uniqueKeys 时重复键沿用原有行为", () => {
  expect(accepts('{"data":{"state":"blocked","state":"eligible"}}', {})).toBe(true);
  expect(accepts('{"code":"ok","code":"ok"}', { uniqueRootKeys: ["code"] })).toBe(false);
});

test("numbers=integers 拒绝小数、指数与 -0", () => {
  const policy = { numbers: "integers" } as const;
  expect(accepts('{"data":{"state_version":2,"offset":-3,"items":[0,10]}}', policy)).toBe(true);
  for (const lexeme of ["2.0", "2e0", "2E0", "2.0000000000000001", "-0", "0.5"]) {
    expect(accepts(`{"data":{"state_version":${lexeme}}}`, policy)).toBe(false);
    expect(accepts(`{"data":{"items":[1,${lexeme}]}}`, policy)).toBe(false);
  }
  // 字符串里的数字写法不受影响。
  expect(accepts('{"message":"2.0 与 -0"}', policy)).toBe(true);
});

test("integerPaths 也拒绝 -0", () => {
  const policy = { integerPaths: [["data", "version"]] } as const;
  expect(accepts('{"data":{"version":0}}', policy)).toBe(true);
  expect(accepts('{"data":{"version":-0}}', policy)).toBe(false);
  expect(accepts('{"data":{"version":1.0}}', policy)).toBe(false);
  expect(accepts('{"data":{"other":1.5,"version":1}}', policy)).toBe(true);
});
