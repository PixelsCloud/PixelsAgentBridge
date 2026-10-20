# Wayland GDM 采集入口与会话类型

日期：2026-10-17，基线 `5b91419`，Ubuntu 22.04.5 / GNOME 42.9 / VMware。
源码、构建快照、产物和运行结果绑定见
[采集记录](remote-desktop-linux-gdm-wayland-capture-2026-10-17.json)；
启动诊断及失败观察见 [诊断记录](remote-desktop-linux-gdm-wayland-diagnosis-2026-10-17.json)。

## 实际缺口

Wayland GDM（UID 128、logind class=greeter）就绪后，DisplayConfig 可以枚举屏幕、
查询电源，SessionManager 可用，但 `org.gnome.ScreenSaver` 没有 owner。原代码
在判断锁屏时直接调用该接口，无法到达 GDM 的 DMA 采集和监督输入路径。

首次探测过早：合成器还未取得 D-Bus 名称，出现 NoReply/ServiceUnknown；这些
结果不能用于判定接口支持。该次恢复检查也过早结束，后续 Xorg greeter 再次
激活，已另行恢复并连续检查 15 秒。修订探测等待 DisplayConfig 就绪，禁止
D-Bus 自动启动被查询服务；得到上述接口差异。第二轮清理也记录了 GDM 后续
创建的 greeter，由有界恢复流程清理。最终采集轮没有出现后续 greeter。

## 变更

系统媒体启动器从已经核实的 logind 会话写入 `XDG_SESSION_CLASS`；不会从被观察
进程复制这个字段。现有环境选择测试加入伪造 class，确认它被过滤。

Wayland worker 将 greeter 与已锁屏桌面共同视为受保护画面，直接选择既有监督
DMA/uinput 通道，并在采集存活期间持有 idle 抑制。普通用户仍查询原 ScreenSaver
状态。该字段只决定采集/输入后端，root broker 原有 peer、活动会话和合成器
身份核实保持；远程认证、可靠传输、单查看者逻辑均未修改。

## 验收范围

完整 Desktop、Executor、broker 测试启动器和 Executor 测试产物顺序构建通过，
Cargo 8 jobs，嵌套构建 1 job；会话环境测试通过。候选 Desktop SHA-256 为
`de544ef367034fff987037c09fa47fcadfa2870a30025a321e184490aa096d5b`。

在真实 Wayland GDM 会话 c11 内，以正式用户启动器和 broker 运行三轮采集：

- 前两轮显式 DMA，第三轮正常 capture 入口，均由 greeter UID 128 进行转换和编码。
- 每轮解码 30 帧，1280×800，末帧抽样亮度范围分别 43～241、38～241、38～241。
- 每轮采集持有一个 Pixels idle cookie，正常关闭后 cookie 释放；broker 无错误，
  会话保持，helper、socket、采集及导出进程正常退出。
- 原用户会话 23 恢复，后续 15 秒全部维持原活动会话；恢复计时器未触发，原服务
  active，GDM/应用配置哈希不变，临时目录和 runtime 清理完成。

`verify_linux_greeter.py` 不创建普通用户窗口、不输入密码、不保留画面。非空白
解码只是组件证据，尚不能代替独立控制台对比、正式窗口显示或真实 GDM 密码登录。
该阶段也未实际验证新增 greeter 输入分支；下一步须在正式独立窗口确认当前登录
画面、输入密码和返回用户桌面的会话交接，之后还要验收注销后的全新登录。

同一候选的用户回归完成 35 秒锁屏采集、单次密码解锁、idle 释放和三轮普通动态
采集，但 broker 记录一次「连接进程不属于受监督 helper」，因此**整轮回归失败**。
结果完整保留，不用子项成功覆盖总体失败；后续需诊断进程取消与排队接收的时序。
用户锁屏回归不能作为 GDM 登录通过证据。W5/W7/W8 保持未完成。

后续独立取消测试定位到已回收客户端的 getpgid 返回 ESRCH，修复及真实会话回归
见 [broker 取消记录](remote-desktop-linux-broker-cancel-2026-10-17.md)。本文件保留
当时的整体回归失败，完整候选的正式 GDM 登录仍需继续。
