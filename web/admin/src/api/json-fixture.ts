// T1 测试共用的 JSON 样本收窄：解析成功只证明文本合法，改写样本前仍须逐层确认形状。

/** 非 null、非数组的普通对象；返回值可安全用于结构化克隆与字段改写。 */
export function jsonObject(value: unknown, label: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error(`${label} 必须是 JSON 对象`);
  }
  // 上方已按运行期形状确认，这里的断言只把检查结果交给类型系统。
  return value as Record<string, unknown>;
}

/** 普通 JSON 数组；元素仍保持未知类型，由调用方继续检查。 */
export function jsonArray(value: unknown, label: string): unknown[] {
  if (!Array.isArray(value)) {
    throw new Error(`${label} 必须是 JSON 数组`);
  }
  return value;
}
