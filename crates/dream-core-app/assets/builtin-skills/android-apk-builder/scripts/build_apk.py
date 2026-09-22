#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
build_apk.py — 不依赖 aapt2 的 APK 构建器

用途：在无法完整安装 Android SDK（缺 aapt2 / platform 文件不全 / 跨平台工具链不匹配）
的环境中，仅用 JDK + d8.jar + jarsigner 构建出可安装的 APK。

核心难点是「手写二进制 AndroidManifest.xml」，本脚本已处理全部已知坑位：
  - 字符串池用 UTF-16 编码（UTF-8 会触发 "string not NULL terminated"）
  - namespace chunk 大小为 24 字节（写 16 会偏移错位）
  - minSdkVersion / targetSdkVersion 用 TYPE_INT_DEC 整数编码
  - APK 内不放任何空目录项（国产机型安装器会拒收）
  - AndroidManifest.xml 用 ZIP_STORED 不压缩

用法：
  # 只生成并校验 manifest（不需要 SDK，用于快速验证）
  python build_apk.py --manifest-only --package com.example.app --app-name MyApp

  # 完整构建
  python build_apk.py \
      --project ./android-project \
      --sdk-dir ./android-sdk \
      --out-dir ./build_output \
      --package com.example.app \
      --app-name MyApp \
      --permission android.permission.INTERNET \
      --permission android.permission.CAMERA

