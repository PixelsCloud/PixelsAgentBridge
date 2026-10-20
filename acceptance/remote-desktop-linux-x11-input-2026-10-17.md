# Linux X11 输入与实际 GDM 画面

日期：2026-10-17，基线 `5fedf71`。在 X11 采集后端上接入原生输入和进程崩溃后
的释放恢复。结果与源文件哈希见[机器记录](remote-desktop-linux-x11-input-2026-10-17.json)。

## 实现

输入复用现有独立 worker 管道、Rust `NativeInput` 退出监督和 `recover-input`。
XTEST 注入物理键和按钮；鼠标以所选输出的物理尺寸、原点和 root window 定位。
真实双 X screen 测试发现 XTEST motion 可能仍停留原 screen，因此使用明确指定
目标 root 的 `XWarpPointer`。未新增远程认证或修改可靠传输。

按下前记录待释放标志，X server 处理释放后清除。记录位于当前用户私有 runtime
目录，以本地 X server 的内核 peer PID 和进程启动时间区分生命周期；独占文件锁
使存活的输入 worker 与恢复操作互斥。261 字节只包含键/按钮的未释放状态，不保存
输入正文、历史或密码。这是运行状态，不是应用配置。布局变更和异常输入结束旧
worker；正常 EOF、半包 EOF、错误包和 SIGKILL 均纳入恢复测试。

## 隔离测试

- 地址及未定义行为 sanitizer：原有 254 项和新增后共 68 项原生检查通过。
- 正式 Release worker 在 1920×1080、640×480 两个 X screen 的实际 X 事件验证：
  中心与边缘定位、一次按下/释放、Ctrl+A、滚轮余量累积、ReleaseAll，以及上述
  四种退出恢复；并发恢复不能释放存活 worker 的键。结束后 261 个状态字节全零。
- 同一 worker 采集/解码分别约 15.018 和 14.994 FPS；颜色变化、静帧等待和共享
  内存回收检查通过。这里没有网络或正式观看窗口，不能作为端到端帧率结论。
- Linux 媒体库 12 项 Rust 测试通过。Cargo 8 jobs、嵌套编译 1 job；原生编译串行。
  Python 测试显式调用生产恢复命令，尚未在 X11 上验收 Rust/helper 自动恢复全链路。

## 真实虚拟机

已有 SSH 凭据只从本地主工作区忽略文件读取，不复制到代码、日志或命令参数。
用户已授权锁屏、注销和切换会话。本次保留原 hy/Wayland 会话 13，短暂切换 GDM。
每次切换前启动独立 systemd 恢复计时器。

Wayland GDM（会话 c4，UID 128）使用与普通用户管理器不同的私有 D-Bus。
初次连接标准用户总线出现 ServiceUnknown，修正为从内核会话对应的 gnome-shell
进程读取所需环境后，枚举成功；采集仍返回 `Session creation inhibited`，零媒体
字节。这是与普通桌面锁屏相同的实际缺口，不能靠重复枚举解决。

其后观察到活动 GDM 为 Xorg（c5），直接使用该现有会话测试，没有写入临时 GDM
设置。正式 worker 枚举 `x11:0:Virtual1`，采集并解码 1314×742 首帧，编码 7,227
字节，解码 1,462,482 字节；抽样亮度范围 44～240，非空白画面。像素未落盘。
鼠标移动、Shift 按下/释放和 ReleaseAll 共四个输入 ACK 成功，恢复退出码 0。
这些 ACK **尚不是独立 OS 状态或密码登录验收**。

随后恢复原会话 Active=yes、LockedHint=no，停止并清理独立恢复计时器及临时
恢复脚本。二次 SSH 检查确认原会话仍活动、临时 greeter 已退出、原 Executor
服务 active。GDM 系统配置哈希不变，无 runtime override；未修改账号、密码、
自动登录或本机已安装应用。

参考 GDM 42 的[显示会话创建](https://github.com/GNOME/gdm/blob/42.0/daemon/gdm-local-display-factory.c)
与[SIGHUP 设置重载](https://github.com/GNOME/gdm/blob/42.0/daemon/main.c)，已准备可撤销的
Xorg 临时测试路径；本轮实际复用了现有 Xorg greeter，没有执行该配置变更。

## 尚未完成

Linux greeter helper 自动监督/注册、会话交接、正式独立窗口登录、实际 Xorg 用户
桌面、Wayland 独立锁屏媒体、真实多显示器和变换布局仍需完成。当前结果不关闭
W5、W7 或 W8，也不作为安装包已支持 Linux 系统登录的发布声明。
