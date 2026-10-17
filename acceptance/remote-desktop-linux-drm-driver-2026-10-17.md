# Linux Wayland 登录画面：VMware 驱动探测

日期：2026-10-17。基线：`cf1988c`。这是隔离驱动实验及环境恢复记录，
**默认 Wayland 锁屏/GDM 采集仍未通过**，没有将实验接入正式产品后备路径。

## 观察结果

Ubuntu 22.04 虚拟机，VMware SVGA II / vmwgfx，3D 加速保持原设置。
探测独立打开 DRM 节点，没有申请 DRM master、改变显示模式或借用合成器句柄。

| 环境 | 主画面 GETFB2 | PRIME 导出 | 只读映射 | VMW 零区域 readback |
| --- | --- | --- | --- | --- |
| 原运行内核 6.8.0-40，用户桌面 | ENODEV (19) | 无可导出主画面句柄 | 未执行 | EACCES (13) |
| 已安装默认内核 6.8.0-138，Wayland GDM | 成功，1280×800 XRGB8888 | ENOENT (2) | 成功但 RGB 全零 | EACCES (13) |

新版内核的主平面 pitch 为 5120、modifier 为 linear。读取完整画面得到
`nonzero_channels=0`、RGB FNV-1a `dff2e40b0a1ba325`，与实际登录界面不符。
光标 64×64 平面可导出；这不能证明主画面可采集。`DRM_CAP_PRIME=3` 也不能
替代对实际主平面的导出验证。零区域 readback 只检查调用权限，不执行像素传输。

参考本地 RustDesk 的 DRM/libdrmtap 路径和 Linux v6.8 vmwgfx 源码：该版本的
surface framebuffer 与 BO framebuffer 使用不同函数表；readback ioctl 需要
master/auth。源码用于解释探测方向，不用上游 v6.8 代码冒充 Ubuntu 两个内核的
完整补丁集，也不据此断言黑帧的最终根因。
参考：[Linux vmwgfx 驱动](https://kernel.googlesource.com/pub/scm/linux/kernel/git/torvalds/linux/+/refs/tags/v6.8/drivers/gpu/drm/vmwgfx/vmwgfx_drv.c)、
[framebuffer 实现](https://kernel.googlesource.com/pub/scm/linux/kernel/git/torvalds/linux/+/refs/tags/v6.8/drivers/gpu/drm/vmwgfx/vmwgfx_kms.c)。

## 实验程序和构建

`experiments/remote-desktop/linux_drm_inventory.c` 新增 vmwgfx 零区域权限检查和
MAP_DUMB 偏移查询；inventory 本身不读取画面。
`experiments/remote-desktop/linux_drm_mapped_probe.c` 是单平面只读像素验证程序：
限制 XRGB8888 linear、8192 边长和 64 MiB 映射，独占创建 0600 PPM 文件，
完成后释放映射及自身 GEM 句柄。它输出像素统计，不把 ioctl 成功当成采集成功。

两者分别在现有 Linux 构建容器中以单个 C 编译进程、容器 1 CPU 构建，
`-O2 -Wall -Wextra -Werror -I/usr/include/libdrm -ldrm` 通过，无并行编译。
二进制哈希及原始命令任务编号见相邻 JSON。像素文件和私人测试工具不加入仓库。

## 重启与恢复

按照已有虚拟机重启授权，确认 6.8.0-138 的内核、initrd 和默认 GRUB 项均已存在
后执行一次普通重启。没有安装内核、修改 GRUB、删除旧内核或更改虚拟硬件。
重启后的安装版 Executor 正常启动，设备配置和安装二进制哈希保持不变。

尝试临时创建 Xorg GDM 作为恢复路径未成功，记录为失败；删除临时 GDM 偏好并
重启显示管理器后，默认 Wayland greeter c3 正常出现。当时无图形用户会话。
通过独立 VMware VIX 控制台截图确认 hy 用户及密码输入框，再用临时 root uinput
键盘提交一次既有系统凭据。密码只经内存及加密 SSH stdin 传递，没有进入命令
参数、测试源码、日志或仓库。VIX 仅用于独立观察，不是产品采集后备。

登录后 logind 确认新会话 23：UID 1000、Wayland、Class=user、Active=yes、
LockedHint=no。临时键盘销毁；GDM 偏好、恢复目录和定时器已清理；隔离媒体
进程为零。原服务 active、安装版二进制和 TOML 哈希再次核对通过。
该结果证明虚拟机已恢复，不算正式远程桌面窗口的注销/新登录验收。

W5 仍需解决可交付的 Wayland 登录采集与输入路径，并重新完成产品窗口验收；
新内核也需要普通桌面回归。其他工作包继续按长期任务推进。
