// 文本原语：排序与后端的 UTF-8 字节序一致，长度按字节计。

import { expect, test } from "bun:test";
import { compareText, utf8ByteLength } from ".";

const ENCODER = new TextEncoder();

function compareBytes(left: string, right: string): -1 | 0 | 1 {
  const a = ENCODER.encode(left);
  const b = ENCODER.encode(right);
  for (let index = 0; index < Math.min(a.length, b.length); index += 1) {
    const x = a[index] ?? 0;
    const y = b[index] ?? 0;
    if (x !== y) {
      return x < y ? -1 : 1;
    }
  }
  if (a.length === b.length) {
    return 0;
  }
  return a.length < b.length ? -1 : 1;
}

test("compareText 与 UTF-8 字节序在各类字符上一致", () => {
  const samples = [
    "",
    "a",
    "ab",
    "é",
    "中",
    "\u{E000}",
    "\u{FFFF}",
    "\u{10000}",
    "\u{1F600}",
    "a\u{10000}",
    "a\u{E000}",
    "\u{10FFFF}",
    "z",
  ];
  for (const left of samples) {
    for (const right of samples) {
      expect(compareText(left, right)).toBe(compareBytes(left, right));
    }
  }
});

test("U+10000 以上的字符排在 U+E000 之后，与 JS 的默认比较相反", () => {
  expect("\u{10000}" < "\u{E000}").toBe(true);
  expect(compareText("\u{10000}", "\u{E000}")).toBe(1);
});

test("utf8ByteLength 按 UTF-8 字节计数", () => {
  expect(utf8ByteLength("")).toBe(0);
  expect(utf8ByteLength("a中\u{1F600}")).toBe(8);
});
