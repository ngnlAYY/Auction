// 首次部署的 Host 与 Origin 语法预检；国际化域名的细枝末节交给平台 URL，后端解析为权威。
// 平台 URL 的 IDNA 数据在个别边角可能与 Rust 不同，以后端校验结果为准。

/** 保持首次部署 owner 的 Host 规则：先拒绝带端口、路径与空白的写法，再由平台 URL 解析。 */
export function validHost(value: string): boolean {
  if (
    value.length === 0 ||
    containsWhitespace(value) ||
    value.includes("%") ||
    value.includes("?") ||
    value.includes("#") ||
    value.includes("/") ||
    value.includes("\\") ||
    value.includes("@") ||
    hasPunycodeLeadingHyphen(value)
  ) {
    return false;
  }
  if (value.startsWith("[")) {
    // Rust 只在输入恰好是 `[...]` 时解析 IPv6；`[::1]:80` 与 `[::1]:` 不是合法 Host，端口由独立字段提供。
    if (!value.endsWith("]") || value.length < 3) {
      return false;
    }
    return ipv6SyntaxValid(value.slice(1, -1)) && urlHasOnlyHost(`http://${value}`);
  }
  if (value.includes(":")) {
    // 裸 IPv6 由 Rust 的 `IpAddr` 分支接受；URL 只有加上方括号才能解析。
    return ipv6SyntaxValid(value) && urlHasOnlyHost(`http://[${value}]`);
  }
  return urlHasOnlyHost(`http://${value}`);
}

/** 保持首次部署 owner 的精确 Origin 规则；不自动去掉路径或改写输入的站点。 */
export function validOrigin(value: string): boolean {
  const authority = originAuthority(value);
  if (
    authority === null ||
    authority.length === 0 ||
    authority.includes("/") ||
    containsWhitespace(value) ||
    value.includes("?") ||
    value.includes("#") ||
    value.includes("\\") ||
    value.includes("@") ||
    hasPunycodeLeadingHyphen(authority)
  ) {
    return false;
  }
  try {
    const parsed = new URL(value);
    return (
      (parsed.protocol === "http:" || parsed.protocol === "https:") &&
      (parsed.protocol === "https:" ||
        parsed.hostname === "localhost" ||
        parsed.hostname === "127.0.0.1" ||
        parsed.hostname === "[::1]") &&
      parsed.hostname !== "" &&
      parsed.username === "" &&
      parsed.password === "" &&
      parsed.pathname === "/" &&
      parsed.search === "" &&
      parsed.hash === "" &&
      parsed.origin === value &&
      !hasPunycodeLeadingHyphen(parsed.hostname)
    );
  } catch {
    return false;
  }
}

function originAuthority(value: string): string | null {
  if (value.startsWith("https://")) {
    return value.slice("https://".length);
  }
  if (value.startsWith("http://")) {
    return value.slice("http://".length);
  }
  return null;
}

/** 浏览器 URL 解析只能近似 Rust `url` crate 的 host 规则；这里要求整个输入恰好是主机。 */
function urlHasOnlyHost(address: string): boolean {
  try {
    const parsed = new URL(address);
    return (
      parsed.username === "" &&
      parsed.password === "" &&
      parsed.port === "" &&
      parsed.pathname === "/" &&
      parsed.search === "" &&
      parsed.hash === "" &&
      parsed.hostname !== "" &&
      !hasPunycodeLeadingHyphen(parsed.hostname)
    );
  } catch {
    return false;
  }
}

/** Rust `IpAddr` 不接受的畸形 IPv6：平台 URL 会把末尾单个冒号悄悄归一化，必须先按语法拒绝。 */
function ipv6SyntaxValid(address: string): boolean {
  // `::` 是合法的零压缩；除此之外，以单个冒号结尾的地址都是残缺的。
  return !(address.endsWith(":") && !address.endsWith("::"));
}

/** 预检原文并按第四轮裁定复查平台归一化后的主机名：`xn--` 后的载荷以 `-` 开头一律拒绝。 */
function hasPunycodeLeadingHyphen(value: string): boolean {
  for (const label of value.split(/[.\u3002\uFF0E\uFF61]/u)) {
    const lower = label.toLowerCase();
    if (lower.startsWith("xn--") && lower.charCodeAt(4) === 0x2d) {
      return true;
    }
  }
  return false;
}

/** `Host::parse` 会拒绝任何 C0 控制字符与空格，包括 URL 解析器会静默去掉的制表符。 */
export function containsAsciiControlOrSpace(value: string): boolean {
  for (let index = 0; index < value.length; index += 1) {
    if (value.charCodeAt(index) <= 0x20) {
      return true;
    }
  }
  return false;
}

function containsWhitespace(value: string): boolean {
  if (containsAsciiControlOrSpace(value)) {
    return true;
  }
  for (let index = 0; index < value.length; index += 1) {
    if (isRustWhitespace(value.charCodeAt(index))) {
      return true;
    }
  }
  return false;
}

/** Rust `str::trim` 的空白集合；JS 的 `trim` 与之不同，不能替换。 */
export function rustTrim(value: string): string {
  let start = 0;
  let end = value.length;
  while (start < end && isRustWhitespace(value.charCodeAt(start))) {
    start += 1;
  }
  while (end > start && isRustWhitespace(value.charCodeAt(end - 1))) {
    end -= 1;
  }
  return value.slice(start, end);
}

function isRustWhitespace(code: number): boolean {
  return (
    code === 0x09 ||
    code === 0x0a ||
    code === 0x0b ||
    code === 0x0c ||
    code === 0x0d ||
    code === 0x20 ||
    code === 0x85 ||
    code === 0xa0 ||
    code === 0x1680 ||
    (code >= 0x2000 && code <= 0x200a) ||
    code === 0x2028 ||
    code === 0x2029 ||
    code === 0x202f ||
    code === 0x205f ||
    code === 0x3000
  );
}

/** 判断字符串是否非空且全部为 ASCII 数字；空串返回 `false`。 */
export function isAsciiDigits(value: string): boolean {
  if (value.length === 0) {
    return false;
  }
  for (let index = 0; index < value.length; index += 1) {
    if (!isAsciiDigitCode(value.charCodeAt(index))) {
      return false;
    }
  }
  return true;
}

/** 判断一个 UTF-16 码元是否为 ASCII 数字 `0`–`9`。 */
export function isAsciiDigitCode(code: number): boolean {
  return code >= 0x30 && code <= 0x39;
}
