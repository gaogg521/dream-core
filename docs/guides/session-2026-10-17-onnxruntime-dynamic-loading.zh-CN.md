# 2026-10-17：ONNX Runtime 改为运行时动态加载（修 v0.1.88 发版构建）

## 现象

v0.1.88 的 Release 工作流 5 个平台只过了 macOS 苹果芯片，其余 4 个编译失败，GitHub 上留下一个没有任何安装文件的空发布。起因是知识库的「默认本地模型」（`4e0fc3e`）引入了 `fastembed` → `ort`，特性用的是 `ort-download-binaries-rustls-tls`：构建时下载 pyke 预编译好的**静态** onnxruntime 并链接进 `dreamcore`。

## 四个平台是四种不同的失败

| 平台 | 报错 | 真因 |
| --- | --- | --- |
| x86_64-apple-darwin | `ort-sys: no prebuilt binaries available for target` | pyke 根本没有这个目标的预编译库（微软官方也没有） |
| Linux x64 | `undefined symbol: __isoc23_strtol`、`std::__cxx11::basic_string::_M_replace_cold` | 预编译库用新工具链编的，要 glibc ≥ 2.38 和新 libstdc++；发版刻意固定在 ubuntu-22.04（glibc 2.35），产物基线 `GLIBC_2.34`（`check-glibc-baseline.sh`） |
| Linux arm64 | `undefined reference to std::__throw_bad_array_new_length()` | 同上，`cross` 镜像是 gcc 9、基线 `GLIBC_2.30`，更老 |
| Windows | `LNK4098 defaultlib MSVCRT conflicts`，最后 `LNK1120` | 预编译库按动态 CRT（`/MD`）编，发版用 `-C target-feature=+crt-static` |

只有 Docker 企业镜像一直是好的：它是 `rust:1.95-trixie`，工具链本来就够新。所以 Codex 在容器里验收通过、没有暴露问题。

**把发版工具链升到能链接的程度不可行**：升 glibc 基线等于放弃 Debian 12 / Ubuntu 22.04 / RHEL 9（发版注释里写明的目标用户），改 `+crt-static` 等于要求用户机器装 VC++ 运行库，否则 `dreamcore.exe` 整个起不来。

## 做法：库放在二进制旁边，运行时加载

`fastembed` 改用 `ort-load-dynamic`，`dreamcore` 不再链接 onnxruntime。

- 加载逻辑在 `crates/dream-domain-devops/src/onnxruntime.rs`，本地嵌入初始化前调用 `ensure_loaded()`。
  - **必须先显式加载**：`ort` 在找不到库时是 `panic`（`expect("Failed to load ONNX Runtime dylib")`），不是返回错误。显式 `ort::init_from(path)` 才能把「没有库」变成一条能写进文档 `last_error` 的普通错误。
  - 查找顺序：`ORT_DYLIB_PATH` → 可执行文件所在目录 → 其 `onnxruntime/` 子目录 → 上 3 层父目录的同样两处（cargo 构建时 exe 在 `target/debug[/deps]`，脚本把库放 `target/onnxruntime`）。
- 库用**微软官方 1.28.3**（`scripts/onnxruntime-pin.txt` 按版本 + sha256 固定）。官方 Linux 库只要 `GLIBC_2.27` / `GLIBCXX_3.4.21`，在发版基线之内（已用 `objdump -T` 实测）。版本选 1.28 是因为 `ort 2.0.0-rc.13` 的预编译包就是 1.28.0；`ort` 要求库的次版本 ≥ 它编译的 API 级别（24）。
- `scripts/fetch-onnxruntime.sh`（Unix）/ `.ps1`（Windows）下载、校验 sha256、把库以规范名放进目标目录：`libonnxruntime.so` / `libonnxruntime.dylib` / `onnxruntime.dll`（官方包里是带版本号的符号链接链，脚本用 `cp -L` 取真实文件）。没有固定版本的目标（x86_64-apple-darwin）**不报错、不写文件**。
- `release.yml` / `build-manual.yml` 打包时把库放进压缩包；Windows 额外带上 `vcruntime140.dll`、`vcruntime140_1.dll`、`msvcp140.dll`（微软允许的 app-local 部署）——`dreamcore.exe` 是静态 CRT，没装 VC++ 运行库的机器上程序能启动，但 `onnxruntime.dll` 会加载失败，只会让本地嵌入不可用。
- `Dockerfile` 在构建阶段下载同一份库，拷到 `/app/` 与二进制并列。
- 桌面端 `prepare-dreamcore.js` 把压缩包里二进制旁边的运行时库一并拷进 `resources/bundled-dreamcore/<平台>/`（旧版本固定值的包里没有库，只警告，不失败）。

