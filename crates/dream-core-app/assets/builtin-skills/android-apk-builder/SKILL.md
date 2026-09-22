---
name: android-apk-builder
display_name: "Android APK 构建器（免 aapt2）"
description: 在缺少 aapt2 或完整 Android SDK 的环境中，仅用 JDK + d8 + jarsigner 构建可安装的 APK。手写二进制 AndroidManifest.xm。触发：用户提出「Android APK 构建器（免 aapt2）」相关需求时使用（常见说法：Android APK 构建器（免 aapt2）、Android APK 构建器（免 aapt2）；英文：android/apk/builder）；不要用于“生成或编辑 Word/Excel/PPT 文档文件”（改用 officecli-docx / officecli-xlsx / officecli-pptx），也不要用于与本技能无关的其他任务。需要用户提供：明确的任务描述，以及必要的输入文件或数据。
---


# Android APK 构建器（免 aapt2）

在装不了完整 Android SDK 的机器上，用 **JDK + d8.jar + jarsigner** 构建出可安装的 APK。

## 适用边界（先看这个）

✅ **适合**：WebView 壳应用、内部小工具、把网页/HTML5 项目包成 App、快速原型验证
❌ **不适合**：需要 XML 布局、`R.layout.*` 引用、AndroidX 依赖、多资源适配的应用

原因：没有 aapt2 就无法编译 `res/` 资源与生成 `R.java`。本方案要求 UI 全部用 Java 代码创建。

## 快速开始

### 第 0 步：确认 manifest 能生成（不需要 SDK，30 秒验证环境）

```bash
python scripts/build_apk.py --manifest-only \
    --out-dir ./build_output \
    --package com.example.myapp \
    --app-name "我的应用"

python scripts/verify_manifest.py ./build_output/AndroidManifest.xml
```

看到 `[OK] 结构与编码均符合要求` 再往下走。**这一步能挡掉 90% 的失败**。

### 第 1 步：准备工程结构

```
android-project/
└── app/src/main/
    ├── java/com/example/myapp/MainActivity.java
    └── assets/www/index.html     ← 网页资源放这里
```

### 第 2 步：完整构建

```bash
python scripts/build_apk.py \
    --project ./android-project \
    --sdk-dir ./android-sdk \
    --out-dir ./build_output \
    --package com.example.myapp \
    --app-name "我的应用" \
    --permission android.permission.INTERNET \
    --permission android.permission.CAMERA
```

工具链会自动探测：JDK 读 `JAVA_HOME`（或 `--jdk-home`），`android.jar` 取 `platforms/` 下版本最高的，`d8.jar` 与 `zipalign` 取 `build-tools/` 下版本最高的。

## 全部参数

| 参数 | 默认 | 说明 |
|---|---|---|
| `--project` | `./android-project` | 工程根，下含 `app/src/main/{java,assets}` |
| `--package` | `com.example.app` | 应用包名 |
| `--app-name` | `MyApp` | 显示名，同时作为输出文件名 |
| `--activity` | `.MainActivity` | 入口 Activity |
| `--version-code` / `--version-name` | `1` / `1.0` | 版本 |
| `--min-sdk` / `--target-sdk` | `21` / `34` | SDK 版本 |
| `--permission` | INTERNET | 权限，**可重复传** |
| `--sdk-dir` | `$ANDROID_HOME` | SDK 根目录 |
| `--jdk-home` | `$JAVA_HOME` | JDK 根目录 |
| `--out-dir` | `./build_output` | 输出目录 |
| `--keystore` / `--key-alias` / `--store-pass` / `--key-pass` | debug 相关 | 签名配置 |
| `--apk-name` | 取 app-name | 输出文件名 |
| `--manifest-only` | 关 | 只生成 manifest |

**没有任何项目名、包名或路径被硬编码在脚本里**，全部由参数传入。

## 六步流水线

| 步 | 做什么 | 工具 |
|---|---|---|
| 1 | 编译 Java，target 8，`-bootclasspath` 指向 android.jar | javac |
| 2 | class → dex | d8 |
| 3 | **手写二进制 AndroidManifest.xml** | 纯 Python |
| 4 | 生成/复用签名密钥 | keytool |
| 5 | 打 zip（manifest 用 ZIP_STORED） | zipfile |
| 6 | V1 签名 + zipalign | jarsigner |

