// Public 看板查询默认值：只序列化稳定的默认页长。

import { expect, test } from "bun:test";

const { publicBoardQueryDefault } = await import("./board");
const { publicQueryPairs } = await import("./serialization");

function pairs(values: readonly (readonly [string, string])[]): [string, string][] {
  return values.map(([key, value]) => [key, value]);
}

test("default_query_state_serializes_only_stable_page_sizes：默认查询只写出稳定页长", () => {
  const query = publicBoardQueryDefault();
  expect(query.includeUnleveled).toBe("true");
  expect(query.historySort).toBe("closed_desc");
  expect(query.historyGroup).toBe("flat");
  expect(publicQueryPairs(query)).toEqual(
    pairs([
      ["page_size", "20"],
      ["movement_page_size", "20"],
    ]),
  );
});