## 平台覆盖

| 平台 | 本地嵌入 |
| --- | --- |
| Windows x64 / Linux x64 / Linux arm64 / macOS 苹果芯片 | ✅ 库随包 |
| **macOS 英特尔** | ❌ 微软和 pyke 在这个版本都没有库。程序正常运行，只是默认的本地嵌入会在文档上报「Intel Mac 不支持本地嵌入，请配置远程嵌入端点」；配了远程端点（OpenAI 兼容）照常可用 |

## 给后来者

1. **别把 `ort` 改回 `download-binaries`**——那就是本次的失败。升级 onnxruntime：改 `onnxruntime-pin.txt` 的 VERSION、BASE_URL 和四个 sha256（`gh api repos/microsoft/onnxruntime/releases/tags/vX.Y.Z --jq '.assets[]|"\(.name) \(.digest)"'`），次版本不得低于 `ort` 的 API 级别。
2. 开发机跑真模型的测试：先 `scripts/fetch-onnxruntime.ps1 -Target x86_64-pc-windows-msvc -DestDir target/onnxruntime`（Linux/mac 用 `.sh`），然后正常 `cargo test`，不用设环境变量。
3. 发版产物里**一定要有库**才算数：看压缩包内容（工作流里 `tar -tzvf` 会打印）。旧的 v0.1.87 及以前没有这个库，也不需要（那时 ort 是静态链接的）。
4. 失败的发版会留下**没有产物的 GitHub 空发布**（`v0.1.88` 就是），dream-ui 的 `dreamcoreVersion` 不要指向它。

## 验证记录（2026-10-17）

| 项 | 结果 |
| --- | --- |
| Windows 本机，真实模型 + 动态加载 `onnxruntime.dll`（`local_embedding_fresh_install --ignored`，库放 `target/onnxruntime` 自动查找） | ✅ 26s，语义排序正确 |
| Linux 容器（`rust:1.95-trixie`），同一测试，官方 Linux 库 | ✅ 21s |
| 企业镜像 `docker build`（`--locked`，含下载脚本与 COPY） | ✅，`/app/libonnxruntime.so` 在位，`ldd` 无缺依赖，`dreamcore` 不再链接 onnxruntime |
| GitHub Manual Build 全平台（Linux x64/arm64、macOS x64/arm64、Windows x64/arm64） | ✅ 6/6 通过，含 glibc 基线检查 |
| 产物内容 | Linux x64/arm64 = 二进制 + `libonnxruntime.so`；macOS arm64 = 二进制 + `libonnxruntime.dylib`；macOS x64 = 仅二进制；Windows x64 = exe + `onnxruntime.dll` + 3 个 VC++ 运行库；Windows arm64 = 仅 exe |
| **CI 产物（静态 CRT 的 Windows 版）在全新个人模式客户端里端到端** | ✅ 注册文档 → 处理（本地模型，2.3s）→ 检索命中；库缺失时处理返回明确错误、服务不崩 |

**没有实际运行验证的**：macOS（苹果芯片）与 Linux arm64 只验证了「能构建、库在包里」，没有在真机/真容器里跑过嵌入；它们的加载逻辑与已验证的两个平台是同一份代码，差别只在库文件本身。
