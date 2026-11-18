// 浏览器命令与标签页修订使用的随机 UUID。

/** 生成 RFC 4122 v4 UUID。明文部署可用 Web Crypto 的 getRandomValues 保持随机强度。 */
export function createUuidV4(): string {
  const crypto = globalThis.crypto;
  if (typeof crypto?.randomUUID === "function") {
    return crypto.randomUUID();
  }
  if (typeof crypto?.getRandomValues !== "function") {
    // 命令的幂等身份依赖低碰撞率，缺少安全随机源时不能继续提交。
    throw new Error("浏览器缺少生成 UUID 所需的安全随机数接口");
  }

  const bytes = crypto.getRandomValues(new Uint8Array(16));
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  view.setUint8(6, (view.getUint8(6) & 0x0f) | 0x40);
  view.setUint8(8, (view.getUint8(8) & 0x3f) | 0x80);
  const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}
