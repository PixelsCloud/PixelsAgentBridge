# 统一构建与版本号

需要 Python 3.11+、项目固定版本的 Rust，以及对应目标所需的 Node/npm、NSIS 或 Docker。先在需要构建的 `apps/desktop`、`apps/web` 目录执行 `npm ci`。

远程桌面内置的软件 H.264 编解码由源码编译。Windows x86/x64、macOS Intel
及 Linux x86/x64 构建还需要 NASM；启动 Cargo 前确认 `nasm -v` 可用。
Linux 构建镜像已经包含它，macOS 仅构建 ARM 不需要 NASM。曾经在缺少 NASM
的环境中编译过 `openh264-sys2` 时，应在自己的构建缓存中重新编译该依赖，
避免复用禁用了 SIMD 的旧产物。NASM 和编译器不随安装包交付，客户无需安装。

macOS PKG 打包需要 Python 3.12+。Mac 的便捷入口会选取可用的 Python 3.12+ 并补齐 Homebrew 工具路径，避免使用系统 Python 3.9。

## 版本规则

客户端保留 `build-version.json` 构建记录及官网的正式版本预留机制。管理平台和 Relay
分别使用 `product-versions.toml` 中的 `management`、`relay` 计数；旧组合入口使用独立的
`legacy_server` 兼容计数。`web` 属于管理平台，`desktop-web` 属于客户端，Docker 产品
目标与对应原生目标共享产品计数。首次服务构建从已登记的 `1.3.6` 开始，此后仅推进
所构建产品的 patch；`1.2.99 → 1.3.0`，`1.99.99 → 2.0.0`，minor、patch 均为 0–99。

统一入口在启动子组件前为每个涉及的产品各分配一次发布版本。**不修改任何 Rust crate、Cargo.lock、npm package/lock 或 Tauri 配置中的内部版本号**。同一产品的多个目标共用一次预留；不同产品允许版本不同，打包不再次递增，失败或中断也占用版本。同一工作目录的并发构建由系统锁阻止，锁随进程退出释放。

版本记录应随代码提交。服务计数在当前 Git 工作目录中分配，不是跨克隆的中央发号服务；同一发布流使用固定工作目录。客户端正式发版继续使用官网预留版本。

## 常用命令（仓库根目录）

```powershell
# Windows Desktop、Executor、MCP；同时生成 ZIP 和带版本号的 EXE 安装包
python scripts/build.py desktop --package

# 管理平台：只构建 Server 与 Web，生成独立归档
python scripts/build.py management --package

# Relay：只构建中继与健康探针，不需要 Node/npm 或 Web 产物
python scripts/build.py relay --package

# 兼容旧流程：Server、Relay、转发健康探针与 Web 的组合归档
python scripts/build.py server --package

# 一次构建多个目标；各产品分别预留自己的版本
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
python scripts/build.py docker-management --profile release
python scripts/build.py docker-relay --profile release
# 兼容旧流程的组合镜像
python scripts/build.py docker --profile release
```

管理平台归档包含 Server、Web、配置样例与原生 Windows/systemd 安装助手；Relay
归档包含中继、健康探针、原生/容器部署助手、`roll_relays.py` 与
[Relay 集群运维指南](docs/RELAY_CLUSTER_OPERATIONS.md)。两种归档分别校验自己的构建记录，
文件名包含产品、平台、架构、profile 和发布版本。旧 `server` 组合归档保留原名称。
探针程序 `pab-relay-healthcheck` 运行在升级操控机器上；混合部署需要分别构建目标平台的包，
不要把 Windows EXE 当作 Linux 程序分发。

管理平台、Relay 和客户端继续共享本仓库的协议与基础 crates。三个产品分别分配版本，
构建输出复用主仓库缓存，不依赖其他产品的安装包。服务发布使用独立的
[发布入口与签名域](docs/RELEASE.md)，最新版整套联调由 `smoke/cases.toml` 的 `INT-01` 验收。
Docker 产品目标添加 `--package` 时，按不可变镜像 ID 导出 tar 与发布清单，供同一服务发布入口验收。

默认使用 debug；只有明确指定 `--profile release` 才使用 release。Docker 服务端镜像固定使用 release，因此该目标要求明确指定。`--image` 可指定 Docker 标签。不要用 `docker compose build` 代替统一入口；构建后将 Compose 的镜像变量指向生成的标签，再使用 `up -d --no-build`。

