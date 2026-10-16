# Linux 产品范围与发布方案

## 实施状态（2026-10-15）

一行安装入口及发布服务已部署到官网，Linux x86_64 stable 当前正式包 **1.0.2** 已签名发布到 COS，增加 Ubuntu 20.04 兼容。以下命令已完成真实在线安装验收：

```bash
curl -fsSL https://agent.rgaa.vip/install.sh | bash
```

- 主仓库 `packaging/desktop/linux-bootstrap.sh` 是脚本源文件，`scripts/sync_linux_bootstrap.py` 同步到官网 `web/public/install.sh`。
- 引导脚本检查平台、签名、下载大小和 SHA-256，拒绝非预期包路径及归档条目，检查 ELF 和动态运行库后再调用安装器。
- 发布脚本、版本预留、带版本号的 tar.gz、包内 `release.json`、Ed25519 签名、官网上传/finalize 和管理页已增加 Linux x86_64 stable 支持。
- 项目自己定义的程序配置统一 TOML，详见 `docs/CONFIGURATION.md`。Linux 使用 `/etc/pixels-agent-bridge/config.toml`，安装与重装保留已有设置及注释；旧 env 只按数据迁移，禁止执行。
- Linux 安装参数只设置首次默认值。后续编辑 TOML，再执行 `sudo systemctl restart pixels-agent-bridge-executor.service`。
- 容器需要同时持久化 `/etc/pixels-agent-bridge` 和 `/var/lib/pixels-agent-bridge`，无 systemd 时使用 `--no-service`。
- 编译、配置迁移、MCP 回归、隔离安装及官网检查已通过，早期结果见 [源码验收记录](acceptance/toml-linux-install-2026-10-15.md)。最终 Release 包已在 Debian 12/systemd 和 Ubuntu 24.04/外部进程管理容器完成官网在线安装、注册、重装保留及远程使用验证，见 [发布验收记录](acceptance/linux-online-install-2026-10-15/README.md)。
- 在线验收发现 `/tmp` 禁止执行会使运行库预检误报，官网引导脚本已改用当前用户主目录下的私有临时目录，并在保留 `/tmp` 的 `noexec` 限制下重测通过。
- V1.0.2 以 glibc 2.31 环境构建，签名校验使用 Python cryptography，不要求 OpenSSL 3。本机 WSL Ubuntu 20.04.2 及 Ubuntu 20.04/22.04/24.04、Debian 12 容器均通过在线安装和远程使用；本机 WSL 的 PDF/OCR 十项样例也通过。详见 [Ubuntu 20.04 兼容验收](acceptance/linux-ubuntu2004-2026-10-15/README.md)。
- 官网 Linux 下载区已直接展示一行安装及查看设备码/密码的命令，包含复制按钮、中英文和手机布局。
- 同一 V1.0.2 正式包已补充通过 Debian 11 的官网安装、自动补齐验签依赖、覆盖安装保留、systemd 启动、远程使用和 10 项文档/OCR 样例验收；Debian 已实测 11、12。Debian 11 测试环境使用官方签名快照解决软件源 404，详见 [Debian 11 验收记录](acceptance/linux-debian11-2026-10-15/README.md)。

## Linux 桌面版（2026-10-16）

独立的 Ubuntu 22.04 x86_64 Tauri/React GUI 已生成 V1.0.0 deb，并在 VMware 的 Ubuntu 22.04.5 / GNOME Wayland 安装验收。账号登录、连接 Company、远程命令和流式终端、设置、单实例与菜单恢复、覆盖安装/卸载重装保留均通过；后台直接复用已验证的 V1.0.2 组件。构建、动态库和产品版本均与无界面包独立。

构建入口为 `scripts/build_linux_desktop.py`，具体参数及安装方法见 [桌面包说明](packaging/desktop/linux-desktop/README.md)，结果见 [桌面验收记录](acceptance/linux-desktop-2026-10-16/README.md)。GUI V1.0.1 已通过 `scripts/release.py linux-desktop` 独立目标签名发布到 COS，官网已提供下载、安装说明和后台管理，并完成官网下载安装包的真实覆盖升级。见 [发布验收](acceptance/linux-desktop-publish-2026-10-16/README.md)。GUI 尚未接入在线安装升级；升级使用新版 deb。官网一行安装仍提供已发布的无界面服务。仅本次 Ubuntu 桌面已实测，不扩展宣称其他发行版或本机 Wayland 桌面操控已通过。

下面保留改造前的调查作为历史依据；其中“当前限制”和旧包名指初次调查时的状态，以前述实施状态及当前源码为准。


日期：2026-10-15。状态：发布方案，官网 Linux 发布链路尚未接入。

建议首版发布 Linux x86_64 无界面设备服务包，复用现有 tar.gz、安装脚本和 systemd 服务。用户从 Windows 或 macOS Desktop 连接 Linux，使用远程终端、命令、文件、Git 和文档工具。Linux 本地 AI 客户端可以使用包内 MCP。Linux Desktop、ARM64、deb/rpm 和自动升级后续分别交付。

