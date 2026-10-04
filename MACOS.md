# macOS 完整适配与验收

2026-10-04 构建更新：macOS 已接入统一版本递增。`bash packaging/desktop/build-macos.sh debug all` 在 Apple Silicon 实机完整生成 ARM/Intel 两套 `1.2.1` Debug tar.gz 和 PKG，同批只递增一次。每种架构的5项PKG测试（含实际重新打包和展开）全部通过，版本计数保持2；13项版本测试、三平台归档回归也通过。安装包仍为未签名、未公证的开发产物，本轮未执行安装或 Intel 实机运行。详情见 [BUILDING.md](BUILDING.md)。

## 目标与边界

覆盖现有 60 个 MCP 工具、Desktop、Executor、后台运行和安装卸载。
Windows/Linux 既有原生后端和安装脚本保持不变。macOS 新增代码通过
目标平台条件编译隔离；共用文件只增加必要的平台分派、分发入口和 Retina
截图元数据校验兼容，不改变原有 Windows/Linux 截图负载。

用户授权后可实现的能力纳入实现；系统保护禁止、应用不提供原生接口或无法
安全保持目标身份的操作明确失败，不伪造成功，不关闭 SIP 或绕过 TCC。
缺少硬件、服务器、签名证书或用户授权导致的未验证项单独记录，不能算已验收。

## 实施计划

1. 平台隔离：撤出此前共用 Unix 脚本中的未提交 macOS 改动，独立实现安装包。
2. 原生构建：检查 workspace、Tauri、前端；Apple Silicon 原生构建，Intel 记录检查范围。
3. 安装运行：原生 app、launchd、用户 helper、本地访问凭据、Finder 配置、升级卸载。
4. 基础功能：设备、命令、文件、终端、任务查询、取消、日志与持久化。
5. 系统功能：系统/磁盘/网络/DNS/进程；macOS 登录会话、进程身份、launchd 管理。
6. 桌面功能：权限检测、屏幕和窗口截图、显示器、窗口控制、Unicode、键鼠、批次。
7. 集成：Git、Docker、本地 PATH、凭据与 socket、MCP 注册和桌面状态提示。
8. 验收交付：测试、构建、打包、跨平台差异审查、使用说明和受限清单。

## 验收原则

- 原生编译和前端构建通过；新后端有针对身份、解析、权限失败的测试。
- 复用仓库测试验证文件、终端、Git、Docker、QUIC 和 stdio；需外部服务的测试
  与可隔离执行的测试分开报告。
- 破坏性测试仅操作临时目录、测试子进程和临时服务。
- 不修改真实用户的系统服务或 Git 仓库来验证控制功能。
- 桌面测试不随意向当前用户窗口发送输入；使用明确的测试窗口或权限预检。
- 每个范围标注已验证、实现但待环境验证、系统限制三种状态之一。
- 收尾检查 Windows/Linux 后端及安装脚本无修改；共享逻辑的必要改动单独说明。

## 实现与验证结果（2026-10-03）

代码适配已完成；未具备权限或外部环境的验收不计为通过。此次在 Apple Silicon
macOS 上执行自动化测试，Intel 做编译检查和 Release 交叉打包，不代表 Intel 实机验收。

