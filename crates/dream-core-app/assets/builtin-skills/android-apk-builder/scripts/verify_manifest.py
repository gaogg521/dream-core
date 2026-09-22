#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
verify_manifest.py — 校验手写的二进制 AndroidManifest.xml 是否合法

为什么需要它：手写 binary XML 出错时，Android 只会给一句 "Failed to parse
AndroidManifest.xml" 或直接安装失败，没有任何定位信息。这个脚本在打包前就把
chunk 链、字符串池、标签嵌套全部走查一遍，并把内容还原成可读 XML。

用法：
    python verify_manifest.py <AndroidManifest.xml>

退出码 0 = 结构合法；非 0 = 有问题，会打印具体位置。
"""

import struct
import sys

CHUNK_STRING_POOL = 0x001C0001
CHUNK_RES_MAP = 0x00080180
CHUNK_XML_FILE = 0x00080003
CHUNK_START_NS = 0x00100100
CHUNK_END_NS = 0x00100101
CHUNK_START_TAG = 0x00100102
CHUNK_END_TAG = 0x00100103

CHUNK_NAMES = {
    CHUNK_STRING_POOL: "STRING_POOL",
    CHUNK_RES_MAP: "RESOURCE_MAP",
    CHUNK_START_NS: "START_NAMESPACE",
    CHUNK_END_NS: "END_NAMESPACE",
    CHUNK_START_TAG: "START_ELEMENT",
    CHUNK_END_TAG: "END_ELEMENT",
}

TYPE_NAMES = {
    0x03: "string",
    0x10: "int-dec",
    0x12: "boolean",
    0x1C: "int-hex",
}

ANDROID_NS_URI = "http://schemas.android.com/apk/res/android"


class ManifestError(Exception):
    pass


class Reader:
    def __init__(self, data):
        self.d = data

    def u8(self, o):
        return self.d[o]

    def u16(self, o):
        return struct.unpack_from("<H", self.d, o)[0]

    def u32(self, o):
        return struct.unpack_from("<I", self.d, o)[0]


def parse_string_pool(r, start, size):
    """返回 (字符串列表, flags)"""
    count = r.u32(start + 8)
    style_count = r.u32(start + 12)
    flags = r.u32(start + 16)
    strings_start = r.u32(start + 20)

    if flags & 0x00000100:
        raise ManifestError(
            "字符串池是 UTF-8 编码。Java 侧解析器在 UTF-8 模式下要求每项带"
            " LEB128 双长度前缀，手写极易漏掉，实测报 'string not NULL "
            "terminated'。请改用 UTF-16（flags=0）。"
        )

    strings = []
    base = start + strings_start
    for i in range(count):
        off = r.u32(start + 28 + 4 * i)
        p = base + off
        n = r.u16(p)
        raw = r.d[p + 2: p + 2 + n * 2]
        term = r.u16(p + 2 + n * 2)
        if term != 0:
            raise ManifestError(
                f"字符串 #{i} 缺少 NULL 结尾（读到 0x{term:04X}）"
            )
        strings.append(raw.decode("utf-16-le"))
    # style 数量只做记录，不做解析
    return strings, flags, style_count


def parse(data):
    """解析整个文件，返回 (字符串列表, 事件列表, 报告行列表)"""
    r = Reader(data)
    report = []

    magic = r.u32(0)
    total = r.u32(4)
    if magic != CHUNK_XML_FILE:
        raise ManifestError(f"文件头不是 XML 魔数（读到 0x{magic:08X}）")
    if total != len(data):
        raise ManifestError(f"声明总长 {total} 与实际 {len(data)} 不符")
    report.append(f"XML 文件头 OK，总长 {total} bytes")

    strings = []
    events = []
    off = 8
    while off < total:
        if off + 8 > total:
            raise ManifestError(f"偏移 {off} 处不足 8 字节，chunk 链断裂")
        head = r.u32(off)
        size = r.u32(off + 4)
        if size < 8 or off + size > total:
            raise ManifestError(
                f"偏移 {off} 处 chunk 长度非法（size={size}，剩余 {total - off}）"
            )
        name = CHUNK_NAMES.get(head, f"UNKNOWN(0x{head:08X})")

        if head == CHUNK_STRING_POOL:
            strings, flags, _sc = parse_string_pool(r, off, size)
            report.append(
                f"字符串池 OK，{len(strings)} 条，flags=0x{flags:08X}（UTF-16）"
            )
        elif head == CHUNK_RES_MAP:
            report.append(f"Resource map OK，{size} bytes")
        elif head == CHUNK_START_NS:
            if size != 24:
                raise ManifestError(
                    f"START_NAMESPACE 长度必须是 24，实际 {size}。"
                    "写 16 会导致后续所有 chunk 偏移错位。"
                )
            events.append(("start-ns", (r.u32(off + 16), r.u32(off + 20))))
        elif head == CHUNK_END_NS:
            if size != 24:
                raise ManifestError(f"END_NAMESPACE 长度必须是 24，实际 {size}")
            events.append(("end-ns", (r.u32(off + 16), r.u32(off + 20))))
        elif head == CHUNK_START_TAG:
            line = r.u32(off + 8)
            ns = r.u32(off + 16)
            tag = r.u32(off + 20)
            attr_start = r.u16(off + 24)
            attr_size = r.u16(off + 26)
            attr_count = r.u16(off + 28)
            expect = 36 + attr_count * 20
            if size != expect:
                raise ManifestError(
                    f"START_ELEMENT 长度应为 {expect}（36+{attr_count}*20），实际 {size}"
                )
            if attr_size != 20:
                raise ManifestError(f"attributeSize 应为 20，实际 {attr_size}")
            if attr_start != 20:
                raise ManifestError(f"attributeStart 应为 20，实际 {attr_start}")
            attrs = []
            for i in range(attr_count):
                p = off + 36 + i * 20
                if p + 20 > off + size:
                    raise ManifestError(f"属性 #{i} 越出 chunk 边界")
                a_ns = r.u32(p)
                a_name = r.u32(p + 4)
                a_raw = r.u32(p + 8)
                dtype = r.u8(p + 15)
                ddata = r.u32(p + 16)
                attrs.append((a_ns, a_name, a_raw, dtype, ddata))
            events.append(("start-tag", (ns, tag, attrs)))
        elif head == CHUNK_END_TAG:
            if size != 24:
                raise ManifestError(f"END_ELEMENT 长度必须是 24，实际 {size}")
            events.append(("end-tag", (r.u32(off + 16), r.u32(off + 20))))
        else:
            report.append(f"[WARN] 未知 chunk 0x{head:08X} @ {off}，已跳过")

        off += size

    if off != total:
        raise ManifestError(f"chunk 链结束于 {off}，应为 {total}")
    report.append(f"chunk 链完整，共 {len(events)} 个 XML 事件")
    return strings, events, report


def short_ns(strings, idx):
    """把 URI 形式的命名空间显示成 android:，提升可读性"""
    if idx == 0xFFFFFFFF or idx >= len(strings):
        return ""
    v = strings[idx]
    if v == ANDROID_NS_URI:
        return "android"
    return v.rsplit("/", 1)[-1]


def s(strings, idx):
    if idx == 0xFFFFFFFF or idx >= len(strings):
        return ""
    return strings[idx]


def render(strings, events):
    """把事件还原成可读 XML"""
    lines = []
    depth = 0
    stack = []
    for kind, payload in events:
        if kind == "start-ns":
            lines.append(f"  xmlns:{s(strings, payload[0])}=\"{s(strings, payload[1])}\"")
            continue
        if kind == "end-ns":
            continue
        if kind == "start-tag":
            ns, tag, attrs = payload
            stack.append(s(strings, tag))
            name = f"{short_ns(strings, ns)}:{s(strings, tag)}" if ns != 0xFFFFFFFF else s(strings, tag)
            parts = []
            for a_ns, a_name, a_raw, dtype, ddata in attrs:
                an = s(strings, a_name)
                if a_ns != 0xFFFFFFFF:
                    an = f"{short_ns(strings, a_ns)}:{an}"
                if dtype == 0x03:  # string
                    val = s(strings, a_raw)
                else:
                    val = str(ddata) if ddata != 0xFFFFFFFF else "true"
                parts.append(f'{an}="{val}"')
            attr_txt = " ".join(parts)
            lines.append("  " * depth + f"<{name}{' ' + attr_txt if attr_txt else ''}>")
            depth += 1
        else:
            depth -= 1
            if depth < 0:
                raise ManifestError("结束标签多于起始标签，嵌套不配对")
            expect = stack.pop()
            got = s(strings, payload[1])
            if expect != got:
                raise ManifestError(f"标签不配对：<{expect}> 却以 </{got}> 结束")
            lines.append("  " * depth + f"</{got}>")
    if stack:
        raise ManifestError(f"有未闭合标签: {stack}")
    return "\n".join(lines)


def check_semantics(strings, events):
    """业务层校验：Android 特有的硬要求"""
    issues = []
    ver_codes = {}
    for kind, payload in events:
        if kind != "start-tag":
            continue
        _ns, tag, attrs = payload
        tname = s(strings, tag)
        for a_ns, a_name, a_raw, dtype, ddata in attrs:
            an = s(strings, a_name)
            if an in ("minSdkVersion", "targetSdkVersion"):
                if dtype != 0x10:
                    issues.append(
                        f"{tname} 的 {an} 用了 0x{dtype:02X} 编码，必须是 0x10"
                        " (TYPE_INT_DEC)，否则 ApkUtils 识别不到 SDK 版本"
                    )
                else:
                    ver_codes[an] = ddata
            if an == "versionCode" and dtype != 0x10:
                issues.append("versionCode 应为 TYPE_INT_DEC 整数编码")
    if "minSdkVersion" not in ver_codes:
        issues.append("未找到 minSdkVersion")
    return issues, ver_codes


def main():
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    path = sys.argv[1]
    with open(path, "rb") as f:
        data = f.read()

    try:
        strings, events, report = parse(data)
        print(f"文件: {path}  ({len(data)} bytes)\n")
        for line in report:
            print("  " + line)

        print("\n--- 还原的 manifest ---")
        print(render(strings, events))

        print("\n--- Android 语义校验 ---")
        issues, ver = check_semantics(strings, events)
        ver_txt = ", ".join(f"{k}={v}" for k, v in sorted(ver.items()))
        print(f"  SDK 版本: {ver_txt or '(未检出)'}")
        if issues:
            for i in issues:
                print(f"  [FAIL] {i}")
            return 1
        print("  [OK] 结构与编码均符合要求")
        return 0
    except ManifestError as e:
        print(f"  [FAIL] {e}")
        return 1


if __name__ == "__main__":
    sys.exit(main())