## 当前已具备的能力

- 构建入口：[scripts/build.py](scripts/build.py) 的 `linux` 目标，在 Windows 宿主通过 Docker 构建；当前入口明确拒绝在 Linux 宿主运行该目标。
- 构建环境：[Dockerfile.linux](packaging/desktop/Dockerfile.linux) 使用 Rust 1.95 和 Debian bookworm；[build-linux.ps1](packaging/desktop/build-linux.ps1)复用 `.build/linux-cargo-home` 和 `.build/linux-target`。
- 包内有 `pab-executor`、`pab-mcp`、`pab-document-worker`，以及离线 PDFium、OCR 数据和安装脚本。没有 Tauri 或 Linux 图形桌面组件。
- [安装脚本](packaging/desktop/unix/install.sh)支持 systemd 和 `--no-service`。安装目录为 `/opt/pixels-agent-bridge`，设备数据为 `/var/lib/pixels-agent-bridge`；覆盖安装和卸载保留设备数据。
- Linux root Executor 可以使用已有执行上下文能力，以明确选定的系统用户执行操作；普通用户不需要有图形登录会话。
- [2026-10-15 验收](acceptance/desktop-terminal-linux-2026-10-15/README.md)覆盖 Debian/systemd 安装实例和隔离的 Ubuntu 原生目标，包括普通用户、服务账户、P2P、Relay、大输出、终端尺寸和取消。这些记录不包含 Linux Desktop 验收。

作为被控端，Linux 不要求用户打开桌面界面或登录操作端账号；发起远程控制的 Desktop/MCP 仍遵守现有账号与设备密码规则。本地 MCP 以 AI 客户端的用户运行，其账户与数据不等同于 root Executor。

## 当前包与正式发布的区别

本次检查的本地包为 `.build/packages/pixels-agent-bridge-linux-x86_64-release.tar.gz`，清单记录发布版本 `1.2.99`，大小 47,372,475 字节。实际 SHA-256 与清单一致：

`abadae86b4878f16942aedd9996d68c633fe34a88d9cf134da0b0fbf1156b74c`

