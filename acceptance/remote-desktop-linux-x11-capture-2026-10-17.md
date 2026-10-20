# Linux X11 采集与锁屏缺口

日期：2026-10-17，基线 `ba2330d`。本阶段增加生产 X11 采集后端，验证独立 X
服务器中的实际像素、解码与生命周期，并实测 GNOME Wayland 锁屏后的失败原因。
不将这些结果计为系统登录完成。产物、源文件哈希和远端任务编号见
[机器可读记录](remote-desktop-linux-x11-capture-2026-10-17.json)。

## 实现

- 通过会话类型区分 X11 与 Wayland，避免把 XWayland 的兼容画面当作完整桌面。
  Wayland 继续使用原 GNOME/PipeWire 路径。
- RandR 枚举活动输出，标识包含 X screen 和输出名称；按选定输出在 root
  framebuffer 中的物理范围采集，已旋转的像素不再重复旋转。布局变化使旧采集
  退出；额外的 RandR 仿射变换暂时明确报错，避免使用错误裁剪/坐标。
- MIT-SHM 映射直接交给现有软件编码器，必要的 BGRx→I420 转换沿用原实现。
  不额外复制整帧。光标位置、热点和预乘透明度在该映射内合成。
- Damage 事件和光标变化驱动更新，静止画面不主动重复编码。共享内存仅本用户
  可读写，X server 成功附加后立即标记删除；退出、管道结束和被杀都可回收。
- 构建和 Debian 安装依赖显式列出 X11/Xext/RandR/XFixes/Damage。系统依赖文件
  保留其规定格式，没有修改 TOML 配置、认证或远程传输协议。

## 验证

使用独立 Docker 容器中的 Xvfb，不连接用户实际 X 服务器。这里的两个 X screen
是 1920×1080 和 640×480，不能冒充 XRServer 的四块显示器或真实多屏热插拔。

1. 地址与未定义行为 sanitizer 下，原有 254 项原生边界检查、新增 63 项检查
   通过。新增检查包含会话选择、格式/stride、光标边界、输出标识、旧布局拒绝，
   以及两个实际 X screen 共 40 次共享图像创建、读回和回收。
2. 正式 Release worker 在两个 X screen 分别解码 47 帧，44 次连续颜色变化
   均正确，采集加解码约 15 FPS。静帧请求等待变化；正常关闭、SIGKILL 和父管道
   EOF 后，`/proc/sysvipc/shm` 均恢复测试前集合。
3. Linux 媒体库 12 项 Rust 测试通过。Cargo 最大 8 jobs、嵌套 1 job；原生测试
   编译串行。未编译或替换本机已安装 Desktop。
4. 同一 worker 上传到 Linux VM 的隔离目录，原 GNOME Wayland 1314×742 动画
   回归完成 90 帧解码、89 次变化，约 13.797 FPS。测试后临时 native 进程为 0。

上述采集/解码数字不含网络与正式窗口显示；此前那组 1080p 网络显示结果保留在
[独立记录](remote-desktop-linux-presentation-2026-10-17.md)，不转移为 X11 产品验收。

## 实际锁屏观察

用户明确允许 Linux 锁屏、注销与切换会话。本次只执行锁屏：先启动独立的
systemd 45 秒恢复计时器，再锁定活动 Wayland 会话 13。logind 确认 LockedHint=yes
后，屏幕枚举仍成功，但采集创建会话返回 `AccessDenied: Session creation inhibited`，
退出码 1，未输出媒体字节。

最终通过系统管理接口恢复原解锁状态，核实原会话 Active=yes、LockedHint=no，
停止恢复计时器。此处系统管理接口只用于测试恢复，**不是产品远程输入登录成功**。
原服务、账号、自动登录和显示管理器配置未改动。

这条实证说明普通 GNOME 采集不能解决当前虚拟机的锁屏需求。Mutter 本身提供
[禁止远程访问的控制接口](https://gnome.pages.gitlab.gnome.org/mutter/meta/class.RemoteAccessController.html)；
后续必须接入独立的锁屏/登录媒体路径，不能仅放宽 helper 注册或持续重试。
本地 RustDesk 的 `platform/linux.rs` 和 `server/wayland.rs` 提供了会话路由与
DRM 路径分离的参考，本阶段没有复制其代码。

## 继续工作

X11 原生输入及异常退出释放、greeter helper 监督与切换、实际 Xorg/GDM 画面、
Wayland 锁屏/登录和 RandR 变换仍未完成。当前 X11 worker 的 input 操作明确返回
未实现错误；这份阶段产物尚不能作为可交付的 X11 远控或系统登录功能发布。
W5、W7、W8 保持未完成，后续继续使用已获授权的虚拟机验证。