`apps/desktop` 和 `apps/web` 的 `npm run build` 已接入各自的前端构建入口；`apps/desktop` 的 `npm run build:desktop` 编译完整 Windows 客户端。Tauri 内部调用 `build:assets`，避免再次递增。

macOS 的 `--macos-arch` 支持 `native`（默认）、`aarch64`/`arm64`、`x86_64`、`all`。构建前检查宿主、Python、Tauri 和 Rust target，缺少前置条件不分配版本。完整编译开始后失败仍占用已分配版本；双架构编译过程中不再次分配。`--package` 同时生成 tar.gz 和 PKG，PKG 文件名包含架构、profile 和版本号。便捷脚本默认 debug；Release 需明确指定。

`cargo build`、直接调用 Tauri/Docker、`npm run build:assets` 是底层组件命令，只使用当前固定的内部版本，不负责分配产品版本。开发热更新、`cargo check/test/clippy/fmt`、前端类型检查和测试不递增。正式产物必须使用上面的统一入口。

## 增量构建

沿用原命令即可，不执行 `cargo clean`，也不再维护组件版本指纹。

本机开发、编译和发版只在主仓库 `D:/GoCloud/PixelsAgentBridge` 中进行，不新建 Git worktree，也不复制源码到 `.build` 或其他目录。其他会话正在修改构建输入或占用编译预算时等待。Windows Desktop 与后台默认共用主仓库 `target`，Linux GUI 将主仓库直接挂载到构建容器；版本目录仅存放产物。Desktop Release 关闭 LTO、开启增量编译并使用优化等级 2，详见 [Windows 构建缓存说明](packaging/desktop/WINDOWS_RELEASE_BUILD.md)。

- 只递增安装包版本：内部清单和锁文件完全不变，Cargo 复用全部可用缓存。
- 只改 UI：重编前端和必要的 Desktop 资源；Executor、MCP 复用缓存。
- 修改 Rust 源码、共享依赖、编译器或选项：Cargo 正常重编受影响部分，内部版本仍固定。
- Windows/macOS 使用相同规则，Linux 无界面版继续通过现有 Docker 入口构建。

Linux 无界面版以 Debian 11（glibc 2.31）及其固定软件源快照构建，兼容 Ubuntu 20.04 及以上。构建缓存使用 `.build/linux-target-bullseye`，与此前 Debian 12 的缓存分开，避免复用引用新版 glibc 的本地库。Linux 后台和 Desktop 构建默认最多 10 个 Cargo 作业，也可通过 `CARGO_BUILD_JOBS` 调低以共享机器预算。容器内取消 `CMAKE_BUILD_PARALLEL_LEVEL` 覆盖，由 cmake crate 把 Cargo jobserver 交给 GNU Make，原生依赖与 Rust 共享同一个作业池；不另外增加 CMake 作业，也不强制把 OCR 原生构建串行化。每台机器所有会话合计仍不得超过 10 个构建作业。Windows/macOS/Android 的嵌套构建限制保持不变。

安装包发布版本与程序内部版本独立。构建记录同时记录发布版本、组件版本和产物 SHA-256；打包仍逐项核验哈希。内部关于页、MCP 握手、设备与 Relay 上报继续显示各组件自己的版本，不伪装成安装包版本。

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

Mac 使用每个架构独立的 `macos-aarch64-<profile>.json` / `macos-x86_64-<profile>.json` 记录，覆盖 Executor、MCP 和完整 App 文件。归档与 PKG 使用构建记录中的发布版本，App 的内部版本保持固定；完整 App（含 Info.plist）仍由哈希绑定，旧的无构建记录产物需要重新构建。

每次生成的 Windows EXE、macOS PKG 安装包文件名都必须包含构建版本，格式为 `pixels-agent-bridge-<platform>-<arch>-<profile>-<version>-setup.<exe|pkg>`，例如 `pixels-agent-bridge-windows-x86_64-release-1.2.48-setup.exe`。版本取自已核验的构建记录，不取 App 或源码的内部版本；重新打包不额外递增。版本同时写入 EXE 文件属性、Windows 已安装应用信息及 SHA-256 清单；Desktop 关于、MCP 握手、设备和 Relay 版本上报、Web 服务版本使用对应编译版本。构建命令不执行安装或部署。

