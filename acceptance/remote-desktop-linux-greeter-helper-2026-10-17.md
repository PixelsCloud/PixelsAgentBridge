# Linux 登录界面独立媒体 helper

日期：2026-10-17，基线 `e2701ca`。本阶段打通 GDM Xorg 下的媒体身份校验与
原生 IPC 测试，并提供 Desktop 的独立 `--media-helper` 入口。
[机器记录](remote-desktop-linux-greeter-helper-2026-10-17.json)包含产物哈希和实机结果。

## 改动

`LinuxGraphicalSession::for_process` 保留仅用户会话的规则。新增媒体专用观察入口，
接受同 UID、活动、本地、X11/Wayland 的 greeter；UID/PID 仍由原有内核通道验证，
会话类型、class、seat 来自 logind，参与后续同一会话检查。root、其他 UID、远程、
非活动及非图形会话仍被拒绝。没有新增网络认证协议，也没有将 greeter 注册成
命令或应用执行用户。

Linux `--media-helper` 只启动媒体循环与信号退出处理，不进入已有应用请求循环。
GUI 和普通 `--session-helper` 保持用户会话路由。Windows 的 Default/Winlogon
入口条件保持原样；本阶段未改变 macOS helper。

## 验证

- 完整 Linux Desktop Release 构建通过，Cargo 8 jobs、嵌套 1。随后检测到另一
  会话 Cargo 2 jobs/CMake 2 jobs，本阶段的测试构建降至 4 jobs、嵌套 1。
  未覆盖或安装本机应用；只将新产物放入已存在的 VM 隔离目录。
- 2 项 logind 策略测试通过，覆盖普通用户规则不变、greeter 只允许媒体、UID
  不匹配、远程、非活动、错误 class/type 和 root 拒绝。
- VM GDM Xorg 会话 c7，UID 128。logind 的 `sd_uid_get_display(128)` 返回 c7，
  因此从 systemd 独立单元启动的该用户媒体进程可以走已有 ENODATA 后备观察。
- 正式 worker 采集 1314×742 并解码；独立 Xlib 查询确认鼠标移动至 (657,371)，
  Shift 确实按下、释放，ReleaseAll 后无该键残留，再恢复原鼠标位置。
- 新 Desktop 的 `--media-helper` 在 greeter 用户下、独立数据目录和关闭的 IPC
  测试端口存活，SIGTERM 后退出码 0；未读取机器访问 token。此项是入口生命周期
  检查，**不冒充 Desktop 到正式安装服务的认证连接验收**。
- Cargo 构建的 Executor 实机测试在同一 greeter 用户下运行，两轮真实 PID/UID/
  logind 校验、helper 注册、原生画面采集/解码、配对输入 ReleaseAll、worker 回收
  及注册撤销通过。这是生产 IPC 实现的隔离测试，未经过外部网络或正式观看窗口。
- 原 hy/Wayland 会话恢复后，同样的两轮原生 helper 测试和 Desktop 媒体入口
  生命周期回归通过。初次经普通用户命令环境运行缺少 D-Bus 环境，任务
  `13d4fe62-3ce3-404a-9da9-3809ecc8c6e3` 明确失败；修正测试启动器为 systemd
  用户身份加已核实的图形会话环境后通过，没有修改原命令执行环境语义。

切换前均设置独立恢复计时器。一次新 Xorg 启动过程的第一帧接近纯黑，测试
明确未通过，随后改为有界等待画面启动，不把 X server 就绪等同于登录 UI 就绪。
该次测试使用 `/run/gdm3/custom.conf` 临时偏好 Xorg，结束后删除并重载；后续
c7 通过轮次复用已有 Xorg greeter，未再次改配置。原 `/etc/gdm3/custom.conf`
哈希保持不变。恢复后复查原 Wayland 会话活动且未锁定，临时媒体进程及临时
恢复/入口测试目录为零，GDM runtime override 不存在，原 Executor active。

## 剩余事项

本阶段没有输入系统密码，也没有完成真实注销再登录。尚需实现 root 服务端
自动发现/启动/回收媒体 helper、经过验证的会话环境交接、安装单元和正式窗口
连续登录验收。Wayland 普通锁屏与 GDM 禁止 PipeWire 的问题仍需独立媒体路径。
真实会话切换触发注册退出尚未作为本测试的通过项；两轮撤销测试由关闭媒体
会话触发。W5、W7、W8 保持未完成。
