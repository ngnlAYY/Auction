// 场次摘要目录读取页长的回归断言。

import { expect, test } from "bun:test";

import { MAX_PUBLIC_PAGE_SIZE } from "@auction/contracts/dto/public/pagination";

const { SUMMARY_PAGE_SIZE } = await import("./directory");

test("summary_directory_reads_use_the_public_page_size_maximum：摘要目录使用公开页长上限", () => {
  expect(SUMMARY_PAGE_SIZE).toBe(MAX_PUBLIC_PAGE_SIZE);
  expect(SUMMARY_PAGE_SIZE).toBe(200);
});