| 范围 | 实现 | 已验证与限制 |
|---|---|---|
| 原生构建 | 独立 macOS `.app`、ARM/Intel 条件编译 | 两种架构均通过核心、桌面端编译，并生成 Release tar 包 |
| 安装、升级、卸载 | 独立脚本、系统 LaunchDaemon、Aqua LaunchAgent、本地凭据 | Shell/plist 校验和三平台打包回归通过；未实际安装正式服务 |
| 设备、命令、任务、日志 | 复用跨平台 Executor/Bridge | ARM 自动化、隔离 QUIC 和 stdio 通过；真实部署连接待验收 |
| 文件、目录、归档 | 复用既有工具，识别系统 `/tmp`、`/var`、`/etc` 别名 | 134 项 Executor 测试通过；自建符号链接仍拒绝 |
| 交互终端 | 原生 PTY + `/bin/zsh -l -i` | PTY 输出、中文、分段读取等自动化通过 |
| 系统、磁盘、网络、DNS | 复用跨平台采集 | 平台采集与隔离网络测试通过 |
| 登录会话 | macOS utmpx、UID、控制台会话 | 原生只读查询通过；记录反映 utmpx 登录，不保证覆盖全部 GUI 进程 |
| 进程终止 | audit token 固定身份、内核身份校验发信号、kqueue 观察退出 | 错误身份不杀进程、只终止自建测试子进程通过 |
| 服务管理 | launchd 查询、启停、重启、启用、禁用 | 原生查询和临时 LaunchAgent 完整生命周期通过，已清理 |
| 显示器、截图 | xcap、Retina 逻辑坐标元数据、截图像素区域裁剪 | 显示器枚举、元数据映射测试通过；真实截图因权限跳过 |
| 窗口、Unicode、键鼠和输入批次 | 公共 Accessibility API + Enigo、保留窗口身份、输入状态清理 | 原生编译、权限预检、键码/几何测试通过；实际桌面操作因权限跳过 |
| Git | 复用原生 Git 和任务持久化 | 临时仓库、本地远端和隔离协议测试通过；真实 SSH/凭据待环境验证 |
| Docker | 复用 Bollard/本地 Unix socket | HTTP 替身和协议测试通过；当前无可用 Docker Engine，真实容器跳过 |
| 桌面设置、MCP | macOS 权限面板、Homebrew PATH、安装版 MCP 定位、Finder 配置 | 前端构建、桌面 Rust 测试和真实 MCP 子进程重连测试通过 |
| 签名、公证 | `.app` ad-hoc 签名 | 无 Developer ID/公证凭据；不是已公证的公开分发包 |

### 平台隔离与兼容性

- `packaging/desktop/unix/`、`packaging/desktop/windows/` 以及 Windows/Linux 原生
  后端未改动。新增打包测试同时验证 Windows ZIP、Linux tar 和 macOS app tar。
- Windows/Linux 下权限面板不显示；新增 Rust 后端、终端选择、系统路径别名处理和
  Homebrew 查找均通过平台条件隔离。没有在本机运行 Windows/Linux 程序，不能据此
  声称已完成两平台运行回归。
- Retina 截图复用已有可选 `desktop_rect` 字段表示逻辑桌面范围，允许显示器截图
  携带它。Windows/Linux 原有请求、响应、字段含义不变；**连接 Mac 时，请同时更新
  MCP、Executor 和 Desktop/helper**，旧版可能拒绝该元数据。
- 截图 `region` 仍以原始图像像素计，桌面点击坐标以逻辑点计；使用返回的
  `preview_to_desktop` 映射，不把 Retina 图片像素直接用作点击坐标。负原点、多屏
  映射已有纯数据测试，物理多屏未验收。
- 保留现有 60 个 MCP 工具接口，没有添加仅 Mac 可见的工具或改变服务名校验规则。

## 构建与使用

### 双击安装 `.pkg`（2026-10-04 更新）

已新增 `packaging/desktop/build_macos_pkg.py`，将校验过的 tar 包封装为原生 Installer
安装器；默认地址与 Windows NSIS 完全一致：

- Control：`wss://pab.rgaa.vip/control`
- Relay：`https://pab-relay.rgaa.vip`

已合并新版协议（`695aa1c`），部署 UUID 已完全移除。macOS 安装、PKG 打包参数、
预置配置和测试均同步删除该字段，不再需要用户提供 UUID。旧版 tar 包不能混用。

构建新版两种架构的 tar 包后，在仓库根目录执行（此打包器需要 Python 3.12+）：

```sh
python3.13 packaging/desktop/build_macos_pkg.py --arch aarch64
python3.13 packaging/desktop/build_macos_pkg.py --arch x86_64
```

