// 浏览器 UUID 生成的安全上下文、明文回退与失败边界。

import { expect, mock, test } from "bun:test";

import { validUuid } from "../ui/validation";
import { createUuidV4 } from "./uuid";

function withCrypto<T>(crypto: Partial<Crypto> | undefined, run: () => T): T {
  const original = Object.getOwnPropertyDescriptor(globalThis, "crypto");
  Object.defineProperty(globalThis, "crypto", { configurable: true, value: crypto });
  try {
    return run();
  } finally {
    if (original === undefined) {
      Reflect.deleteProperty(globalThis, "crypto");
    } else {
      Object.defineProperty(globalThis, "crypto", original);
    }
  }
}

test("优先使用浏览器的 randomUUID", () => {
  const native = globalThis.crypto;
  const randomUUID = mock(() => native.randomUUID());
  const uuid = withCrypto({ randomUUID }, createUuidV4);
  expect(randomUUID).toHaveBeenCalledTimes(1);
  expect(validUuid(uuid)).toBe(true);
  expect(uuid.charAt(14)).toBe("4");
});

test.each([
  [0x00, "00000000-0000-4000-8000-000000000000"],
  [0xff, "ffffffff-ffff-4fff-bfff-ffffffffffff"],
])("getRandomValues 回退设置版本与变体位，保留其他随机位", (byte, expected) => {
  const getRandomValues = <T extends ArrayBufferView | null>(bytes: T): T => {
    if (!(bytes instanceof Uint8Array)) {
      throw new Error("预期 UUID 使用字节数组");
    }
    expect(bytes.length).toBe(16);
    bytes.fill(byte);
    return bytes;
  };
  const uuid = withCrypto({ getRandomValues }, createUuidV4);
  expect(validUuid(uuid)).toBe(true);
  expect(uuid).toBe(expected);
});

test("两种安全随机接口都缺失时立即失败", () => {
  expect(() => withCrypto({}, createUuidV4)).toThrow("安全随机数接口");
  expect(() => withCrypto(undefined, createUuidV4)).toThrow("安全随机数接口");
});