**为什么用 jarsigner 不用 apksigner**：apksigner 会校验二进制 XML 的合法性，对手写的 manifest 必然失败；jarsigner 只签名 zip 条目、不解析 XML。代价是只能有 V1 签名（Android 7 起支持 V2/V3，但 V1 仍被所有版本接受）。

## 坑位清单（每一条都是踩出来的）

1. **字符串池必须 UTF-16**（flags=0）。UTF-8 模式下每项需要 LEB128 双长度前缀，手写极易漏，实测报 `string not NULL terminated`
2. **namespace chunk 是 24 字节不是 16**。写错会导致后续所有 chunk 偏移错位，报错信息完全看不出原因
3. **minSdkVersion / targetSdkVersion 必须整数编码** `TYPE_INT_DEC`(0x10)，不能是字符串。否则 ApkUtils 识别不到，安装器直接拒收
4. **属性的 ns 字段指向 URI，不是前缀**。填 `"android"` 会让严格解析器忽略所有 `android:` 命名空间的属性——表现为 manifest 看着对，但 versionCode、minSdkVersion 全读不到。**这个错误不会报任何错，最难查**
5. **attributeStart / attributeSize / attributeCount 是 uint16 不是 uint32**，用 `<H` 不是 `<I`
6. **属性必须嵌在 start tag chunk 内部**（紧跟 36 字节头），不能追加在后面
7. **AndroidManifest.xml 用 ZIP_STORED 不压缩**，Android 直接从 zip 读它
8. **APK 里不能有空目录条目**。`res/`、`META-INF/` 这类空项会让小米、华为、OPPO 等机型的安装器直接拒收。只写真实文件
9. **Android 12+ 要求带 intent-filter 的 activity 显式声明 `android:exported`**，否则安装时报 `INSTALL_FAILED_VERIFICATION_FAILURE`
10. **负值用 `& 0xFFFFFFFF`**，struct.pack 不接受 -1

## 验证

### 结构验证（内置）

```bash
python scripts/verify_manifest.py build_output/AndroidManifest.xml
```

会走查：文件头魔数 → chunk 链长度闭合 → 字符串池 UTF-16 与 NULL 结尾 → 标签嵌套配对 → SDK 版本整数编码。有错明确指出位置，退出码非 0。

### 第三方交叉验证（推荐）

装一个独立的 AXML 解析器，确认产物不是"自己说自己对"：

```bash
pip install pyaxmlparser
python -c "from pyaxmlparser.axmlprinter import AXMLPrinter; \
print(AXMLPrinter(open('build_output/AndroidManifest.xml','rb').read()).get_xml().decode())"
```

正常应输出完整的 `android:versionCode`、`android:minSdkVersion` 等属性。**如果看到 `ns0:` 前缀，说明属性 ns 填错了**（对应坑位 4）。

## 获取 SDK 组件

如果机器上没有 `android.jar` 和 `d8.jar`：

1. 取仓库索引 `https://dl.google.com/android/repository/repository2-1.xml`
2. 搜索 `<remotePackage path="platforms;android-34">` 得到 platform 包文件名
3. 搜索 `<remotePackage path="build-tools;34.0.0">` 得到 build-tools 文件名
4. 下载 `https://dl.google.com/android/repository/<文件名>` 并解压

只需要这两个文件，**不需要下载完整 SDK**：
- `platforms/android-XX/android.jar`
- `build-tools/XX.X.X/lib/d8.jar`（`zipalign` 在同目录，可选）

国内访问 Google 缓慢时可用镜像站搜索同名文件。

## Java 源码要求

MainActivity 必须纯代码建 UI，不引用任何 `R.*`：

```java
public class MainActivity extends Activity {
    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        WebView webView = new WebView(this);
        webView.setLayoutParams(new FrameLayout.LayoutParams(MATCH_PARENT, MATCH_PARENT));
        webView.getSettings().setJavaScriptEnabled(true);
        setContentView(webView);
        webView.loadUrl("file:///android_asset/www/index.html");
    }
}
```

只用 android.jar 里的类，不要引入 AndroidX 或第三方库（没有资源编译与依赖打包环节）。

## 已知边界

- **只做 V1 签名**：因 apksigner 无法校验手写 manifest。V1 在所有 Android 版本都可用，但不满足 V2/V3 要求的应用商店
- **无应用图标**：不带 `res/` 资源，安装后显示系统默认图标
- **本 skill 已验证到**：二进制 manifest 的结构合法性（内置验证器 + 第三方 AXML 解析器交叉确认）。完整构建链路需在具备 JDK 与 SDK 组件的环境执行
