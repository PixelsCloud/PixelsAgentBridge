# 执行身份长任务最终交付

日期：2026-10-07。已完成本轮开发、可用设备验收、修复打包安装和测试资源清理。
产品修复提交：Mac `1a65c9e`、Linux `40f48f2`，均已推送。源码版本 1.2.39。
当前宿主使用正式原生 Pixels MCP 1.2.32；没有独立 stdio 脚本替代宿主验收。

## 已安装与产物

| 平台 | 安装情况 | 最终产物版本 / SHA-256 |
|---|---|---|
| Windows x86_64 | 本机及 90（211399447）均已安装 | 1.2.32 / a9c9514fa3c85b1f42194ec8d269745e6c4205466654297cba8622f0f56560c6 |
| Mac ARM | 603527578 已完整安装、固定证书/TCC 保留 | 1.2.37 / e031037186397ee0fb65d1f23eeb429ec93573459b2caa5856f81f041090c7b4 |
| Mac Intel | 完整 PKG、签名及哈希检查，无实机 | 1.2.37 / c1ee2069bce343e043d4293027c203aa290bce5957235df37e8922427f5b90b8 |
| Linux x86_64 headless | Debian 12 测试端 565893930 已升级 | 1.2.39 / 67586aa8e88df94cbb10ebf182d99426ee9ee76f90964e97e97d928ce5fb12fe |

产物位于本机 `.build/packages/`，文件名依次为
`pixels-agent-bridge-windows-x86_64-debug-setup.exe`、
`pixels-agent-bridge-macos-aarch64-debug-setup.pkg`、
`pixels-agent-bridge-macos-x86_64-debug-setup.pkg`、
`pixels-agent-bridge-linux-x86_64-debug.tar.gz`。
不同版本反映各平台实际修复构建，不为对齐数字重复安装未受影响的平台。
测试本身没有消耗产品版本；构建仍由统一入口自动递增。

## 验收结果

- Windows 指定 Administrator、Mac UID 501、Linux service 与 UID 23001/23002 均完成
  两轮命令/中文文件/八种 Git/文件与 ZIP/二进制传输/PTY 的核心工作流。
  本轮九份 256 KiB 下载 SHA-256 一致；另一轮 Linux 用户证据来自已提交 native-reload。
- Windows/Mac 实际编辑器输入中文、保存后读取内容/归属、正常关闭完成。Windows helper
  故障后自动恢复且保留应用；Mac 保存面板兼容修复已通过完整安装复验。
- Windows 三语×亮暗任务身份页面、Mac 三语×亮暗设置页面通过；Mac 实际选择 huayang
  执行 id，任务历史与实际 UID 501 一致。Mac 没有冒充六种组合都重复执行任务。
- Linux 运行中服务重启保留 interrupted 原任务，无重复及迟到副作用；跨版本升级保留
  设备身份、凭据、任务、事件和输出。桌面请求明确 unsupported，旧 MCP 也能得到确定失败。
- 大输出终端并发读取/关闭/归档、跨用户权限、去重身份冲突、断线观察和旧上下文拒绝已有
  原生证据；SSH/Keychain/慢凭据程序及工作进程边界另有分层测试报告。
- Mac desktop-control 47 项、Linux Executor 153 项及新增失败去重用例、最终源码 Mac
  IPC 22 项通过。各执行版本、忽略项及测试层次在对应报告注明，不合并伪称最终全套重跑。

证据：

- [核心两轮与恢复](execution-e8-final-rounds-2026-10-07.md)、[逐步操作引用](execution-e8-final-rounds-2026-10-07.json)
- [Mac 安装与真实 UI](execution-e8-macos-installed-2026-10-07.md)
- [Linux 最终安装与旧 MCP 兼容](execution-e8-linux-final-2026-10-07.md)
- [本机安装 UI](execution-e7-ui-refresh-2026-10-07.md)、[正式宿主重载回归](execution-e8-native-reload-2026-10-07.md)
- [安装操作索引](execution-e8-installed-final-2026-10-07.json)、[Mac 锁屏输入复验](macos-lock-input-regression-2026-10-07.md)

## 保留边界与清理

没有 Intel、多屏、两个同时活动的 Windows WTS 桌面及完整 split-token 组合的真机条件。
这些是未测范围；源码/模拟测试不冒充硬件验证。现有 Windows 验收用户是 Administrator，
不是所有域账户或普通用户排列。Mac 免费固定签名不等于公证。

Windows focus 系统拒绝仍如实报告。Mac 首次关闭保存空文件、保存位置与期望不同等过程
均记录，最终显式保存并回读才确认成功。AX 接受或启动复用不等于应用完成目标操作。
原生命令输出要求 UTF-8；没有加入猜测任意本地代码页的解码器。

测试树、测试编辑器文稿/窗口、临时 Desktop 自连记录、安装一次性作业、AX 探针和
临时 caffeinate 已清理。Mac 按键账本七份均为零，helper 继续工作。没有重置 TCC、
改用户真实账户或清理用户数据。Linux 测试容器及数据卷、目标机私有升级备份保留；
编译缓存与最终安装包保留供后续开发和交付。报告不包含输入密码、环境值或原始工具参数。