生成 `pixels-agent-bridge-macos-{aarch64|x86_64}-release-setup.pkg` 和各自的 SHA-256
清单。双击、输入管理员密码即可安装，不需要在目标 Mac 执行终端命令或填写部署参数。
安装器会检查原生架构、当前登录用户及系统卷，复用现有安装逻辑，配置后台服务和
该用户的访问凭据。屏幕录制/辅助功能依然必须手动授权。请先退出旧 Desktop/MCP。

安装器使用 scripts-only 组件，receipt 不包含完整已安装文件清单，仍使用
`uninstall.sh` 卸载；安装失败无自动回滚。`--sign` 支持 Developer ID Installer
证书；当前没有该证书，不会把未签名、未公证的包描述为已公证发行版。

验证覆盖参数、Windows 地址一致性、Shell 转义、归档路径和校验和，并构建/展开临时
`.pkg` 检查内容和不含部署 UUID 的配置；测试包不执行安装，结束后清理。
ARM 和 Intel 分别执行上述测试，均为 5 项通过；解包后再次核验全部二进制架构和
app 签名完整性。Windows/Linux 安装脚本与 Windows NSIS 打包器均未改动。

本次新版源码打包回归：`pab-bridge`、`pab-executor`、`pab-protocol` 共 256 项
测试通过，2 项默认忽略；Desktop 使用新构建的 ARM Release MCP，8 项全部通过。
日志为 `.build/macos-package-regression-current.log` 和
`.build/macos-desktop-package-tests-current.log`。下文 2026-10-03 的全仓库记录保留作
适配历史，本次没有重新执行需要 PostgreSQL 或桌面权限的环境验收。

### 源码构建与 tar 安装

需要 Xcode Command Line Tools、仓库固定版本的 Rust、Node/npm、Python 3.12+。
本次开发环境补齐了 Homebrew rustup、Node、Python 3.13 和
Rust Intel target；不由安装包自动安装这些开发工具。

在 macOS 仓库根目录执行：

```sh
npm --prefix apps/desktop ci
bash packaging/desktop/build-macos.sh debug all
# 默认只编译当前架构的开发包：bash packaging/desktop/build-macos.sh
# 正式双架构：bash packaging/desktop/build-macos.sh release all
# Intel 交叉构建：bash packaging/desktop/build-macos.sh release x86_64
# Apple Silicon：bash packaging/desktop/build-macos.sh release aarch64
```

脚本调用统一构建入口，在编译前递增一次版本，构建 Executor、MCP、前端和原生 app，
再打包 tar.gz 和 PKG 到 `.build/packages/`。`all` 的 ARM/Intel 共用该版本，打包不再次递增。
直接入口为 `python3.13 scripts/build.py macos --macos-arch all --package`；使用便捷脚本
可以自动选择 Python 3.12+ 和 Homebrew 工具路径。以仓库所属用户执行，不以 root 编译。
ARM 包名为 `pixels-agent-bridge-macos-aarch64-release.tar.gz`；指定 `x86_64` 可在
Mac 上交叉构建 Intel 包（需安装相应 Rust target）。相邻 `SHA256.json` 记录
Release 校验和，Debug 使用独立清单。构建产物使用目标三元组子目录以隔离架构。
配置的最低系统版本是 macOS 12，但旧系统运行兼容性尚未实测。

解压完整包，以实际桌面用户执行（替换两个服务器地址，不要直接以 root 登录安装）：

```sh
sudo bash install.sh wss://CONTROL_ENDPOINT https://RELAY_ENDPOINT
```

安装前退出旧 Desktop 及使用 MCP 的客户端。升级使用新完整包重新执行同一命令；
脚本停止 PAB helper/Executor 后替换文件，保留数据。不提供安装失败的事务回滚，
升级前可备份下列数据目录。不要混合新旧二进制。

