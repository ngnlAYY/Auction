// 后端 `uuid::Uuid` 的线上表示：接受标准连字符、无连字符、花括号与 URN 四种写法。

import * as v from "valibot";

const HEX = "[0-9a-fA-F]";
const HYPHENATED = `${HEX}{8}-${HEX}{4}-${HEX}{4}-${HEX}{4}-${HEX}{12}`;
const UUID_PATTERN = new RegExp(
  `^(?:${HEX}{32}|${HYPHENATED}|\\{${HYPHENATED}\\}|[uU][rR][nN]:[uU][uU][iI][dD]:${HYPHENATED})(?![\\s\\S])`,
);

/** 后端 `uuid::Uuid`：校验后保留收到时的原始字符串。 */
export const uuid = v.pipe(
  v.string(),
  v.check((value) => UUID_PATTERN.test(value), "必须是 UUID"),
);
