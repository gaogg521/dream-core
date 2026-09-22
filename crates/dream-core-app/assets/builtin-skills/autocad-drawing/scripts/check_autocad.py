#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
check_autocad.py — 检测本机 AutoCAD 安装与 COM 自动化能力

用途：判断能否走「COM 直连 AutoCAD 实时绘图」路线（需 AutoCAD 已安装并运行）。
     无法 COM 时，改用 DXF 生成路线（gen_dxf.py），不依赖 AutoCAD。
"""

import sys
import subprocess

APP_IDS = [
    "AutoCAD.Application.23",   # 2019
    "AutoCAD.Application.24",   # 2021
    "AutoCAD.Application.25",   # 2023
    "AutoCAD.Application.26",   # 2025
    "AutoCAD.Application.27",   # 2026
]


def detect_registry():
    """通过 Windows 注册表查找已安装的 AutoCAD。"""
    import winreg
    found = []
    roots = [
        (winreg.HKEY_LOCAL_MACHINE, r"SOFTWARE\Autodesk\AutoCAD"),
        (winreg.HKEY_CURRENT_USER, r"SOFTWARE\Autodesk\AutoCAD"),
    ]
    for root, base in roots:
        try:
            key = winreg.OpenKey(root, base)
        except OSError:
            continue
        i = 0
        while True:
            try:
                ver = winreg.EnumKey(key, i)
                i += 1
                found.append(ver)
            except OSError:
                break
    return sorted(set(found))


def detect_com():
    """检测 COM 自动化是否可用。"""
    try:
        import win32com.client  # noqa: F401
        for appid in APP_IDS:
            try:
                acad = win32com.client.Dispatch(appid)
                name = acad.Name
                return True, name
            except Exception:
                continue
        return False, None
    except ImportError:
        return False, None


def main():
    print("== AutoCAD 检测 ==")
    try:
        versions = detect_registry()
    except Exception:
        versions = []
    if versions:
        print("已安装版本（注册表）：%s" % ", ".join(versions))
    else:
        print("注册表未发现 AutoCAD 安装。")

    ok, name = detect_com()
    if ok:
        print("COM 自动化：可用（当前实例：%s）" % name)
        print("路线：可用 COM 直连实时绘图。")
    else:
        print("COM 自动化：不可用（未运行 AutoCAD 或未装 pywin32）。")
        print("路线：改用 DXF 生成（gen_dxf.py），AutoCAD 直接打开。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
