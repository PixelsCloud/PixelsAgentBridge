# Linux 锁屏静置与正式窗口登录

日期：2026-10-17。基线 `9f5f32b`；实际候选源码与产物哈希、完整采样和恢复记录见
[机器记录](remote-desktop-linux-lock-idle-2026-10-17.json)。Ubuntu 22.04.5、GNOME
Wayland、VMware vmwgfx、用户会话 23、单屏 1280×800。本轮不覆盖 GDM 或注销后的新会话。

## 原因与修复

前一轮正式窗口在锁屏等待时反复更换媒体会话并退回只查看。这次先用旧产物复现：
正常采集入口自动唤醒锁屏后，无输入等待到 26.926 秒，显示电源状态变为 3；
已解码 403 帧后采集明确退出 `Display is asleep; refusing stale scanout`。
测试恢复程序解锁，未提交登录密码。这证明该复现中的退出由再次熄屏触发。

参考本地 RustDesk `7aa98d43cf1962a7a29ec16ffef42974377ef11e` 的 Linux
`SessionIdleInhibit`，采用 GNOME SessionManager 的 idle flag 8。只有已经锁屏的
DMA 采集持有抑制；正常桌面不会因此失去自动锁屏。正常结束调用 Uninhibit，采集
进程被杀或总线断开也会由系统释放。保持原有锁状态、布局和电源变化检查，真实
状态变化仍结束旧代次。没有周期性发送假按键，也没有修改持久系统设置。

依据 [GNOME SessionManager 接口](https://raw.githubusercontent.com/GNOME/gnome-session/gnome-42/gnome-session/org.gnome.SessionManager.xml)
和 [Mutter idle monitor 实现](https://raw.githubusercontent.com/GNOME/mutter/gnome-42/src/backends/meta-idle-monitor.c)，
idle 抑制会暂停空闲计时监视；调用方断开时 cookie 释放。实现没有请求禁止注销、
用户切换或系统休眠的其他 flags。

旧二进制加测试端抑制的对照实验保持唤醒 35.024 秒、解码 498 帧，并成功一次
密码解锁；但之后第二轮普通 DMA 回归出现 `Get: Timeout was reached`，整体失败。
该失败保留，不能把局部结果当整轮通过，也没有证据将超时归因为同时构建。

## 候选实机结果

完整 Linux Desktop、Executor 和测试产物顺序编译通过，Cargo 8 jobs、嵌套构建
1 job。图形会话环境选择测试通过；编译快照与候选原生源文件逐字节相同。

- 锁屏正常入口自动唤醒，35 秒持续解码 526 帧，36 次采样电源状态全部为 0，
  会话一直锁定，采集没有退出。静态锁屏帧数不作为动态 FPS 成绩。
- 正常关闭、SIGKILL、密码解锁后三种场景的 idle cookie 均回收；普通 PipeWire
  采集没有持有 idle cookie。
- 独立 VMware 控制台与解码锁屏、密码框画面对比通过。密码仅提交一次；系统
  LockedHint 在恢复程序运行前变为 no，恢复计时器未触发，也不需要恢复解锁。
- 解锁退出旧采集代次，普通 PipeWire 恢复；随后两轮 DMA 动态采集约 14.987、
  15.042 FPS，PipeWire 约 13.679 FPS。每轮 90 帧、89 个变化样本，均通过。

正式 Windows 独立窗口使用既有认证和可靠传输连接同一候选。普通桌面中心点击
收到恰好一次按下/抬起，坐标误差每轴小于 1 像素。锁屏后原窗口自动恢复画面，
回到只查看；手动启用操作后连续观察 74.206 秒、15 次采样，媒体 session ID 和
采集 PID 完全相同，屏幕持续唤醒、输入持续启用，只有一个 idle cookie。

首次在点击启用操作后立即发送 Enter，没有看到密码框，未据此发送密码。等待
观察完成后另一次 Enter 打开密码框，目视确认后仅提交一次密码。原窗口自动返回
原测试桌面并回到只查看；实际画面比较各 RGB 通道平均差小于 0.001/255。
已在任何恢复动作前确认 LockedHint=no、媒体服务仍 active、回滚未触发。

最后关闭测试窗口并恢复原服务。原安装文件、配置、服务单元保持，临时服务覆盖、
媒体目录、输入设备、测试进程、远端目录、本机隔离 profile 及凭据副本全部清理。
截图仅保留在 gitignored 本地目录，公开记录只保存哈希和量化结果。

本项关闭前一轮观察到的锁屏空闲重连问题；74 秒不替代长期稳定性。W5 的 Wayland
GDM 登录、注销后新会话、安装升级仍未完成，其他平台、多屏和 W7/W8 也保持原要求。
