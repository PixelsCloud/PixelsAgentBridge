# 统一构建与版本号

需要 Python 3.11+、项目固定版本的 Rust，以及对应目标所需的 Node/npm、NSIS 或 Docker。先在需要构建的 `apps/desktop`、`apps/web` 目录执行 `npm ci`。

macOS PKG 打包需要 Python 3.12+。Mac 的便捷入口会选取可用的 Python 3.12+ 并补齐 Homebrew 工具路径，避免使用系统 Python 3.9。

## 版本规则

`build-version.json` 是版本来源，随代码提交。首次整体编译使用 `1.2.0`，随后每次构建 patch 加1；`1.2.99 → 1.3.0`，`1.99.99 → 2.0.0`。minor、patch 范围均为0–99。

统一入口在启动子组件前分配一次版本，并同步自有 Rust crate、两个 Cargo.lock、前端 package/package-lock、Tauri 配置。第三方依赖及协议 schema 版本不变。一个命令内的所有目标使用同一版本，打包不再次递增。构建开始后失败或中断也占用该版本，重新构建递增；同一工作目录禁止两个整体构建并发，以免编译中途版本改变。锁随进程退出自动释放。

版本文件、同步后的清单和锁文件应一起提交。此计数在当前 Git 工作目录中分配，不是跨克隆的中央发号服务；正式构建使用同一个发布工作目录。

## 常用命令（仓库根目录）

```powershell
# Windows Desktop、Executor、MCP；同时生成 ZIP 和原名 EXE 安装包
python scripts/build.py desktop --package

# Server、Relay 和 Web；生成当前宿主平台归档
python scripts/build.py server --package

# 一次整体编译，全部组件共用一个版本号
python scripts/build.py desktop server --package

# 只编译前端，每次命令同样递增一次
python scripts/build.py web
python scripts/build.py desktop-web

# Windows 宿主通过 Docker 构建 Linux 客户端
python scripts/build.py linux --package

# 在 Mac 上一次构建 ARM 和 Intel，两种架构共用一次版本递增
python3.13 scripts/build.py macos --macos-arch all --package
# 或使用自动选择 Python/Homebrew 路径的便捷入口
bash packaging/desktop/build-macos.sh debug all

# 正式 Docker 镜像，显式使用 release；默认标签为产品版本号
python scripts/build.py docker --profile release
```

默认使用 debug；只有明确指定 `--profile release` 才使用 release。Docker 服务端镜像固定使用 release，因此该目标要求明确指定。`--image` 可指定 Docker 标签。不要用 `docker compose build` 代替统一入口；构建后将 Compose 的镜像变量指向生成的标签，再使用 `up -d --no-build`。

`apps/desktop` 和 `apps/web` 的 `npm run build` 已接入各自的前端构建入口；`apps/desktop` 的 `npm run build:desktop` 编译完整 Windows 客户端。Tauri 内部调用 `build:assets`，避免再次递增。

macOS 的 `--macos-arch` 支持 `native`（默认）、`aarch64`/`arm64`、`x86_64`、`all`。构建前检查宿主、Python、Tauri 和 Rust target，缺少前置条件不分配版本。完整编译开始后失败仍占用已分配版本；双架构编译过程中不再次分配。`--package` 同时生成 tar.gz 和 PKG，分别保持原有架构文件名。便捷脚本默认 debug；Release 需明确指定。

`cargo build`、直接调用 Tauri/Docker、`npm run build:assets` 是底层组件命令，只使用当前已同步版本，不负责分配产品版本。开发热更新、`cargo check/test/clippy/fmt`、前端类型检查和测试不递增。正式产物必须使用上面的统一入口。

## 打包与版本校验

macOS 免费签名：首次以构建用户运行 `python3.13 scripts/macos_signing.py init`，
在 `~/Library/Application Support/PixelsAgentBridgeBuildSigning` 创建固定自签名证书和
独立钥匙串。私钥、钥匙串密码只留在该用户的受限目录，不纳入 Git、不随安装包分发。
不要删除该目录来解决构建问题，应安全备份；更换证书会改变应用授权身份。
脚本不添加系统信任根、不修改 TCC/Gatekeeper。此签名不代表 Apple 公证，其他 Mac
的首次运行仍可能被 Gatekeeper 或管理策略阻止。

