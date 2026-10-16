# Linux 图形会话与媒体 helper 阶段验收

此记录对应 `bd6366d` 后的 Linux helper 接入，不能代替 W5 的完整验收。
采集 worker 的基础证据见 [Linux 采集阶段](remote-desktop-linux-capture-2026-10-17.md)。

## 实现范围

- 从 `/proc/PID/status` 读取进程的有效 UID/GID，沿用原有进程生命周期标记和
  helper 通道身份挑战。没有新增远程账号、密码或认证流程。
- 使用 logind 观察本地活动图形用户会话。用户管理器启动的进程没有直接会话
  关联时，仅在 ENODATA 情况下查询该 UID 的主要图形会话；核对 UID、活动状态、
  本地会话、X11/Wayland 类型、user 类别与 seat。没有将 POSIX SID 冒充 logind ID。
- Desktop GUI 和独立 session helper 启动媒体连接。注销或活动会话变化时，
  helper 和 Executor 均检查旧注册失效；静态画面等待期间也会检查。
- 继续使用原有压缩媒体管道，应用执行、窗口与截图的原有通道保持现有策略。
  Linux 原生输入未接入，因此不对外宣称支持操作。

## 已完成的实测

环境：设备 `556688188`，Ubuntu 22.04.5、GNOME Wayland、hy UID 1000、
logind 会话 `13`、seat0，2 vCPU VMware 虚拟机。

1. Jammy 构建环境中的 3 项 Linux OS 测试通过：有效身份解析、当前进程身份、
   图形会话条件检查。动态加载 libsystemd，不要求目标机安装开发头文件。
2. `linux_graphical_check` 在 VM 的用户进程中识别到上述真实会话；拒绝 root
   作为普通图形用户和无效 PID。远程任务：`62e92024-6838-4961-bd27-9b9266cd8281`。
3. VM 中运行 4 项媒体 helper 测试全部通过。真实 GNOME 画面经过身份核对的
   helper 管道，连续两次完成注册、1314×742 采集、软件解码、关闭和重开，
   旧注册清除、worker 回收。另覆盖启动/静态等待取消与输入通道断开释放。
   其中输入使用测试后端，不能算 Linux 原生鼠标键盘验收。
   远程任务：`33808a98-aab7-421d-ac91-d68401be6cb8`。
4. 完整 Linux Desktop Release 构建通过，使用 `tauri/custom-protocol`；Cargo 8
   jobs、嵌套构建 1 job。产物 44,392,120 字节，SHA-256 为
   `0282c4638bd0212198153784722a81317f6c6fc6ca834517e488cc068b398f35`。
5. 将产物放入 VM 的独立临时目录，使用独立用户数据与关闭的回环端口运行
   `--session-helper`。启动后保持运行，SIGTERM 后退出码 0；未读取已安装
   服务的凭据。这是 helper 启动检查，没有运行完整 GUI 的连接流程。
6. 同一 Desktop 二进制的 `--media-worker` 在真实 1080p 全屏动画中完成 90 帧
   解码、89 次变化，预热后 13.61 FPS；使用内置 OpenH264 软件路径。
   结束后恢复原 1314×742 分辨率与布局，检查自有临时 native 进程数为 0。
   此数字仅覆盖采集/编解码/本地管道，不含网络和显示。
7. Windows Desktop 的 `cargo check --locked` 通过，覆盖共用 Executor 改动。
   初次检查因本机 CMake 不支持自动选中的 VS 2026 生成器失败，显式选用已安装
   VS 2022 后通过；Cargo 8 jobs、CMake 1 job、MSVC `/MP1`，NASM 3.01。

任务 ID、产物哈希、源码哈希与实测输出见
[结构化证据](remote-desktop-linux-helper-2026-10-17.json)。

## 仍需验收

以上 helper 测试使用临时回环连接，不证明真实远程网络和正式窗口显示可用。
没有注销、重启或更改已安装服务；真实会话切换退出尚未做破坏性实测。
Linux 原生显示、输入、X11 实测、GDM/系统登录和端到端显示性能仍未完成。
普通用户会话接入不代表 greeter/root 登录界面接入，W5 保持未完成。
