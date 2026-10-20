# Linux：虚拟输入与 Wayland 锁屏登录

日期：2026-10-17；基线 `eb742d4`。新增系统媒体 supervisor 的 uinput 后备，
完成一次真实 GNOME Wayland 熄屏、自动唤醒、密码解锁和桌面采集恢复。
**本轮验收是生产 broker、用户启动器、内嵌采集/输入组件的真实链路，尚未覆盖
正式远程窗口内连续登录、完整 supervisor 服务重启或 GDM/不同用户切换。W5
保持未完成。** 原始结果与编译代码/产物哈希见
[机器可读证据](remote-desktop-linux-uinput-login-2026-10-17.json)。

## 实现

- root supervisor 在既有 helper profile 中新增 `input.sock`，沿用内核 UID、
  PID、进程组及活动图形会话核验。没有新增远程认证或应用配置，也不改变网络
  协议和可靠传输。输入几何和事件只在已有监督关系内传递。
- 每个输入连接创建独立 uinput 键盘和指针。系统进程持有 `/dev/uinput`，普通
  用户采集/输入 worker 不持有它。通过 pidfd 固定合成器身份，等待该合成器
  实际打开新设备后才返回就绪，避免固定延时导致第一个登录按键丢失。
- 指针使用整个逻辑桌面的轴范围，把所选屏幕的归一化坐标转换为该屏幕实际
  原点和尺寸；计算使用宽整数，并校验边界。合成坐标检查包含负原点与不同
  尺寸屏幕，但实际 VM 仍为单屏，不能代替真实混合 DPI/旋转/多屏验收。
- 释放操作、半包 EOF、普通退出和 SIGTERM 释放本连接持有的键/按钮，然后
  销毁设备。设备 fd 在进程崩溃后关闭；实机检查额外验证了 root worker
  SIGKILL 后重开没有遗留修饰键或按钮。物理输入设备不被销毁或重置。
- 正常 `input` 入口在锁屏时选择 uinput；普通 GNOME 输入启动失败时也可后备。
  旧代次在锁状态或布局变化后退出，不重放历史输入。
- 正常 `capture` 入口检测锁屏/熄屏后选择 supervised DMA。熄屏时通过独立
  虚拟键盘发出 Shift 按下/抬起，等待屏幕唤醒并留出首轮重绘时间，然后读取
  当前 scanout；不直接把旧 framebuffer 当作有效锁屏画面。
- GNOME 的 PowerSaveMode `-1` 表示未知/不支持，不当作熄屏，也不使普通
  PipeWire 会话无故改走 DMA。DMA 仍要求已确认亮屏，避免回传旧扫描图像。
  该语义来自 [Mutter 接口定义](https://raw.githubusercontent.com/GNOME/mutter/main/data/dbus-interfaces/org.gnome.Mutter.DisplayConfig.xml)。

参考本地 RustDesk `7aa98d43cf1962a7a29ec16ffef42974377ef11e` 的
`src/server/uinput.rs`，采用独立虚拟键盘/鼠标及全桌面绝对轴的思路。这里沿用
本项目的会话监督、8 字节输入协议及坐标模型，没有引入 RustDesk 的传输或
鉴权。设备建立与销毁使用 [Linux uinput 接口](https://docs.kernel.org/input/uinput.html)。

## 编译与实机检查

最终 Desktop SHA-256：
`8a864bfd02ea9e39679c7a079f7ecd3925a4eca15bf1e9ca77aea20a041834b8`。
Linux Release 构建使用容器 8 CPU、Cargo 8 jobs、嵌套 CMake 1 job，编译命令
顺序执行；原生几何检查最多另用 1 CPU，总数不超过 10。构建快照以内容变化
更新文件时间，最终源码字节与快照逐项核对。Rust 按项目 edition 2024 格式检查
通过，图形环境白名单单元检查通过。

设备为 Ubuntu 22.04.5、GNOME Wayland、vmwgfx、2 vCPU、UID 1000、会话 23、
单屏 1280×800。隔离 systemd 包装器调用生产 `PreparedUser` 和 broker，使用
正式 Desktop 二进制内嵌的 worker。安装版、账号和 TOML 配置未替换。

| 检查 | 结果 |
| --- | --- |
| 坐标与键鼠 | 选屏绝对点击、Shift/Ctrl 组合、拖拽、双向滚轮通过 |
| 输入释放 | ReleaseAll、半包 EOF、用户 worker 崩溃、root worker SIGKILL、错误操作包均可释放并重开 |
| 布局变化 | 陈旧几何拒绝；按住 Ctrl/按钮时切换分辨率，旧输入退出，恢复分辨率后重开通过 |
| 原生几何边界 | ASan/UBSan、负坐标与无效边界检查通过；不是实机多屏结论 |
| helper 选择 | 已认证 supervised helper 优先，退出后回退 GUI helper 的实机 IPC 测试通过 |
| 锁屏唤醒 | PowerSaveMode=3 时正常 capture 自动唤醒到 0，仍处于锁屏状态 |
| 锁屏画面 | 通过软件解码，锁屏/密码框各与独立 VMware 控制台截图比较；平均亮度误差约 4.70/255，差值大于 30 的比例小于 0.000001 |
| 系统密码解锁 | 正常 input 入口完成一次凭据提交，LockedHint 从 yes 变 no；独立恢复定时器未触发，也没有调用恢复解锁 |
| 代次退出与恢复 | 解锁后旧输入退出；旧锁屏采集请求返回 0 字节并退出，新采集使用 GNOME PipeWire |
| 登录后动态回归 | 两次 DMA：14.926、14.826 FPS；PipeWire：13.664 FPS；每次 90 帧 |
| 同最终产物完整输入回归 | 11 项输入检查通过；两次 DMA：14.173、15.006 FPS；PipeWire：13.574 FPS |

以上 FPS 是 VM 内存中的采集/编码/解码速度，不含网络与操作端显示。锁屏页面
是静态页面，未把静态重复帧算成性能验收。密码框截图在输入凭据之前生成，
像素仅保存在本机忽略目录；输入正文没有写入日志或提交。

## 失败记录与恢复

- 首轮 `uinput-84a45d7590fb` 在 GTK 全屏尺寸就绪前断言失败，尚未开始输入。
  修正测试窗口先显示再全屏，并等待实际分配尺寸；保留原失败报告，不删改门槛。
- 中间产物 `uinput-21a1f5c35c5b` 的 11 项输入检查通过；随后处理未知电源状态
  的分支并重新编译，最终结论使用 `uinput-be7f40732459` 和
  `lock-uinput-fc2597ebd801` 的相同产物。
- 一次锁屏测试上传因 SFTP 文件打开模式错误在启动测试前失败，空凭据文件和
  上传目录已清理；当时没有启动服务或锁屏。修正为排他写入后才运行上述登录验收。
- 输入故障测试的 `Invalid native input operation` 是刻意发送错误包得到的
  诊断，随后释放与重开均通过，不把它隐藏成无错误日志。
- 登录结束前后会话、安装版 Executor、配置和服务单元哈希一致；测试进程、
  虚拟输入设备、socket、私有 profile、传输文件均清理，原服务保持 active。

## 后续必需项

继续验证完整 `--media-supervisor`、正式独立远程窗口的连续锁屏登录和自动
恢复，以及 GDM Wayland、注销/切换会话、真实多屏、其他驱动和长时间运行。
现有 X11/GDM 记录单独保留；本次 GNOME 用户会话解锁不替代开机登录或其他桌面环境。
