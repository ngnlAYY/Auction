#!/usr/bin/env python3
"""从本地真实 MoogleMail 页面生成脱敏 W6.3 语料；不联网、不打印真实身份或凭据。

运行：python3 ci/fixtures/sanitize_mooglemail.py [--check]
原始目录默认 target/audit-mm-live；原始页面与映射始终留在被忽略的目录中。
--check 只核对已生成文件与泄漏断言，不写入文件。
"""

import argparse
import hashlib
import html
from html.parser import HTMLParser
import json
from pathlib import Path
import re


class Element:
    def __init__(self, tag="", attrs=(), parent=None):
        self.tag, self.attrs, self.parent = tag, dict(attrs), parent
        self.children = []

    def text(self):
        return "".join(c if isinstance(c, str) else c.text() for c in self.children)

    def walk(self):
        yield self
        for child in self.children:
            if isinstance(child, Element):
                yield from child.walk()


class Document(HTMLParser):
    VOID = {"area", "base", "br", "col", "embed", "hr", "img", "input", "link",
            "meta", "param", "source", "track", "wbr"}

    def __init__(self, source):
        super().__init__(convert_charrefs=True)
        self.root = self.current = Element()
        self.feed(source)

    def handle_starttag(self, tag, attrs):
        child = Element(tag, attrs, self.current)
        self.current.children.append(child)
        if tag not in self.VOID:
            self.current = child

    def handle_startendtag(self, tag, attrs):
        self.handle_starttag(tag, attrs)
        self.handle_endtag(tag)

    def handle_endtag(self, tag):
        element = self.current
        while element.parent:
            if element.tag == tag:
                self.current = element.parent
                return
            element = element.parent

    def handle_data(self, data):
        self.current.children.append(data)


# 来源文件名也是输出名，防止把合成页面误标为真实抓取。
SAMPLES = [
    ("inbox-main.html", "inbox_empty", "main", "scan_inbox_page", 0),
    ("inbox-isekai.html", "inbox_empty", "isekai", "scan_inbox_page", 0),
    ("read-main.html", "read_list", "main", "scan_read_mail_page", 0),
    ("read-page1.html", "read_list", "main", "scan_read_mail_page", 1),
    ("read-page2.html", "read_list", "main", "scan_read_mail_page", 2),
    ("read-isekai.html", "read_list", "isekai", "scan_read_mail_page", 0),
    ("sent-main.html", "sent_list", "main", "scan_sent_page", 0),
    ("sent-isekai.html", "sent_list", "isekai", "scan_sent_page", 0),
    ("sentdetail-3125366.html", "sent_equipment_cod", "main", "parse_sent_detail", None),
    ("sentdetail-3125364.html", "sent_credits", "main", "parse_sent_detail", None),
    ("sentdetail-3125344.html", "sent_credits", "main", "parse_sent_detail", None),
    ("sentdetail-3125305.html", "sent_taken", "main", "parse_sent_detail", None),
    ("sentdetail-3125298.html", "sent_taken_name_projection", "main", "parse_sent_detail", None),
    ("readdetail-read-3125357.html", "read_return_receipt", "main", "parse_detail", None),
    ("readdetail-inbox-3125357.html", "inbox_return_receipt", "main", "parse_detail", None),
    ("taken-3125358.html", "read_taken", "main", "parse_action_state", None),
    ("taken-3125358-nostate.html", "read_taken_without_state", "main", "parse_terminal_empty_detail", None),
    ("compose-main.html", "compose_empty", "main", "parse_compose_state", None),
    ("compose-isekai.html", "compose_empty", "isekai", "parse_compose_state", None),
    ("sentdetail-missing.html", "sent_missing", "main", "parse_action_state_or_missing", None),
    ("inboxdetail-missing.html", "inbox_missing", "main", "parse_action_state_or_missing", None),
]
SECRET_NAME = re.compile(r"(?i)^(?:mm_?token|simple_token|postoken|post_?(?:token|key)|auth(?:_?(?:token|key))?|login_?key|ipb_pass_hash|cookie)$")
JS_SECRET = re.compile(r"\b(simple_token|mmtoken|postoken|post_token|post_key|auth|authkey|auth_key|auth_token|login_key)\s*=\s*(['\"])(.*?)\2", re.I)
FIXTURE_TOKEN = "fixture0001"


