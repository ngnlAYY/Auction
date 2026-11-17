// 结构相等：对应迁移前派生值与依赖比较所用的按值相等。
// 只比较 JSON 形状的数据（基本类型、数组、普通对象）与 Map/Set/Date；函数与其余对象按引用比较。

/** 按值比较两份数据。 */
export function structuralEquals(left: unknown, right: unknown): boolean {
  if (Object.is(left, right)) {
    return true;
  }
  if (typeof left !== "object" || typeof right !== "object" || left === null || right === null) {
    return false;
  }
  if (Array.isArray(left)) {
    if (!Array.isArray(right) || left.length !== right.length) {
      return false;
    }
    return left.every((item, index) => structuralEquals(item, right[index]));
  }
  if (Array.isArray(right)) {
    return false;
  }
  if (left instanceof Date) {
    return right instanceof Date && left.getTime() === right.getTime();
  }
  if (left instanceof Map) {
    if (!(right instanceof Map) || left.size !== right.size) {
      return false;
    }
    for (const [key, value] of left) {
      if (!right.has(key) || !structuralEquals(value, right.get(key))) {
        return false;
      }
    }
    return true;
  }
  if (left instanceof Set) {
    if (!(right instanceof Set) || left.size !== right.size) {
      return false;
    }
    for (const value of left) {
      if (!right.has(value)) {
        return false;
      }
    }
    return true;
  }
  const leftPrototype: unknown = Object.getPrototypeOf(left);
  if (leftPrototype !== Object.getPrototypeOf(right)) {
    return false;
  }
  if (leftPrototype !== Object.prototype && leftPrototype !== null) {
    return false;
  }
  const leftRecord = left as Record<string, unknown>;
  const rightRecord = right as Record<string, unknown>;
  const leftKeys = Object.keys(leftRecord);
  if (leftKeys.length !== Object.keys(rightRecord).length) {
    return false;
  }
  return leftKeys.every(
    (key) => Object.hasOwn(rightRecord, key) && structuralEquals(leftRecord[key], rightRecord[key]),
  );
}
