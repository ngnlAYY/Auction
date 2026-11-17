// 可失败操作的返回值：预期内的失败作为值返回，不用异常表达。

/** 成功结果。 */
export interface Ok<T> {
  readonly ok: true;
  readonly value: T;
}

/** 失败结果。 */
export interface Err<E> {
  readonly ok: false;
  readonly error: E;
}

/** 成功或失败二选一；用 `result.ok` 收窄后读取 `value` 或 `error`。 */
export type Result<T, E> = Ok<T> | Err<E>;

/** 构造成功结果。 */
export function ok<T>(value: T): Ok<T> {
  return { ok: true, value };
}

/** 构造失败结果。 */
export function err<E>(error: E): Err<E> {
  return { ok: false, error };
}

/** 穷尽性检查：switch 的 default 分支调用，漏掉联合成员时编译失败。 */
export function unreachable(value: never): never {
  throw new Error(`出现了未处理的取值：${String(value)}`);
}
