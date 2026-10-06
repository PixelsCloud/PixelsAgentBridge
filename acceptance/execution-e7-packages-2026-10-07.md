# E7 正式构建、安装与首轮连接

日期：2026-10-07。实现基线 `cc232f0`。Windows/Linux 一次构建分配 1.2.27，
Mac 在同步这一版本提交后一次构建分配 1.2.28（ARM/Intel 共用版本）。
版本提交：`509fa90`、`f66d7bd`。两个平台批次代码功能相同，版本差异来自约定的构建自增。

## 产物

文件位于本机 `.build/packages/`；Mac 两个 PKG 已通过原生 Pixels 下载到本机。

| 文件 | 版本 | 字节 | SHA-256 |
|---|---|---:|---|
| pixels-agent-bridge-windows-x86_64-debug-setup.exe | 1.2.27 | 29607935 | c095b611cf42af60a4b15e337a7eb996b9bf309e0cce31d40cf16bd87849b6a7 |
| pixels-agent-bridge-linux-x86_64-debug.tar.gz | 1.2.27 | 111945944 | d222ac23c1986f040a628704c35eeed7ce688395783ab94c9813f432e39e99e2 |
| pixels-agent-bridge-macos-aarch64-debug-setup.pkg | 1.2.28 | 77131456 | 30a69e395547b3e495a419e399b800a8dad5d95703119c2001d9ca57173479b5 |
| pixels-agent-bridge-macos-x86_64-debug-setup.pkg | 1.2.28 | 78010591 | 70d8b6924b136eb74737e32ce8d64b8b3a6954801d8d370736d8511da2360abc |

构建入口：`python scripts/build.py desktop linux --profile debug --package`；
Mac `python3.13 scripts/build.py macos --profile debug --macos-arch all --package`。
Mac 保留免费固定证书 `BD9EAA0BA8136249F6FA2D8AB7154512E73AB275`，PKG 本身未做付费 Installer 签名/公证。
Intel 已完成交叉构建及打包签名验证，无 Intel 物理机器运行证据。

## 已安装验证

### Windows 90：211399447

NSIS `/S` 安装完成，结果 exit 0，注册表版本 1.2.27 debug，服务 Running。
安装后三个 exe 哈希与 `.build/builds/desktop-debug.json` 一致；设备 endpoint key 哈希保持。
原生 `pab_connect` 重连成功，环境版本为 native-v2。
验证任务 `5849bd5e-1de1-4802-a76d-c540b6fd0e09`。

进程检查：SYSTEM supervisor；Administrator 会话 1 的 application-helper 和 Default helper；
SYSTEM Winlogon helper。仅证明部署和进程账户正确，不能替代应用启动后的令牌及完整多会话验收。
安装由一次性计划任务执行，完成后任务已移除；核对任务 `55390c27-8052-4946-a394-3851dbe51f48`。

### Mac ARM：603527578

系统 Installer 日志明确 `The install was successful.`，安装 App 版本 1.2.28。
三个程序与签名构建产物逐一比对 SHA-256；验证实际指定要求绑定原固定证书，服务/helper 均 running。
设备 endpoint key 哈希保持，原生 `pab_connect` 成功。验证任务 `fe463b6c-220f-4e64-acb0-26c8da1607fe`。
原生 `pab_list_windows` 和 JPEG 截图成功；截图保持 2560×1080、144621 字节。
Mac 仍在锁屏，窗口缺少可控制引用；桌面编辑/保存及旧测试文档清理待解锁。

安装前将机器/用户 SQLite 用 SQLite backup API 和 endpoint key 备份到 Mac
`.build/upgrade-backup-1.2.28`（root 私有 0700）。未重置 TCC、证书或用户配置。
一次性测试启动器使用 `launchctl submit` 后被系统再次调度，备份目录存在检查在安装前拒绝了重入，
导致测试状态文件被写为 failed。已移除该任务；以原始 Installer 成功日志、安装版本、三程序哈希和
固定指定要求复核确认实际安装成功。没有重复运行安装器。后续一次性安装夹具应使用无 KeepAlive 的 LaunchDaemon。

Mac 原工作区 164 个覆盖文件逐一与提交哈希比较：162 个完全一致，两处仅是已识别旧版 catalog
及截图关闭动画差异。已保留 stash `execution-verified-overlay-before-cc232f0`，fast-forward 到正式提交，
未覆盖未知用户改动。旧 Finder stash 仍保留。

### Linux 无界面：565893930

Docker 容器 `pab-execution-e8-linux`（Debian 12/systemd、无 DISPLAY），实际解包并安装 1.2.27。
两个程序哈希与 Linux 构建记录一致，Executor 服务 active/running。持久数据卷
`pab-execution-e8-linux-data` 保留供后续重启/升级验收；测试账户 UID 23001、23002。

一次性源码辅助程序复用 Desktop 的 BridgeRuntime 认证流程连接并保存设备凭据；凭据从只读查询
通过内存 stdin 传递，没有输出到日志或命令行，也没有直接修改凭据数据库。辅助源码已清除。
随后使用当前宿主原生 `pixels.pab_connect` 连接成功。
原生命令任务 `b8244140-ccd1-4446-a277-8ff1a44cef2a` 验证 service UID/GID=0、DISPLAY 未设置、
中文文件内容/归属正确并清理。原生窗口查询明确失败 `interactive window helper is unavailable`，没有假装有 GUI。

### Windows 本机

最后使用完整 NSIS `/S` 升级本机，安装进程 exit 0。注册表版本 1.2.27 debug、Executor 服务 Running，
三个已安装程序 SHA-256 与构建记录一致，设备 endpoint key 哈希保持。证据
`.build/local-install-1.2.27-result.json`。安装器已结束两个旧 MCP 进程，检查时 MCP 进程数为 0；
当前 AI 会话须重启才能加载新增工具及 execution 参数，不能据此宣称正式宿主新能力通过。

## 仍未完成

- 新会话重载 68 个工具及新的 execution 参数。
- 三平台指定用户及应用的正式宿主两轮工作流、完整 E6 异常矩阵。
- Mac 解锁后的应用/文档清理和 UI 验收、Windows 分离令牌/多会话及生命周期测试。
- Linux 服务重启/升级后真实设备身份及两用户工作流。

本报告是 E7 安装及原生旧能力首轮证据，不能视作 E8 完成。