## 验证

```powershell
python -m unittest discover -s scripts -p 'test_build*.py' -v
```

覆盖首次编译、连续递增、两级进位、非法版本、所有内部清单/锁文件逐字节不变、失败重试、跨进程互斥、多目标只分配一次，以及打包版本和产物哈希匹配。测试在临时目录运行，不消耗正式版本号。


## Linux 图形桌面独立构建

Ubuntu 22.04 x86_64 GUI 使用独立入口，不改变已发布的 glibc 2.31 无界面包：

```powershell
python scripts/build_linux_desktop.py --version 1.0.0 --package --headless-package PATH_TO_VERIFIED_TAR --headless-sha256 VERIFIED_SHA256
```

该入口生成原生 Tauri GUI 和 deb，复用经过哈希验证的稳定后台、MCP、文档 Worker 与离线资源；不递增 `build-version.json`。GUI 构建记录绑定版本、架构和 ELF 哈希，缺失记录、篡改或错误版本拒绝打包。Cargo 8 + CMake 2，总编译并发最多 10，Docker CPU 限制为 10。具体方法见 [桌面包说明](packaging/desktop/linux-desktop/README.md)。使用 `python scripts/release.py linux-desktop --headless-package PATH --headless-sha256 SHA256 --notes-zh "说明" --notes-en "Notes"` 可自动预留版本、构建、核验、签名、上传和发布；私有凭据沿用已有发布环境变量。

## Development

Use the pinned Rust toolchain and checked-in Cargo/npm lockfiles. Build prerequisites,
platform packaging and version allocation are described in [BUILDING.md](BUILDING.md).
Keep the combined Cargo, CMake, Gradle and nested build concurrency at or below
10 jobs per machine. Ordinary checks do not allocate release versions.

### Repository layout

- `crates/`: shared protocol, transport, client, executor, server and Relay code.
- `apps/`: Desktop, Web, Android and agent integrations.
- `packaging/` and `scripts/`: reproducible builds, deployment templates and checks.
- `vendor/` and `patches/`: pinned third-party code, local patches and licenses.
- `acceptance/fixtures/`: deterministic test inputs; `smoke/tool-coverage.json`
  is the MCP test contract. Keep these inputs in Git.
- `experiments/remote-desktop/reliable_media.rs` and `media_child.rs`: source inputs
  of the registered `reliable-media` Cargo example; keep both in Git.

### Checks

Release candidates use the maintained [smoke test entry point](smoke/README.md).
Definitions and reusable orchestration live in `smoke/`; private environments and
per-run evidence stay in its ignored `local/` and `runs/` directories.

Run from the repository root, selecting the packages affected by a change:

```powershell
cargo fmt --all -- --check
python scripts/verify_iroh_vendor.py
cargo test -p <package> -j 8
cargo clippy -p <package> --all-targets -j 8 -- -D warnings
```

Frontend commands live in each app's `package.json`. Install dependencies with
`npm ci` from that app before running its tests. For Desktop page work, run
`npm run dev:desktop` in `apps/desktop`; restart after Rust changes. A browser
opening the Vite URL does not provide Tauri's native APIs.

Integration tests must use isolated databases, test accounts and temporary
directories. Never point test configuration at a production database. Hardware
and installed-package checks require their actual target platform; a source
check is not a substitute for those checks.

### Files that stay local

Keep task plans, progress histories, one-off experiments, screenshots, run reports,
machine instructions (`AGENTS.md`) and private credentials on the development
machine. The ignore rules retain these files locally while excluding them from
future commits. Use `.build/` for generated reports and build artifacts;
`acceptance/` for local run evidence, apart from the tracked fixtures and contract;
`design/` for mockups, apart from reusable `design/brand/` assets.

Commit source, reusable automated tests, lockfiles, configuration examples,
licenses, build scripts and maintained usage/deployment documentation. Do not
force-add ignored run evidence. Removing a tracked file with `git rm --cached`
keeps its local contents; it does not erase that file from earlier commits.

Project-owned application settings use TOML. Secrets and live deployment settings
belong in ignored local storage, not example files. Preserve user configuration,
credentials and device identities during upgrades. See [configuration](docs/CONFIGURATION.md),
[deployment](docs/WEB_DEPLOYMENT.md) and [release instructions](docs/RELEASE.md).