| 内容 | 安装路径 |
|---|---|
| GUI + 用户 helper | `/Applications/Pixels Agent Bridge.app` |
| MCP、Executor、配置、脚本 | `/Library/Application Support/PixelsAgentBridge` |
| 机器身份、任务、日志 | `/Library/Application Support/PixelsAgentBridgeData` |
| 当前用户 Bridge、设置、本地凭据 | `~/Library/Application Support/PixelsAgentBridge` |
| 机器后台服务 | `/Library/LaunchDaemons/com.pixelsagentbridge.executor.plist` |
| 登录后的桌面 helper | `/Library/LaunchAgents/com.pixelsagentbridge.session-helper.plist` |

Finder 可直接打开 app；无需从终端注入部署变量。已有用户设置优先于机器级配置。
MCP stdio 命令为 `/Library/Application Support/PixelsAgentBridge/run-mcp.sh`，将整个
路径作为客户端配置中的一个 command 字符串。设置页中的 AI Agent 集成会查找
Homebrew Codex CLI；未安装 Codex CLI 时可手动注册这个 stdio 入口。

卸载（先退出 Desktop/MCP 客户端）：

```sh
sudo bash '/Library/Application Support/PixelsAgentBridge/uninstall.sh'
```

卸载只移除 app、PAB 程序和两个 launchd 作业，保留机器/用户数据，便于恢复安装。
其他用户不自动获得本地敏感信息访问；需要另行以管理员身份签发其 `local-access.key`。

### 权限和运行账户

- 屏幕录制和辅助功能需用户在系统设置中授予 **Pixels Agent Bridge**。设置页可
  查看状态、打开对应面板；未授权明确报错，不弹出循环请求、不绕过 TCC/SIP。
  屏幕权限变更后重启 app，并重新登录以重启后台 helper。
- 可无交互检查：`'/Applications/Pixels Agent Bridge.app/Contents/MacOS/pab-desktop' --macos-check`。
  本次结果是 ARM、活动控制台、1 个显示器，屏幕录制/辅助功能均 `false`，本地访问
  凭据未配置。按用户允许跳过权限受限项的要求，没有发送真实键鼠事件或申请授权。
- 图形操作只针对当前活动控制台用户；不支持登录前桌面、FileVault 解锁、绕过锁屏
  或 Windows 安全注意序列。受保护窗口、无法唯一匹配 AX 的窗口、应用不支持的
  操作会失败。最大化是调整窗口几何而非进入全屏 Space；应用限制几何时返回未确认。
- Executor 是机器级 root 服务，桌面捕获/输入由用户 helper 完成。Git/SSH 凭据、
  工作目录权限、Docker socket 属于 **Executor 的运行身份**，不能假设继承桌面
  用户的 SSH agent、Docker CLI context 或用户 PATH。
- Docker 需已有本地 Engine，支持本地 `DOCKER_HOST=unix:///绝对路径`；必要时由
  管理员在机器 `settings.env` 配置，并重启 Executor。不要放宽 socket 权限来绕过
  访问控制。本次没有安装 Docker、修改个人 Git 凭据或实际部署 PAB 系统服务。

### macOS 服务管理约定

使用 `system:标签`、`gui:UID:标签`、`user:UID:标签`，例如
`gui:501:com.example.worker`。列表列出 Executor 所在 bootstrap 域；显式名称可查询
其他域，仍受系统权限约束。启停需在标准 LaunchAgents/LaunchDaemons 目录找到同名
且 Label 一致的 plist。禁止控制 `com.apple.*` 和 `com.pixelsagentbridge.*`。

Stop 使用 bootout 卸载当前作业，Start 用 bootstrap/kickstart；Enable/Disable
只改变 launchd 禁用标志，不承诺立即启动/终止进程。Restart 用 kickstart -k 并观察
新 PID。launchd 可能按默认约 10 秒节流，建议服务控制超时设置 20 秒或更多；请求
被接受但状态未观察到时返回 `unconfirmed`，不能当成未发生或自动重试。

## 验收记录与复现

以下命令在仓库根目录执行；日志保存在本机 `.build/macos-*.log`（不纳入 Git）。

