# Draw.io Desktop 安装与验证指南

仅在用户明确要求安装 Draw.io Desktop，或导出任务确实需要本地 CLI 且未检测到可执行文件时使用本指南。

## 安全原则

1. 先检测，后安装；已安装时不得重复安装。
2. 安装第三方软件前，向用户说明软件来源、安装方式、可能产生的系统变更，并取得明确确认。
3. 仅使用可信来源：
   - 官方项目发布页：https://github.com/jgraph/drawio-desktop/releases
   - macOS Homebrew 官方 Cask：https://formulae.brew.sh/cask/drawio
   - Windows Package Manager 中的 `JGraph.Draw`
   - Linux Snap Store 中的 `drawio`
4. 不静默接受许可协议，不绕过系统安全机制，不关闭 Gatekeeper、SmartScreen、杀毒软件或软件签名校验。
5. 不以 `sudo`、管理员权限或机器级安装作为默认方案。需要提权时，先解释原因并再次确认。
6. 不拼接不可信参数，不执行来源不明的脚本，不使用第三方下载站。
7. 安装失败时停止并报告，不循环重试，不擅自改用不可信镜像。

## 1. 检测是否已安装

### macOS

按顺序检测：

```bash
command -v drawio
```

```bash
test -x "/Applications/draw.io.app/Contents/MacOS/draw.io"
```

若命令存在，运行以下命令确认 CLI 可启动：

```bash
drawio --help
```

或：

```bash
"/Applications/draw.io.app/Contents/MacOS/draw.io" --help
```

### Windows

按顺序检测：

```powershell
Get-Command drawio -ErrorAction SilentlyContinue
```

```powershell
Test-Path "C:\Program Files\draw.io\draw.io.exe"
```

### Linux

按顺序检测：

```bash
command -v drawio
```

```bash
command -v draw.io
```

```bash
snap list drawio
```

## 2. 取得用户确认

在执行安装命令前，明确告知：

- 将安装的软件：Draw.io Desktop；
- 来源和具体安装命令；
- 软件会写入的典型位置；
- 是否可能要求管理员权限或系统确认；
- 安装完成后将执行 CLI 可用性验证。

只有用户明确同意后才执行安装。若宿主环境会自动弹出权限确认，仍需先完成上述说明。

## 3. 安装方式

### macOS（推荐）

先检测 Homebrew：

```bash
command -v brew
```

若 Homebrew 已存在，经用户确认后执行：

```bash
brew install --cask drawio
```

典型应用路径：

```text
/Applications/draw.io.app
```

Homebrew 不存在时，不自动安装 Homebrew。改为向用户提供官方发布页，让用户选择是否手动下载安装：

https://github.com/jgraph/drawio-desktop/releases

### Windows（推荐）

先检测 WinGet：

```powershell
winget --version
```

经用户确认后执行：

```powershell
winget install --id JGraph.Draw --exact --source winget
```

若 WinGet 不可用，提供官方发布页，不自行下载并运行 `.exe` 或 `.msi`：

https://github.com/jgraph/drawio-desktop/releases

### Linux（优先 Snap）

先检测 Snap：

```bash
command -v snap
```

经用户确认后执行：

```bash
sudo snap install drawio
```

该命令通常需要管理员权限，必须在执行前单独说明并确认。若 Snap 不可用，提供官方 Releases 页面中的 AppImage、DEB 或 RPM，由用户选择与系统匹配的安装包；不要猜测发行版或包架构。

## 4. 安装后验证

### CLI 启动验证

运行可执行文件的帮助命令，确认退出状态正常：

```bash
drawio --help
```

macOS 可使用绝对路径：

```bash
"/Applications/draw.io.app/Contents/MacOS/draw.io" --help
```

### 最小导出验证

仅在有可用测试 `.drawio` 文件时执行：

```bash
drawio -x -f png -e -b 10 -o <output.drawio.png> <input.drawio>
```

验证输出文件存在且非空。测试文件必须写入当前任务的工作目录或临时目录，不得覆盖用户已有文件。

## 5. 失败处理

- 包管理器不存在：提供官方 Releases 地址并停止自动安装。
- 权限不足：说明所需权限，由用户决定是否继续；不得绕过权限。
- 系统版本或 CPU 架构不兼容：报告检测结果并提供匹配版本选择，不得强制安装。
- 网络、签名或校验失败：停止安装，保留原系统状态，不使用非官方镜像降级。
