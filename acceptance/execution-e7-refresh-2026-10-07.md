# E7 修复版本构建及升级复验

日期：2026-10-07。实现基线 `94ac2a4`，包括 Git 工作进程清理、Mac 退出竞态、
终端关闭输出保留及应用 helper 期限测试。Windows/Linux 构建分配 `1.2.29`（`e8a8c76`），
Mac ARM/Intel 构建分配 `1.2.30`（`0a5d191`），遵循每批构建自动递增规则。

## 完整产物

Windows/Linux 运行 `python scripts/build.py desktop linux --profile debug --package`，退出 0。
Mac 使用 `python3.13 scripts/build.py macos --profile debug --macos-arch all --package`，
正式用户任务 `9d39075b-2bc6-4a7d-bdfc-46dec12b9547`，退出 0。
两批都使用 4 个 Cargo 并行任务；Windows 桌面编译/链接 5 分 53 秒，Linux 程序编译 2 分 14 秒。

本机 `.build/packages/`：

| 文件 | 版本 | 字节 | SHA-256 |
|---|---|---:|---|
| pixels-agent-bridge-windows-x86_64-debug-setup.exe | 1.2.29 | 29624542 | 802aea117c184856a52b749569d3bbbf9051a46f9b1b6f2902007097f772c128 |
| pixels-agent-bridge-windows-x86_64-debug.zip | 1.2.29 | 48135395 | b49e9e6354cf92af6b536a4119604cbb571625b34c93e269ee09334d0482b4e7 |
| pixels-agent-bridge-linux-x86_64-debug.tar.gz | 1.2.29 | 112014665 | 8c08ce1d8452dbffb338ca820850df5e91ce5e3b31382fcdd0d9301ddc469343 |
| pixels-agent-bridge-macos-aarch64-debug-setup.pkg | 1.2.30 | 77244780 | f0e31a6ae05f0f5ecf2077d6971600afd68088e11450864680334dc4d008af90 |
| pixels-agent-bridge-macos-x86_64-debug-setup.pkg | 1.2.30 | 78074883 | ba0c537aec868ac91d8cd75d53e51418b6570672631416910621c359a517cca9 |

Mac 两包通过正式 `pixels.pab_download_file` 下载，完成记录及本机哈希一致。
前批四个平台安装产物已保留在 `.build/packages/archive/execution-e7-first/`。
Mac 继续使用固定免费签名证书；PKG 未做付费签名或公证。Intel 只有构建/签名检查，无实机运行证据。

## Mac ARM：603527578

- 1.2.28 → 1.2.30 完整 PKG 升级成功，Installer exit 0。
- 所有签名产物文件和构建清单一致；设备 endpoint key 哈希未变。
- 三个程序签名均验证通过，指定要求仍绑定证书 `BD9EAA0BA8136249F6FA2D8AB7154512E73AB275`。
- 正式 Pixels 重连成功，服务心跳 Authenticated，Executor 与 session-helper 正常运行。
- 采用无 KeepAlive 的一次性 LaunchDaemon，完成后移除。未重置 TCC，也未退出其他用户应用。
- 升级结果与清理任务 `0edd6089-344b-4420-94c0-7ffe309233c8`；心跳任务 `d8abbbdb-ef74-435b-ac3f-091e66cba955`。
- 私有 SQLite 升级备份在 `/private/var/tmp/pab-e7-refresh-1.2.30/backup` 保留，未下载到报告或公开路径。

## Windows 90：211399447

- 1.2.27 → 1.2.29 完整 NSIS `/S` 升级成功，exit 0。
- Executor/MCP/Desktop 三个文件哈希与构建清单一致，设备 endpoint key 哈希未变，服务 Running。
- 正式 Pixels 重连、Administrator 的 user/desktop_user 上下文查询成功。
- 一次性 SYSTEM 安装任务已移除，结果核对任务 `fa3a5fa7-2086-4d18-b00f-1f06130d4631`。

## 安装后终端关闭首轮

使用当前宿主正式 `pab_open_terminal` / `pab_terminal_input` / `pab_terminal_read` / `pab_terminal_close`：

| 目标 | 会话 | 实际身份及启动 | 结果 |
|---|---|---|---|
| Mac 1.2.30 | 6c884d59-f233-4b5f-bd8c-5ce5164db45c | huayang / UID 501，zsh `-l -i` | 中文输出、关闭成功；远端持久状态 closed |
| Windows 90 1.2.29 | dbcde4ea-ce4e-4d37-b283-17c0d23b4bd6 | Administrator / WTS 1，PowerShell `-NoLogo -NoProfile` | whoami、中文输出、关闭成功 |
| Linux 1.2.29 | ed64c82b-4c65-4da6-9de9-60c6e6776a4d | pabuser1 / UID 23001，`/bin/sh -i` | 中文输出、关闭成功；远端持久状态 closed |

本机只读检查三个会话的归档文件，分别为 386、457、121 字节，均保留实际中文输出行。
Windows 最后一行由 close 排空后写入归档，不能只凭先前 read 返回的部分内容判失败。
Mac 远端数据库只读核对任务 `e2ac10df-4edc-45cc-97a9-1746e6df45cb`。
关闭后旧 MCP 再 read 返回 `terminal session is not active`，符合活动会话已移除，不视为关闭失败。
此时本机 MCP 仍为 1.2.27；大输出、并发关闭及新 Bridge 归档逻辑还需本机升级并重载后正式复验。

## Linux 无界面：565893930

- 通过正式上传工具传入完整包并核验 SHA-256；一次性 systemd 作业执行 1.2.27 → 1.2.29 升级。
- 两个安装文件与构建哈希一致，服务产生新的 Authenticated 心跳。
- endpoint key、device_access、原任务 `b8244140-ccd1-4446-a277-8ff1a44cef2a` 及其事件/输出哈希保持。
- 保留的 pabuser1 首轮测试目录逐项比较路径、UID/GID 和文件内容，升级前后一致。
- 升级结果在 `/var/tmp/pab-e7-refresh-1.2.29/result.json`，私有数据库备份在该目录的 `backup` 下。
- 结果查询任务 `a6177326-c970-433b-a54a-a466823b04bd` 的升级断言通过；随后停止已被 `--collect`
  回收的 timer 返回未加载，使查询任务 exit 1。这是夹具清理检查错误，不是安装失败，没有重放安装。
- 独立只读任务 `18b74e8b-1e81-42c5-bc5b-9362e23f2ba4` 确认 timer/service 都是 not-found，无安装作业残留。
- 正式连接、原用户发现和终端关闭通过；数据库 closed 核对任务 `be93b54a-5037-4d8e-b72d-bd8997588c69`。

## 本机 Windows

远端操作完成后，使用同一 NSIS `/S` 完整升级本机到 1.2.29，Installer exit 0。
三个安装文件哈希与构建清单一致，endpoint key 哈希保留，Executor 服务 Running。
结果在 `.build/local-refresh-1.2.29/result.json`，升级夹具进程退出 0。
安装已结束旧 MCP，检查时 `pab-mcp` 进程数为 0。本次会话尚未重载新 MCP；
不能把前面的旧宿主/新 Executor 小输出测试说成新版 MCP 的完整正式验收。

## 尚待完成

本报告不替代 E8 完整两轮工作流。Linux 三种身份的两轮、双桌面应用编辑/另存为/退出、
Windows 多会话与 split-token、安装后 UI 三语/主题，以及新 MCP 大输出并发归档验收仍待完成。
Mac 仍锁屏，旧 TextEdit 测试文档继续保留，解锁后才能做完整桌面验证与清理。