```sh
cargo test --workspace --exclude pab-server --lib --bins --tests --offline
cargo test --workspace --exclude pab-server --doc --offline
cargo test -p pab-server --lib --offline
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --lib --offline
cargo test -p pab-os-control temporary_launch_agent_lifecycle -- --ignored
PAB_MCP_SMOKE_EXE="$PWD/target/debug/pab-mcp" cargo test \
  --manifest-path apps/desktop/src-tauri/Cargo.toml \
  real_mcp_stdio_processes_register_before_tools_and_clean_up_on_exit -- --ignored
python3 packaging/desktop/test_packaging.py
cargo clippy -p pab-os-control -p pab-os-sessions -p pab-desktop-control --all-targets -- -D warnings
cargo check --workspace --all-targets --target x86_64-apple-darwin --locked
cargo check --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets --target x86_64-apple-darwin --locked
```

- 非 Server workspace：29 组测试，338 通过、4 默认忽略；doctest 单独重跑通过。
- Server 纯单元：6 通过；Desktop 最终使用 Release MCP 并启用 ignored 测试，
  8 项全部通过。临时 launchd 服务测试另外显式运行通过（约 10.9 秒）。
- 全仓库 Clippy（不加 `-D warnings`）通过；原有 `too_many_arguments` 等警告
  导致全仓库严格模式不通过。新增三个原生后端的严格 Clippy 通过。
- 完整 Server 数据库集成测试需 PostgreSQL `DATABASE_URL`，当前未配置；没有把
  缺少数据库导致的失败视为 macOS 回归，也未改动数据库服务。
- `scripts/verify_iroh_vendor.py` 仍失败：仓库既有 `vendor/iroh-relay/src/server.rs`
  和 `src/server/testing.rs` 与记录清单不一致。已核对相关文件工作树与 Git HEAD
  一致，本次未改 vendor/补丁清单，避免把无关跨平台修复混入本任务。

### 最新交付（2026-10-04，无部署 UUID）

产物位于 `.build/packages/`：

| 平台 | 双击安装包 | 大小 |
|---|---|---|
| Apple Silicon | `pixels-agent-bridge-macos-aarch64-release-setup.pkg` | 28,672,232 字节 |
| Intel | `pixels-agent-bridge-macos-x86_64-release-setup.pkg` | 30,873,068 字节 |

- ARM PKG SHA-256：`1fbe26fab2e2d6c3ec734e052f6d8515c9c4246b6a3e275c512979909af4ce2a`。
- Intel PKG SHA-256：`d14f9d6c3871e5051fb9b3fc14354b0edbfef5362b8eb664ade1cadec596113d`。
- 分别与 `SHA256-macos-aarch64-setup-release.json`、
  `SHA256-macos-x86_64-setup-release.json` 核对一致；清单包含实际内置服务器地址。
- 两个同名架构的 Release `.tar.gz` 也已重新生成，校验和见 `SHA256.json`。
  2026-10-03 的旧包已被新版替换，不能再使用旧哈希验收。
- 两个架构的 app、Executor、MCP 均核验为对应 Mach-O；解包后 app 的
  `codesign --verify --deep --strict` 通过。此处是 ad-hoc app 完整性校验，
  **不是** Developer ID 签名或公证；`pkgutil` 确认两个 PKG 均未签名。
- 每个架构均通过 5 项 PKG 测试（含实际构建/解包）；三平台打包回归、Shell 语法、
  plist、`git diff --check` 通过。系统 Installer 能读取 ARM 包的安装选项。
- 新版 ARM app 的只读诊断成功；没有实际安装/启动后台服务，没有 Intel 实机运行验收。
- 安装包未公证，其他 Mac 的 Gatekeeper/企业安全策略仍可能阻止启动；正式公开
  分发应提供 Developer ID 签名和公证，不要关闭系统安全保护来解决。

完成范围是仓库代码、原生安装包和现有环境可执行的验收。未做真实部署安装、
真实 Docker Engine、实际图形操作、Intel 实机或旧版 macOS 验收；原因和边界见上文。