截至本次检查，[官网公开目录](https://agent.rgaa.vip/api/releases)只包含 Windows x86_64 和 macOS aarch64。Linux 包已能本地构建和安装，但尚不能通过统一发布脚本发布到官网。

发布链路存在以下明确限制：

| 位置 | 当前限制 | 所需增量 |
| --- | --- | --- |
| [scripts/release.py](scripts/release.py) | 参数和 stable 目标仅支持 Windows/macOS；产物按 EXE/PKG 查找 | 增加 Linux x86_64、tar.gz 清单校验和恢复流程 |
| [scripts/build.py](scripts/build.py) | `--installer-version` 只允许 desktop/macos | Linux Release 包接受官网预留版本，不重复分配本地版本 |
| [packaging/desktop/build.py](packaging/desktop/build.py) | Linux 归档文件名没有版本，每次构建覆盖同名包 | 生成带版本号的归档与清单 |
| [scripts/sign_release.py](scripts/sign_release.py) | 只识别 Windows EXE/macOS PKG | 支持 Linux 文件名、平台和架构签名 |
| 相邻官网仓库 `src/release_api.rs` | 目标验证、上传及 finalize 只接受两种桌面平台 | 增加 `linux-x86_64-stable` 并统一目标与产物规则 |
| 官网 `web/src/ReleaseAdmin.tsx` | 发布目标和文件选择器只有 EXE/PKG | 增加 Linux 和 tar.gz |
| 官网下载页 | 已有 Linux Headless 卡片，没有已发布记录 | 首个有效发布后展示下载、安装方式和兼容范围 |

官网路径为 `D:/GoCloud/PixelsAgentBridgeSite`。现有版本预留、COS 上传、SHA-256、Ed25519 签名、finalize 和历史激活机制均应复用。公开版本按目标独立分配，不要求 Linux 与 Windows/macOS 同号，也不能只给旧二进制换新版本名。

建议正式文件名为 `pixels-agent-bridge-linux-x86_64-release-<version>.tar.gz`，归档内增加产品发布清单，记录版本、架构、提交、文件哈希和依赖要求，安装后保留清单供查询。组件内部版本继续保持独立。

## 平台兼容与安装预检

现有安装说明声明 x86_64、glibc 2.36 或更新；构建基线是 Debian 12。首版建议以 Debian 12 和 Ubuntu 24.04 x86_64 为完整安装验收目标，通过后写入支持清单。其他发行版不能仅凭 glibc 版本宣布支持。

本次对实际 1.2.99 归档做 ELF 检查，得到以下直接依赖下限：

| 文件 | GLIBC 最高引用版本 | GLIBCXX 最高引用版本 |
| --- | --- | --- |
| pab-executor | 2.34 | 3.4.29 |
| pab-mcp | 2.34 | 无直接引用 |
| pab-document-worker | 2.35 | 3.4.29 |
| libpdfium.so | 2.16 | 无直接引用 |

这些符号版本不等于完整运行兼容性。Executor 和文档工作进程需要 `libstdc++.so.6`；包也需要目标系统提供相应 glibc、libgcc 等运行库。Ubuntu 22.04 的 glibc 为 2.35，仅据此不能判定当前包不可运行，也不能宣布支持；需要完整安装、运行和 OCR 验证。[Debian libc6](https://packages.debian.org/bookworm/libc6)、[Ubuntu 22.04 libc6](https://packages.ubuntu.com/jammy/libc6)、[Ubuntu 24.04 libc6](https://packages.ubuntu.com/noble/libc6)提供发行版基线。

安装器应在停止旧服务和替换文件之前，完成架构、所需动态库、包内文件与运行时完整性检查。目前脚本仅检查 Linux、root、URL、文件存在和 systemd，文档运行时检查发生在停服务之后，需要前移。

ARM64 需要扩展构建目标、PDFium 资源、包名、官网目标及真实环境测试；当前脚本和 PDFium 资源均固定 Linux x86_64。Alpine/musl 也不属于现有包的支持承诺。

## 现在可执行的构建与安装

在 Windows 发布工作目录运行以下命令，可以生成本地 Release 包，但不会上传官网：

```powershell
python scripts/build.py linux --profile release --package
```

当前输出为 `.build/packages/pixels-agent-bridge-linux-x86_64-release.tar.gz` 和 `.build/packages/SHA256.json`。普通构建会递增本地安装包版本；正式接入官网后应使用官网预留版本。

用户将包下载到 Linux 并完成校验后，解压到独立目录：

```bash
mkdir pixels-linux
tar -xzf pixels-agent-bridge-linux-x86_64-release.tar.gz -C pixels-linux
cd pixels-linux
sudo bash install.sh wss://YOUR_SERVER/control https://YOUR_RELAY
sudo systemctl status pixels-agent-bridge-executor.service
sudo /opt/pixels-agent-bridge/run-executor.sh show-access
```

服务注册成功后，`show-access` 显示设备码和密码，用户在本机 Desktop 添加该设备。控制服务和 Relay 地址使用同一部署的真实地址；发版文档应提供官方部署的已核实值，并保留自部署配置示例。

无 systemd 时使用 `install.sh --no-service`，由外部进程管理器运行 `/opt/pixels-agent-bridge/run-executor.sh`。容器持久化挂载 `/var/lib/pixels-agent-bridge`；该模式控制的是容器环境，不能描述为自动获得宿主机控制能力。

Linux 上使用 AI 客户端时，将 `/opt/pixels-agent-bridge/run-mcp.sh` 注册为 stdio MCP 命令，以该 AI 客户端的用户运行。已有 `pab-mcp account login/register <username> --password-stdin` 及 `account status` 命令；用户文档需补齐隐藏输入密码、账号登录和添加目标设备流程。

首版升级方式为下载新包并重新运行安装脚本，保留原控制服务与 Relay 配置。安装会重启服务并中断正在使用的产品进程。服务恢复后核对身份、连接和工具；无 systemd 的实例先停止外部进程管理器。自动更新、回滚二进制与数据库的兼容策略另行设计。

## 正式发版顺序

1. 整理可追溯的发布提交，保留并协调当前工作区其他会话的修改。统一编译最多 10 个并发任务，嵌套构建及同时运行的编译合计不超限。
2. 增加带版本号的 Linux 归档、包内发布清单和安装预检；完整包包含三个程序、PDFium 和 OCR 数据。
3. 主仓库发布脚本和官网 API 同步支持 Linux 目标，保持现有清单签名、上传恢复和服务端完整校验。
4. 使用最终归档验证 Debian/systemd、Ubuntu、无 systemd 三种安装路径，覆盖首次安装、覆盖升级、进程停止、身份与数据保留、错误包拒绝和卸载。
5. 验证命令、指定用户、远程终端、文件/Git、MCP 连接及 PDF/OCR。现有安装测试的无参数路径仍只准备旧的两个二进制，应更新夹具；正式包验收应显式传入最终归档。
6. 部署官网 Linux 发布支持，预留版本，构建、验收、签名、上传并 finalize，最后检查公开目录和实际下载文件哈希。
7. 同步官网中英文安装说明及主仓库 `docs/USAGE_AND_SELF_HOSTING.md`，按官网 AGENTS.md 执行文档同步与一致性检查。

预期接入后的命令如下；**当前尚不支持此命令**：

```powershell
python scripts/release.py linux --arch x86_64 --notes-zh "Linux 无界面服务首发" --notes-en "Initial Linux headless release"
```

首版使用 tar.gz 可以直接复用现有安装生命周期。后续若需要系统包管理器集成，再增加 deb/rpm 与对应仓库；若需要 Linux 本地桌面控制端，再单独交付 Tauri GUI。

## 本次核对结果

Linux 包结构与篡改拒绝测试 `test_linux_package.py` 通过；当前 Release 包 SHA-256 与清单一致，ELF 架构及动态依赖已检查。核对过程未编译产品、未安装或更新本机应用、未修改现有 Linux 实例，也未上传或发布版本。