def identities(documents):
    names = set()
    for source in documents.values():
        for element in Document(source).root.walk():
            if element.attrs.get("id") == "mmail_list":
                for row in element.walk():
                    cells = [c for c in row.children if isinstance(c, Element) and c.tag == "td"]
                    if row.tag == "tr" and len(cells) == 4:
                        names.add(cells[0].text().strip())
            if element.attrs.get("id") == "mmail_left":
                values = [c.attrs.get("value", "").strip() for c in element.walk() if c.tag == "input"]
                names.update(value for value in values[:2] if value)
        names.update(re.findall(r"This message was returned from ([^<\n]+?), kupo!", html.unescape(source)))
    # 系统发件人是 RTS 协议标记，不是玩家名称（actions.rs 严格匹配）。
    names.discard("MoogleMail")
    if not names or "" in names:
        raise ValueError("未提取到完整的玩家身份集合")
    return names


def pseudonyms(names):
    # 最长身份为 owner；列表前缀沿用同一替身前缀，同时保持 Unicode 字符数和 UTF-8 字节数。
    roots = sorted(n for n in names if not any(other != n and other.startswith(n) for other in names))
    replacements = {}
    for root in roots:
        digest = hashlib.sha256(root.encode()).digest()
        chars = []
        for index, char in enumerate(root):
            if char.isspace():
                chars.append(char)
            else:
                pools = {1: "ABCDEFGHIJKLMNOPQRSTUVWXYZ" if index == 0 else "abcdefghijklmnopqrstuvwxyz",
                         2: "αβγδεζηθικλμνξοπρστυφχψω", 3: "◇◆○●□■△▲", 4: "𝔄𝔅𝔇𝔈𝔉𝔊𝔍𝔎"}
                pool = pools[len(char.encode())]
                chars.append(pool[digest[index % len(digest)] % len(pool)])
        replacements[root] = "".join(chars)
    for name in names:
        owners = [root for root in roots if root.startswith(name)]
        if len(owners) != 1:
            raise ValueError("玩家名称前缀有歧义，需要人工明确来源")
        replacements[name] = replacements[owners[0]][:len(name)]
    if len(set(replacements.values())) != len(replacements):
        raise ValueError("替身名称冲突")
    return replacements


def capture_uid(documents):
    values = set()
    for source in documents.values():
        values.update(re.findall(r"\bvar uid = (\d+);", source))
    if len(values) != 1:
        raise ValueError("来源抓取账号 UID 不唯一，需人工确认")
    return values.pop()


def equipment_key_values(documents):
    values = set()
    for source in documents.values():
        values.update(re.findall(r'"k":"([0-9a-fA-F]{6,})"', source))
    return values


def secret_values(documents):
    values = set()
    for source in documents.values():
        for element in Document(source).root.walk():
            if element.tag == "input" and SECRET_NAME.fullmatch(element.attrs.get("name", "")):
                value = element.attrs.get("value", "")
                if value:
                    values.add(value)
        values.update(match.group(3) for match in JS_SECRET.finditer(source) if match.group(3))
    return values


def variants(value):
    return {value, html.escape(value), html.escape(value, quote=False),
            json.dumps(value, ensure_ascii=True)[1:-1]}


def sanitize(source, names, secrets, uid, equipment_keys):
    replacements = {variant: replacement for name, replacement in names.items() for variant in variants(name)}
    replacements.update({variant: FIXTURE_TOKEN for value in secrets for variant in variants(value)})
    uid_replacement = "1" + "0" * (len(uid) - 1)
    replacements[uid] = uid_replacement
    for key in equipment_keys:
        replacements[key] = hashlib.sha256(key.encode()).hexdigest()[:len(key)]
    pattern = re.compile("|".join(re.escape(value) for value in sorted(replacements, key=len, reverse=True)))
    result = pattern.sub(lambda match: replacements[match.group()], source)
    # 当前抓取不含这些结构；拒绝未来带凭据的来源，不能靠漏删后继续生成。
    if re.search(r"(?i)(?:set-cookie\s*:|cookie\s*:|document\.cookie|https?://[^\s/<>]+@|[?&](?:amp;)?(?:authkey|auth_key|login_key|pass(?:word)?|token)=)", result):
        raise ValueError("检测到 Cookie 或 URL 凭据，需显式扩展脱敏规则")
    return result


