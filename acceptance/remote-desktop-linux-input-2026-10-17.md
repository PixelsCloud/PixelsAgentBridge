# Linux GNOME 原生输入阶段验收

此记录延续 `216d103` 的 Linux 媒体 helper 接入。目标是让现有输入通道真正驱动
Linux 桌面；不把本阶段当作 W5 全部完成。

## 实现

Linux worker 新增 `input SCREEN_ID`，沿用既有启动几何与 8 字节输入协议。
Desktop helper 开启原生输入能力，Executor 通过现有身份配对、代次与操作开关
路由。未更改远程账号、设备密码、可靠网络传输或单观看者规则。

普通 GNOME 会话使用 Mutter RemoteDesktop v1 创建独立虚拟输入设备，关联所选
屏幕的 ScreenCast stream。绝对位置使用屏幕流像素坐标；GNOME 负责转换为桌面
原点与缩放后的坐标。输入启动核对原点和逻辑大小，显示布局变化或会话关闭时，
即使没有新输入，旧 worker 也会退出。基础键码转换保留目标系统的物理键布局与
修饰键处理，未实现 Unicode 文本注入或输入法专用协议。

worker 跟踪自己按下的键和按钮，正常关闭及错误时释放，再停止 GNOME 会话。
进程被强制结束时，由 GNOME 销毁该 D-Bus 客户端的虚拟设备；不会全局释放本地
物理键盘鼠标。输入后备恢复入口因此没有全局按键日志或全局注入动作。

参考了固定 RustDesk `7aa98d43cf1962a7a29ec16ffef42974377ef11e` 的
`src/server/wayland.rs` 和 `src/server/uinput.rs`，核对多屏坐标与输入生命周期问题，
未复制其实现。本阶段的 GNOME 适配为本项目编写，接口依据：

- [Mutter 42.9 RemoteDesktop 接口](https://raw.githubusercontent.com/GNOME/mutter/42.9/data/dbus-interfaces/org.gnome.Mutter.RemoteDesktop.xml)
- [ScreenCast 会话与流接口](https://raw.githubusercontent.com/GNOME/mutter/42.9/data/dbus-interfaces/org.gnome.Mutter.ScreenCast.xml)
- [屏幕流坐标转换](https://raw.githubusercontent.com/GNOME/mutter/42.9/src/backends/meta-screen-cast-monitor-stream.c)
- [输入会话及客户端退出处理](https://raw.githubusercontent.com/GNOME/mutter/42.9/src/backends/meta-remote-desktop-session.c)

## 已验证

环境：Ubuntu 22.04.5 GNOME Wayland 虚拟机 `556688188`，单屏 1314×742，
RemoteDesktop Version=1、SupportedDeviceTypes=7。未替换已安装程序或服务。

- `native_linux.cpp` 扩展到 70 项边界检查，在 Jammy 环境中通过 ASan/UBSan、
  `-Wall -Wextra -Werror`。覆盖坐标端点与越界、主要键码映射、原有采集缓冲边界。
- Cargo Release worker 在自建 GTK 窗口内通过 10 项真实输入检查：定位点击、
  字母及 Shift/Ctrl 组合键、按住拖拽、双向滚轮、主动释放、半包 EOF、进程被杀、
  错误包、过期几何，以及按键按住期间的空闲布局变化。
- 布局变化测试暂时切到 1920×1080，等待旧 worker 退出，恢复原布局后重开输入，
  验证普通字母不再带 Ctrl/Shift/鼠标按住状态。共 7 个 worker 均回收。
  没有保存真实屏幕像素或按键内容。

- VM 上正式 helper 模块的 4 项测试通过，其中真实采集经过身份核对与压缩帧
  通道完成解码，同时配对原生输入 worker 并收到操作确认。两次关闭重开均清理
  输入/采集进程及注册。该项只发送 ReleaseAll，不向任意用户窗口键入文本；
  真实按键与指针效果由上面的自建 GTK 窗口测试验证。

- 完整 Linux Desktop Release 构建通过，产物 SHA-256 为
  `4eda5ec41d307e0e11a9017526a1fc87e8be0dc5fe4db33e63472ba74d7e57d2`，
  44,408,632 字节。同一产物的内嵌 `--media-worker` 再次通过全部 10 项实机
  输入测试，确认原模式和逻辑布局恢复；临时 native 进程为 0。
- Windows 完整 Desktop 的 `cargo check --locked` 通过。所有编译均串行调度，
  Cargo 最大 8 jobs，嵌套 CMake 1 job，MSVC `/MP1`；原生 sanitizer 检查单进程。

详细任务 ID、结果与源码/产物哈希见
[结构化证据](remote-desktop-linux-input-2026-10-17.json)。

## 仍未完成

以上包含真实 GNOME 输入，但不证明正式远程窗口通过网络操作 Linux 的端到端
闭环。Linux 原生显示、X11、其他 Wayland 桌面、GDM/系统登录、真实多屏混合 DPI、
输入法和九种平台组合仍需实现或验证。没有将普通桌面输入称为系统登录支持。