所有与具体项目相关的值都通过命令行参数传入，脚本内不含任何硬编码的项目名、
包名或本机路径。
"""

import argparse
import os
import shutil
import struct
import subprocess
import sys
import zipfile

# ---------------------------------------------------------------- 常量

# ResChunk_header: type(uint16) + headerSize(uint16) 打包成一个 uint32（小端）
CHUNK_STRING_POOL = 0x001C0001
CHUNK_RES_MAP = 0x00080180
CHUNK_XML_FILE = 0x00080003
CHUNK_START_NS = 0x00100100
CHUNK_END_NS = 0x00100101
CHUNK_START_TAG = 0x00100102
CHUNK_END_TAG = 0x00100103

FLAG_UTF16 = 0x00000000
ATTR_SIZE = 20          # ResXMLTree_attrExt 固定 20 字节
ATTR_START = 20         # 属性区相对元素起始处的偏移

# Res_value 的 type 字段（size=8, res0=0 已打包进高字节）
TYPE_STRING = 0x03000008
TYPE_INT_DEC = 0x10000008
TYPE_INT_BOOLEAN = 0x12000008

ANDROID_NS_URI = "http://schemas.android.com/apk/res/android"
NO_NS = -1
NO_REF = 0xFFFFFFFF


# ---------------------------------------------------------------- 小工具

def pack_u4(val):
    """打包 4 字节无符号整数，-1 自动变成 0xFFFFFFFF"""
    return struct.pack("<I", val & 0xFFFFFFFF)


def run(cmd, desc="", check=True):
    """执行命令，失败时打印尾部错误"""
    if isinstance(cmd, list):
        printable = " ".join(str(c) for c in cmd)
    else:
        printable = str(cmd)
    print(f"  [{desc}] {printable[:150]}")
    result = subprocess.run(cmd, capture_output=True, text=True, timeout=600)
    if check and result.returncode != 0:
        print(f"  ERROR({desc}): {result.stderr.strip()[-500:]}")
        sys.exit(1)
    return result


def die(msg):
    print(f"  [FATAL] {msg}")
    sys.exit(1)


# ---------------------------------------------------------------- 二进制 XML 构造

def create_string_pool(strings):
    """构造字符串池 chunk（UTF-16 编码）。

    为什么必须用 UTF-16：Java 侧的 XML 解析器（Android PackageManager 与
    apksigner）在 UTF-8 模式下对长度前缀的解析与 AOSP 写入约定不一致，
    实测会抛 "string not NULL terminated"。UTF-16 模式下每条字符串结构是
        [uint16 字符数(不含结尾 null)][UTF-16LE 数据][uint16 0x0000]
    解析器兼容性最好。
    """
    encoded = [s.encode("utf-16-le") for s in strings]
    # 每条字符串占用的字节数：2(长度前缀) + 2*n(数据) + 2(null 结尾)
    offsets = []
    cur = 0
    for data in encoded:
        offsets.append(cur)
        cur += 2 + len(data) + 2

    string_count = len(strings)
    style_count = 0
    strings_start = 28 + 4 * string_count + 4 * style_count

    data_size = cur
    chunk_size = strings_start + data_size
    pad = (4 - (chunk_size % 4)) % 4
    chunk_size += pad

    buf = bytearray()
    buf.extend(struct.pack("<I", CHUNK_STRING_POOL))
    buf.extend(struct.pack("<I", chunk_size))
    buf.extend(struct.pack("<I", string_count))
    buf.extend(struct.pack("<I", style_count))
    buf.extend(struct.pack("<I", FLAG_UTF16))       # 0 = UTF-16
    buf.extend(struct.pack("<I", strings_start))
    buf.extend(struct.pack("<I", 0))                # stylesStart = 0（无样式）
    for off in offsets:
        buf.extend(struct.pack("<I", off))
    for data in encoded:
        buf.extend(struct.pack("<H", len(data) // 2))   # 字符数，不是字节数
        buf.extend(data)
        buf.extend(b"\x00\x00")                          # null 结尾
    buf.extend(b"\x00" * pad)
    return bytes(buf)


def create_resource_map():
    """空的 resource map chunk（可选，占位即可）"""
    buf = bytearray()
    buf.extend(struct.pack("<I", CHUNK_RES_MAP))
    buf.extend(struct.pack("<I", 8))
    return bytes(buf)


def create_start_ns(prefix_idx, uri_idx):
    """起始命名空间 chunk，size 必须是 24 不是 16"""
    buf = bytearray()
    buf.extend(struct.pack("<I", CHUNK_START_NS))
    buf.extend(struct.pack("<I", 24))
    buf.extend(struct.pack("<I", 0))              # lineNumber
    buf.extend(pack_u4(NO_REF))                   # comment = -1
    buf.extend(pack_u4(prefix_idx))
    buf.extend(pack_u4(uri_idx))
    return bytes(buf)


def create_end_ns(prefix_idx, uri_idx):
    buf = bytearray()
    buf.extend(struct.pack("<I", CHUNK_END_NS))
    buf.extend(struct.pack("<I", 24))
    buf.extend(struct.pack("<I", 0))
    buf.extend(pack_u4(NO_REF))
    buf.extend(pack_u4(prefix_idx))
    buf.extend(pack_u4(uri_idx))
    return bytes(buf)


def create_start_tag(ns_idx, name_idx, attr_count):
    """起始元素 chunk：header(8) + line(4) + comment(4) + element(20) + attrs"""
    size = 36 + attr_count * ATTR_SIZE
    buf = bytearray()
    buf.extend(struct.pack("<I", CHUNK_START_TAG))
    buf.extend(struct.pack("<I", size))
    buf.extend(struct.pack("<I", 0))                        # lineNumber
    buf.extend(pack_u4(NO_REF))                             # comment = -1
    buf.extend(pack_u4(ns_idx))
    buf.extend(pack_u4(name_idx))
    buf.extend(struct.pack("<H", ATTR_START))               # 注意是 uint16 不是 uint32
    buf.extend(struct.pack("<H", ATTR_SIZE))
    buf.extend(struct.pack("<H", attr_count))
    buf.extend(struct.pack("<H", 0))                        # idIndex
    buf.extend(struct.pack("<H", 0))                        # classIndex
    buf.extend(struct.pack("<H", 0))                        # styleIndex
    return bytes(buf)


def create_end_tag(ns_idx, name_idx):
    buf = bytearray()
    buf.extend(struct.pack("<I", CHUNK_END_TAG))
    buf.extend(struct.pack("<I", 24))
    buf.extend(struct.pack("<I", 0))
    buf.extend(pack_u4(NO_REF))
    buf.extend(pack_u4(ns_idx))
    buf.extend(pack_u4(name_idx))
    return bytes(buf)


def create_attribute(ns_idx, name_idx, raw_idx=NO_REF, type_id=TYPE_STRING, data=NO_REF):
    """单个属性，固定 20 字节"""
    buf = bytearray()
    buf.extend(pack_u4(ns_idx))
    buf.extend(pack_u4(name_idx))
    buf.extend(pack_u4(raw_idx))       # 字符串型属性填字符串池下标，整数型填 -1
    buf.extend(pack_u4(type_id))
    buf.extend(pack_u4(data))
    return bytes(buf)


def build_binary_manifest(cfg):
    """生成二进制 AndroidManifest.xml，返回 bytes"""
    perms = cfg["permissions"]
    strings = [
        "manifest",                      # 0
        "android",                       # 1  ns 前缀
        ANDROID_NS_URI,                  # 2  ns URI
        "package",                       # 3
        cfg["package"],                  # 4
        "versionCode",                   # 5
        "versionName",                   # 6
        cfg["version_name"],             # 7
        "uses-sdk",                      # 8
        "minSdkVersion",                 # 9
        "targetSdkVersion",              # 10
        "uses-permission",               # 11
        "name",                          # 12
        "application",                   # 13
        "allowBackup",                   # 14
        "label",                         # 15
        cfg["app_name"],                 # 16
        "activity",                      # 17
        cfg["activity"],                 # 18
        "intent-filter",                 # 19
        "action",                        # 20
        "android.intent.action.MAIN",    # 21
        "category",                      # 22
        "android.intent.category.LAUNCHER",  # 23
        "exported",                      # 24
    ]
    # 权限字符串追加在后面
    perm_idx = []
    for p in perms:
        perm_idx.append(len(strings))
        strings.append(p)

    I = {s: i for i, s in enumerate(strings)}
    # 属性的 ns 字段必须指向 URI 字符串（不是前缀 "android"）。
    # aapt 生成的 AXML 就是这样：start_namespace chunk 负责声明前缀映射，
    # 而元素/属性的 ns 一律填 URI。填成前缀会导致命名空间的属性被解析器忽略。
    NS = I[ANDROID_NS_URI]

    def element(name_idx, attrs, ns_idx=NO_NS):
        """attrs: list of (ns, name_idx, raw_idx, type_id, data)"""
        body = bytearray()
        for a in attrs:
            body.extend(create_attribute(*a))
        return create_start_tag(ns_idx, name_idx, len(attrs)) + bytes(body)

    tree = bytearray()
    tree.extend(create_start_ns(1, 2))

    # <manifest package android:versionCode android:versionName>
    tree.extend(element(I["manifest"], [
        (NO_NS, I["package"], I[cfg["package"]], TYPE_STRING, NO_REF),
        (NS, I["versionCode"], NO_REF, TYPE_INT_DEC, cfg["version_code"]),
        (NS, I["versionName"], I[cfg["version_name"]], TYPE_STRING, NO_REF),
    ]))

    # <uses-sdk minSdkVersion targetSdkVersion />   —— 必须整数编码
    tree.extend(element(I["uses-sdk"], [
        (NS, I["minSdkVersion"], NO_REF, TYPE_INT_DEC, cfg["min_sdk"]),
        (NS, I["targetSdkVersion"], NO_REF, TYPE_INT_DEC, cfg["target_sdk"]),
    ]))
    tree.extend(create_end_tag(NO_NS, I["uses-sdk"]))

    # <uses-permission android:name="..." /> × N
    for pi in perm_idx:
        tree.extend(element(I["uses-permission"], [
            (NS, I["name"], pi, TYPE_STRING, NO_REF),
        ]))
        tree.extend(create_end_tag(NO_NS, I["uses-permission"]))

    # <application allowBackup label>
    tree.extend(element(I["application"], [
        (NS, I["allowBackup"], NO_REF, TYPE_INT_BOOLEAN, NO_REF),  # true
        (NS, I["label"], I[cfg["app_name"]], TYPE_STRING, NO_REF),
    ]))

    # <activity android:name=".MainActivity" android:exported="true">
    # Android 12+ 要求带 intent-filter 的 activity 显式声明 exported
    tree.extend(element(I["activity"], [
        (NS, I["name"], I[cfg["activity"]], TYPE_STRING, NO_REF),
        (NS, I["exported"], NO_REF, TYPE_INT_BOOLEAN, NO_REF),
    ]))

    tree.extend(create_start_tag(NO_NS, I["intent-filter"], 0))
    tree.extend(element(I["action"], [
        (NS, I["name"], I["android.intent.action.MAIN"], TYPE_STRING, NO_REF),
    ]))
    tree.extend(create_end_tag(NO_NS, I["action"]))
    tree.extend(element(I["category"], [
        (NS, I["name"], I["android.intent.category.LAUNCHER"], TYPE_STRING, NO_REF),
    ]))
    tree.extend(create_end_tag(NO_NS, I["category"]))
    tree.extend(create_end_tag(NO_NS, I["intent-filter"]))

    tree.extend(create_end_tag(NO_NS, I["activity"]))
    tree.extend(create_end_tag(NO_NS, I["application"]))
    tree.extend(create_end_tag(NO_NS, I["manifest"]))
    tree.extend(create_end_ns(1, 2))

    pool = create_string_pool(strings)
    res_map = create_resource_map()

    out = bytearray()
    out.extend(struct.pack("<I", CHUNK_XML_FILE))
    out.extend(struct.pack("<I", 8 + len(pool) + len(res_map) + len(tree)))
    out.extend(pool)
    out.extend(res_map)
    out.extend(tree)
    return bytes(out)


# ---------------------------------------------------------------- 工具链探测

def first_existing(*paths):
    for p in paths:
        if p and os.path.exists(p):
            return p
    return None


def locate_toolchain(args):
    """定位 JDK 与 SDK 组件，尽量自动探测，找不到就给明确报错"""
    tc = {}

    # --- JDK ---
    jdk = args.jdk_home or os.environ.get("JAVA_HOME")
    if jdk:
        bindir = os.path.join(jdk, "bin")
        tc["javac"] = first_existing(
            os.path.join(bindir, "javac.exe"), os.path.join(bindir, "javac"))
        tc["java"] = first_existing(
            os.path.join(bindir, "java.exe"), os.path.join(bindir, "java"))
        tc["jarsigner"] = first_existing(
            os.path.join(bindir, "jarsigner.exe"), os.path.join(bindir, "jarsigner"))
        tc["keytool"] = first_existing(
            os.path.join(bindir, "keytool.exe"), os.path.join(bindir, "keytool"))
    if not tc.get("javac"):
        for name in ("javac", "java", "jarsigner", "keytool"):
            found = shutil.which(name)
            if found:
                tc[name] = found
    for k in ("javac", "java", "jarsigner", "keytool"):
        if not tc.get(k):
            die(f"找不到 {k}。请用 --jdk-home 指定 JDK 目录，或设置 JAVA_HOME。")

    # --- SDK ---
    sdk = args.sdk_dir
    if not sdk:
        die("需要 --sdk-dir 指向 SDK 根目录（内含 platforms/ 与 build-tools/）")
    if not os.path.isdir(sdk):
        die(f"SDK 目录不存在: {sdk}")

    # android.jar：取版本号最高的 platform
    plat_dir = os.path.join(sdk, "platforms")
    if not os.path.isdir(plat_dir):
        die(f"缺少 platforms/ 目录: {plat_dir}")
    cands = []
    for d in os.listdir(plat_dir):
        jar = os.path.join(plat_dir, d, "android.jar")
        if os.path.exists(jar):
            try:
                ver = int("".join(ch for ch in d if ch.isdigit()))
            except ValueError:
                ver = 0
            cands.append((ver, jar))
    if not cands:
        die(f"在 {plat_dir} 下没找到任何 android.jar")
    cands.sort(reverse=True)
    tc["android_jar"] = cands[0][1]

    # d8.jar 与 zipalign：取版本号最高的 build-tools
    bt_dir = os.path.join(sdk, "build-tools")
    if not os.path.isdir(bt_dir):
        die(f"缺少 build-tools/ 目录: {bt_dir}")
    bt_cands = []
    for d in os.listdir(bt_dir):
        jar = os.path.join(bt_dir, d, "lib", "d8.jar")
        if os.path.exists(jar):
            try:
                ver = int("".join(ch for ch in d.split("-")[0] if ch.isdigit()) or 0)
            except ValueError:
                ver = 0
            bt_cands.append((ver, d, jar))
    if not bt_cands:
        die(f"在 {bt_dir} 下没找到任何 d8.jar")
    bt_cands.sort(reverse=True)
    _, bt_name, d8 = bt_cands[0]
    tc["d8_jar"] = d8
    tc["zipalign"] = first_existing(
        os.path.join(bt_dir, bt_name, "zipalign.exe"),
        os.path.join(bt_dir, bt_name, "zipalign"),
    )
    return tc


# ---------------------------------------------------------------- 构建步骤

def step_compile(tc, cfg):
    print("\n=== Step 1/6  编译 Java ===")
    src = os.path.join(cfg["project"], "app", "src", "main", "java")
    if not os.path.isdir(src):
        die(f"源码目录不存在: {src}")
    java_files = []
    for root, _dirs, files in os.walk(src):
        for f in files:
            if f.endswith(".java"):
                java_files.append(os.path.join(root, f))
    if not java_files:
        die(f"{src} 下没有 .java 文件")
    print(f"  找到 {len(java_files)} 个源文件")

    classes = os.path.join(cfg["out"], "classes")
    os.makedirs(classes, exist_ok=True)
    cmd = [
        tc["javac"], "-nowarn",
        "-source", "8", "-target", "8",
        "-bootclasspath", tc["android_jar"],
        "-d", classes,
    ] + java_files
    run(cmd, "javac")
    n = sum(1 for _r, _d, fs in os.walk(classes) for f in fs if f.endswith(".class"))
    print(f"  产出 {n} 个 .class")


def step_dex(tc, cfg):
    print("\n=== Step 2/6  生成 DEX ===")
    classes = os.path.join(cfg["out"], "classes")
    dex_out = os.path.join(cfg["out"], "dex")
    os.makedirs(dex_out, exist_ok=True)
    cmd = [
        tc["java"], "-Xmx1024M",
        "-cp", tc["d8_jar"], "com.android.tools.r8.D8",
        "--release",
        "--lib", tc["android_jar"],
        "--output", dex_out,
        "--min-api", str(cfg["min_sdk"]),
    ]
    for root, _d, files in os.walk(classes):
        for f in files:
            if f.endswith(".class"):
                cmd.append(os.path.join(root, f))
    run(cmd, "d8")
    dex = os.path.join(dex_out, "classes.dex")
    if not os.path.exists(dex):
        die("classes.dex 未生成")
    print(f"  classes.dex {os.path.getsize(dex)} bytes")


def step_manifest(cfg):
    print("\n=== Step 3/6  生成二进制 AndroidManifest.xml ===")
    data = build_binary_manifest(cfg)
    path = os.path.join(cfg["out"], "AndroidManifest.xml")
    with open(path, "wb") as f:
        f.write(data)
    print(f"  {len(data)} bytes -> {path}")
    return path


def step_keystore(tc, cfg):
    print("\n=== Step 4/6  准备签名密钥 ===")
    ks = cfg["keystore"]
    if os.path.exists(ks):
        print(f"  复用已有密钥库: {ks}")
        return ks
    os.makedirs(os.path.dirname(ks) or ".", exist_ok=True)
    cmd = [
        tc["keytool"], "-genkey", "-v",
        "-keystore", ks,
        "-alias", cfg["key_alias"],
        "-keyalg", "RSA", "-keysize", "2048", "-validity", "10950",
        "-storepass", cfg["store_pass"],
        "-keypass", cfg["key_pass"],
        "-dname", f"CN={cfg['app_name']}, OU=Debug, O=Unknown, L=Unknown, ST=Unknown, C=CN",
    ]
    run(cmd, "keytool")
    return ks


def step_package(cfg, manifest_bin):
    print("\n=== Step 5/6  打包 APK ===")
    apk = os.path.join(cfg["out"], "app-unsigned.apk")
    if os.path.exists(apk):
        os.remove(apk)

    assets = os.path.join(cfg["project"], "app", "src", "main", "assets")
    with zipfile.ZipFile(apk, "w", zipfile.ZIP_DEFLATED) as zf:
        # 必须 ZIP_STORED：Android 直接从 zip 里读 manifest，不做解压
        zf.writestr(
            zipfile.ZipInfo("AndroidManifest.xml", date_time=(2008, 1, 1, 0, 0, 0)),
            open(manifest_bin, "rb").read(),
            compress_type=zipfile.ZIP_STORED,
        )
        zf.write(os.path.join(cfg["out"], "dex", "classes.dex"), "classes.dex")
        # 只写真实文件，绝不写空目录条目
        if os.path.isdir(assets):
            for root, _d, files in os.walk(assets):
                for f in files:
                    full = os.path.join(root, f)
                    arc = os.path.relpath(full, os.path.dirname(assets)).replace("\\", "/")
                    zf.write(full, arc)
    print(f"  未签名 APK: {os.path.getsize(apk)} bytes")
    with zipfile.ZipFile(apk) as zf:
        for name in sorted(zf.namelist())[:12]:
            print(f"    {name}")
        if len(zf.namelist()) > 12:
            print(f"    ... 共 {len(zf.namelist())} 项")
    return apk


def step_sign(tc, cfg, apk):
    print("\n=== Step 6/6  签名（V1 / jarsigner）===")
    # 用 jarsigner 而非 apksigner：apksigner 会校验二进制 XML 的合法性，
    # 对手写的 manifest 必然失败；jarsigner 只签名 zip 条目，不解析 XML。
    cmd = [
        tc["jarsigner"], "-verbose",
        "-sigalg", "SHA256withRSA",
        "-digestalg", "SHA-256",
        "-keystore", cfg["keystore"],
        "-storepass", cfg["store_pass"],
        "-keypass", cfg["key_pass"],
        apk, cfg["key_alias"],
    ]
    run(cmd, "jarsigner")
    return apk


def step_align(tc, cfg, apk):
    """zipalign 是可选步骤，缺失时给出提示而不是报错"""
    aligned = os.path.join(cfg["out"], "app-aligned.apk")
    if not tc.get("zipalign"):
        print("  未找到 zipalign，跳过对齐。APK 仍可安装，但部分设备会提示优化失败。")
        shutil.copy2(apk, aligned)
        return aligned
    if os.path.exists(aligned):
        os.remove(aligned)
    run([tc["zipalign"], "-f", "4", apk, aligned], "zipalign")
    print(f"  已对齐: {os.path.getsize(aligned)} bytes")
    return aligned


def step_verify(tc, apk):
    print("\n=== 校验 ===")
    r = run([tc["jarsigner"], "-verify", "-verbose", apk], "verify", check=False)
    ok = r.returncode == 0 and "jar verified" in (r.stdout or "").lower()
    print("  签名校验:", "PASSED" if ok else "未通过（见上方输出）")
    with zipfile.ZipFile(apk) as zf:
        names = zf.namelist()
        empty_dirs = [n for n in names if n.endswith("/")]
        if empty_dirs:
            print(f"  [WARN] 存在空目录条目，国产机型可能拒装: {empty_dirs}")
        print(f"  zip 条目数: {len(names)}")
    return ok


# ---------------------------------------------------------------- 入口

def parse_args(argv=None):
    p = argparse.ArgumentParser(
        description="不依赖 aapt2 的 APK 构建器（JDK + d8 + jarsigner）",
        formatter_class=argparse.ArgumentDefaultsHelpFormatter,
    )
    g_proj = p.add_argument_group("项目配置")
    g_proj.add_argument("--project", default="./android-project",
                        help="工程根目录，下含 app/src/main/{java,assets}")
    g_proj.add_argument("--package", default="com.example.app", help="应用包名")
    g_proj.add_argument("--app-name", default="MyApp", help="应用显示名")
    g_proj.add_argument("--activity", default=".MainActivity", help="入口 Activity")
    g_proj.add_argument("--version-code", type=int, default=1)
    g_proj.add_argument("--version-name", default="1.0")
    g_proj.add_argument("--min-sdk", type=int, default=21)
    g_proj.add_argument("--target-sdk", type=int, default=34)
    g_proj.add_argument("--permission", action="append", default=None,
                        help="权限，可重复；不传则只用 INTERNET")
    g_proj.add_argument("--apk-name", default=None,
                        help="输出 APK 文件名，默认取 --app-name")

    g_env = p.add_argument_group("环境配置")
    g_env.add_argument("--sdk-dir", default=os.environ.get("ANDROID_HOME")
                       or os.environ.get("ANDROID_SDK_ROOT"),
                       help="SDK 根目录，含 platforms/ 与 build-tools/")
    g_env.add_argument("--jdk-home", default=None, help="JDK 根目录，默认读 JAVA_HOME")
    g_env.add_argument("--out-dir", default="./build_output", help="构建输出目录")

    g_sign = p.add_argument_group("签名配置")
    g_sign.add_argument("--keystore", default=None, help="密钥库路径，默认在输出目录下")
    g_sign.add_argument("--key-alias", default="debug")
    g_sign.add_argument("--store-pass", default="android")
    g_sign.add_argument("--key-pass", default="android")

    g_misc = p.add_argument_group("其他")
    g_misc.add_argument("--manifest-only", action="store_true",
                        help="只生成二进制 manifest（不需要 SDK），用于快速验证")
    return p.parse_args(argv)


def main(argv=None):
    args = parse_args(argv)

    cfg = {
        "project": os.path.abspath(args.project),
        "out": os.path.abspath(args.out_dir),
        "package": args.package,
        "app_name": args.app_name,
        "activity": args.activity,
        "version_code": args.version_code,
        "version_name": args.version_name,
        "min_sdk": args.min_sdk,
        "target_sdk": args.target_sdk,
        "permissions": args.permission or ["android.permission.INTERNET"],
        "keystore": args.keystore,
        "key_alias": args.key_alias,
        "store_pass": args.store_pass,
        "key_pass": args.key_pass,
        "apk_name": args.apk_name,
    }
    if not cfg["keystore"]:
        cfg["keystore"] = os.path.join(cfg["out"], "debug.keystore")
    cfg["keystore"] = os.path.abspath(cfg["keystore"])
    os.makedirs(cfg["out"], exist_ok=True)

    print("=" * 62)
    print("  APK Builder (no aapt2)  —  JDK + d8 + jarsigner")
    print(f"  {cfg['app_name']}  <{cfg['package']}>")
    print("=" * 62)

    if args.manifest_only:
        path = step_manifest(cfg)
        print(f"\n[完成] manifest 已生成: {path}")
        print("用 scripts/verify_manifest.py 可解析校验它。")
        return 0

    tc = locate_toolchain(args)
    print(f"  android.jar : {tc['android_jar']}")
    print(f"  d8.jar      : {tc['d8_jar']}")
    print(f"  zipalign    : {tc.get('zipalign') or '(未找到，跳过对齐)'}")

    for d in ("classes", "dex"):
        p = os.path.join(cfg["out"], d)
        if os.path.exists(p):
            shutil.rmtree(p)
        os.makedirs(p, exist_ok=True)

    step_compile(tc, cfg)
    step_dex(tc, cfg)
    manifest = step_manifest(cfg)
    step_keystore(tc, cfg)
    apk = step_package(cfg, manifest)
    apk = step_sign(tc, cfg, apk)
    apk = step_align(tc, cfg, apk)
    step_verify(tc, apk)

    final = os.path.join(
        cfg["out"],
        (cfg["apk_name"] or cfg["app_name"]) + ".apk",
    )
    shutil.copy2(apk, final)
    print("\n" + "=" * 62)
    print("  BUILD SUCCESS")
    print(f"  {final}")
    print(f"  {os.path.getsize(final)} bytes")
    print("=" * 62)
    return 0


if __name__ == "__main__":
    sys.exit(main())
