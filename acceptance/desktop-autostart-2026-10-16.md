# 桌面登录自启动

日期：2026-10-16。本次修改安装配置及脚本。经用户授权，在 Ubuntu 虚拟机修改启动项并重启验证；没有更换已安装程序二进制或重新发布 COS 安装包。

## 行为

- Windows：NSIS 和 ZIP 安装共用 `install.ps1`，写入所有用户 Startup 目录下的 `Pixels Agent Bridge.lnk`，直接以登录用户启动主程序，窗口正常显示。重复安装更新同一快捷方式；卸载仅删除指向本次安装目录的启动项。
- macOS：新增 Aqua 会话的 `com.pixelsagentbridge.desktop` LaunchAgent，通过系统 `open` 打开应用。安装时注册当前图形会话，后续用户登录时自动打开。没有 KeepAlive，不会在用户主动退出后立即重启。新 plist 纳入 TAR/PKG 必需文件、升级备份与回滚、各用户会话的卸载清理。
- Linux 桌面 DEB：XDG 登录启动项改为正常启动，去掉 `--background`。文件由 dpkg 管理，升级替换、卸载移除。已有用户自定义禁用项不被改写。无桌面的 Linux 命令行包继续只启动后台服务。
- 三平台均沿用已有主界面单实例处理。后台服务开机运行；主窗口需进入图形登录会话后显示。

## 验证

- Windows PowerShell 5.1 `test-desktop-startup.ps1` 通过：真实临时快捷方式覆盖可见窗口、含空格路径、重复安装、更新目标、缺少程序拒绝、卸载归属检查及重复卸载。未写入真实 Startup 目录。
- `test_desktop_startup.py`：4 项通过。检查 Linux 启动项、Bash 语法，并用模拟 launchctl 执行安装/卸载中的真实脚本片段，覆盖多用户卸载、新安装/升级注册、重复注册。
- `test_packaging.py`：1 项通过，实际构建三平台测试归档并检查新 macOS plist 的内容、权限和归档归属。
- `test_macos_pkg.py`：5 项通过、4 项跳过。通过项包含新 LaunchAgent 的正常用户环境、可见启动、无 KeepAlive 和 PKG 必需文件检查。跳过项需要 macOS 工具、POSIX 路径或指定的实包。
- 用户授权后，Ubuntu 22.04 VMware 虚拟机已完成真实重启及 GNOME Wayland 登录验证，主窗口自动显示，只有一个 GUI 进程；系统服务、原配置及设备身份正常。证据见 [Linux 登录自启动验收](linux-autostart-2026-10-16/README.md)。macOS 实机登录测试仍未执行。

除上述已修改启动项的测试虚拟机外，其他安装仍需要后续新安装包才会生效。