def manifest(outputs):
    quote = lambda value: json.dumps(value, ensure_ascii=False)
    lines = ["# W6.3 真实页面子集；由 ci/fixtures/sanitize_mooglemail.py 生成。",
             'schema_version = 1', 'capture_date = "2026-11-02"',
             'source_directory = "target/audit-mm-live"',
             'sanitizer = "ci/fixtures/sanitize_mooglemail.py"',
             'sanitization = "玩家替身保持字符数、UTF-8 字节数及列表前缀；表单/JS token 统一为 fixture0001；账号 UID 与装备密钥替换为等长 fixture；拒绝 Cookie 和 URL 凭据；保留 MID、时间、标题截断、附件、COD、分页及页面导航。"',
             'scope = "本清单仅覆盖 MoogleMail；缺失类别不伪造真实来源。"', ""]
    for filename, kind, world, parser, page in SAMPLES:
        source = outputs[filename]
        mids = sorted(set(re.findall(r"mid=(\d+)", html.unescape(source))), key=int)
        own_mid = re.search(r"-(\d+)(?:-nostate)?\.html$", filename)
        pairs = []
        if kind.startswith("sent_") and own_mid:
            pairs = ["sent-main.html"]
        elif own_mid:
            pairs = ["read-main.html"]
            if "3125357" in filename:
                pairs.append("readdetail-inbox-3125357.html" if "-read-" in filename else "readdetail-read-3125357.html")
            elif "3125358" in filename:
                pairs.append("taken-3125358.html" if "nostate" in filename else "taken-3125358-nostate.html")
        elif kind in {"read_list", "sent_list"} and world == "main":
            pairs = [name for name, _, _, _, _ in SAMPLES if any(re.search(r"-" + mid + r"(?:-nostate)?\.html$", name) for mid in mids)]
        lines += ["[[fixtures]]", f"file = {quote(filename)}", f"source_file = {quote(filename)}",
                  f"class = {quote(kind)}", f"world = {quote(world)}",
                  f"parser_entry_point = {quote(parser)}", 'sanitization = "见顶层 sanitization；未复制原始页面。"',
                  f"pairs = {json.dumps(pairs)}", f"bytes = {len(source.encode())}",
                  f"sha256 = {quote(hashlib.sha256(source.encode()).hexdigest())}"]
        if page is not None:
            lines.append(f"page = {page}")
            lines.append(f"mids = {json.dumps(mids)}")
        if own_mid:
            lines.append(f"mid = {quote(own_mid.group(1))}")
        if filename == "sentdetail-3125298.html":
            lines.append('pairing_note = "MID3125298：Sent 列表玩家名 13 字符/15 字节，详情完整名 14 字符/16 字节；替身保持前缀关系。"')
        if "nostate" in filename:
            lines.append('source_note = "来源目录已有的去状态脚本诊断变体；非独立远端响应，不能当作真实无状态响应证据。"')
        lines.append("")
    for kind in ["forum", "battle", "login"]:
        lines += ["[[missing]]", f"class = {quote(kind)}",
                  'reason = "本地授权来源没有该类别的真实样本；未创建合成替代页。"', ""]
    return "\n".join(lines)


def main():
    arguments = argparse.ArgumentParser(description=__doc__)
    arguments.add_argument("--raw-dir", type=Path, default=Path("target/audit-mm-live"))
    arguments.add_argument("--output-dir", type=Path, default=Path("crates/auction-integrations/tests/fixtures/w6_3"))
    arguments.add_argument("--check", action="store_true")
    args = arguments.parse_args()
    # 全部本地 HTML 提供身份别名；只输出 SAMPLES 明确列出的页面。
    documents = {path.name: path.read_text() for path in sorted(args.raw_dir.glob("*.html"))}
    names = pseudonyms(identities(documents))
    secrets = secret_values(documents)
    if not secrets:
        raise ValueError("来源未提取到表单 token")
    uid = capture_uid(documents)
    equipment_keys = equipment_key_values(documents)
    outputs = {name: sanitize(documents[name], names, secrets, uid, equipment_keys) for name, _, _, _, _ in SAMPLES}
    outputs["manifest.toml"] = manifest(outputs)
    forbidden = {variant for value in set(names) | secrets | {uid} | equipment_keys for variant in variants(value)}
    for filename, source in outputs.items():
        if any(value in source for value in forbidden):
            raise ValueError(f"脱敏失败：{filename} 仍含真实名称或 token（不打印值）")
    # 字符数、字节数与投影关系都是转换的不变量。
    for name, replacement in names.items():
        assert len(name) == len(replacement) and len(name.encode()) == len(replacement.encode())
        for other, other_replacement in names.items():
            if other.startswith(name):
                assert other_replacement.startswith(replacement)
    if not args.check:
        args.output_dir.mkdir(parents=True, exist_ok=True)
        for filename, source in outputs.items():
            (args.output_dir / filename).write_text(source)
    for filename, source in outputs.items():
        if (args.output_dir / filename).read_text() != source:
            raise ValueError(f"文件与确定性脱敏结果不符：{filename}")
        print(f"{filename}\t{len(source.encode())} bytes")
    print(f"PASS: {len(names)} real identity variants and {len(secrets)} real token values absent; character/byte lengths and all name prefixes preserved.")


if __name__ == "__main__":
    main()
