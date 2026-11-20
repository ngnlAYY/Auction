// 查询写回把同地址策略透传给共享历史层，并返回写入是否成功。

import { expect, test } from "bun:test";

import { installFakeWindow } from "../../router/test-window";
import { writeQueryPairs } from ".";

test("没有浏览器环境时不写入并返回 false", () => {
  expect(writeQueryPairs(["page"], [["page", "2"]], "push")).toBe(false);
});

test("显式同地址写入照常创建历史项，缺省仍跳过", () => {
  const { fake, restore } = installFakeWindow("/admin?page=1");
  try {
    const before = fake.writes.length;
    expect(writeQueryPairs(["page"], [["page", "1"]], "push")).toBe(false);
    expect(fake.writes.length).toBe(before);

    expect(writeQueryPairs(["page"], [["page", "1"]], "push", { sameAddress: "write" })).toBe(true);
    expect(fake.writes.length).toBe(before + 1);
    expect(fake.location.search).toBe("?page=1");

    expect(writeQueryPairs(["page"], [["page", "1"]], "replace", { sameAddress: "write" })).toBe(
      true,
    );
    expect(fake.writes.length).toBe(before + 2);
  } finally {
    restore();
  }
});

test("浏览器拒绝写入时返回 false", () => {
  const { fake, restore } = installFakeWindow("/admin?page=1");
  try {
    fake.rejectWrites("push");
    expect(writeQueryPairs(["page"], [["page", "1"]], "push", { sameAddress: "write" })).toBe(
      false,
    );
  } finally {
    restore();
  }
});