统一构建入口在分配版本之前检查签名材料，缺失/损坏时直接失败，不自动生成新证书。
Tauri 的临时 App 随后使用固定证书重签；App、Executor、MCP 的 designated requirement
分别绑定固定程序 ID 和证书指纹，不绑定每次变化的二进制哈希。签名完成后才记录
产物哈希及归档。PKG 构建再次核验三者的签名身份，拒绝临时签名或另一张证书的产物。
安装器 PKG 本身保持未签名，App 的固定自签名身份与 PKG 签名是两件事。

`PAB_TEST_LOCAL_SIGNING=1 python3.13 scripts/test_macos_signing.py` 在 Mac 上使用临时
程序验证两次不同编译仍匹配同一要求、错误证书和文件篡改被拒绝，不更改隐私权限。
这只能验证代码身份；真实权限保持必须在用户授权后，覆盖升级并实际截图/输入验收。

`.build/builds/` 记录成功构建的版本与产物 SHA-256。现有 `packaging/*/build*.py` 仍可单独重打包已构建产物，保持该产物的版本；无构建记录、组件被替换或混入另一批前端时拒绝打包，不给旧二进制贴新版本。

Mac 使用每个架构独立的 `macos-aarch64-<profile>.json` / `macos-x86_64-<profile>.json` 记录，覆盖 Executor、MCP 和完整 App 文件。归档与 PKG 的版本必须与 App 一致；旧的无构建记录产物需要重新构建。

Windows 安装包文件名保持 `pixels-agent-bridge-windows-x86_64-debug-setup.exe` 等原有名字。版本写入 EXE 文件属性、Windows 已安装应用信息及 SHA-256 清单；Desktop 关于、MCP 握手、设备和 Relay 版本上报、Web 服务版本使用对应编译版本。构建命令不执行安装或部署。

## 验证

```powershell
python -m unittest discover -s scripts -p test_build_version.py -v
```

覆盖首次编译、连续递增、两级进位、非法版本、清单/锁文件同步、第三方版本不变、失败重试、跨进程互斥、多目标只分配一次，以及打包版本和产物哈希匹配。测试在临时目录运行，不消耗正式版本号。

2026-10-04 验证：9项自动化测试通过；`python scripts/build.py desktop server --package` 完成首次 `1.2.0` 实际构建，版本计数为1。Windows 客户端 ZIP/NSIS、Server/Relay/Web 归档均生成成功，Desktop 与安装器的 ProductVersion/FileVersion 均为 `1.2.0`。17个自有 Rust 包版本一致，两份 Cargo.lock 的第三方条目未变。构建日志 `.build/versioned-build.log`。本轮未执行 Linux/Docker 镜像实际构建。

2026-10-04 合并后验证：快进同步远端 `1691883`，该提交已合并 macOS 原生支持、双架构安装工具与上述版本管理功能，无需再次处理文本冲突。在 Windows 上通过9项版本测试、1项包含 Windows/Linux/macOS 归档的测试、3项 macOS 安装器输入校验测试，以及 Desktop `tsc --noEmit` 和 `cargo check --locked --manifest-path apps/desktop/src-tauri/Cargo.toml`。Rust 检查日志 `.build/merge-macos-windows-check.log`。未执行 macOS 原生编译、PKG 构建或安装；macOS 独立构建脚本尚未接入统一版本递增，归档沿用已编译 App 的版本。此次检查不消耗产品版本，仍为 `1.2.0`、计数1。

2026-10-04 macOS 入口已接入：通过 Pixels MCP 在设备603527578的 `/Users/huayang/source/PixelsAgentBridge`，以仓库用户 huayang 运行 `bash packaging/desktop/build-macos.sh debug all`。本轮从 `1.2.0` 计数1递增到 `1.2.1` 计数2，ARM/Intel 均成功编译并生成 tar.gz、PKG。两套安装包均通过展开、架构、App 签名完整性、内嵌版本和SHA-256验证；每种架构的5项PKG测试全部通过。另通过13项版本测试和三平台归档回归。重新打包测试后版本计数仍为2。本机版本清单已同步此远端构建结果，不再次分配版本。远端构建日志 `.build/macos-versioned-build.log`。本轮仅构建和展开验证，没有安装、更新运行中的服务，也未在 Intel 实机执行程序。
