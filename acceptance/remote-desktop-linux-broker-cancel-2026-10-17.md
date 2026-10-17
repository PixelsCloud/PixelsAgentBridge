# Linux broker 排队连接的取消

日期：2026-10-17。代码、构建启动器、实机运行和清理结果见
[机器记录](remote-desktop-linux-broker-cancel-2026-10-17.json)。

GDM 后续的用户锁屏回归中，输入、密码解锁和动态采集子项成功，但 broker 记录
一次 `media broker peer is outside the supervised helper`，整轮因此失败。
没有把这条错误忽略或改成测试允许项。

使用同一原生候选，在真实用户会话中连续启动 12 个采集进程，每个读完本地协议
头后立即 SIGKILL 并回收，尚未请求图像帧。对 root broker 的 getpgid/getsockopt
系统调用做有界跟踪，6 次 getpgid 返回 ESRCH；六个 PID 都属于已结束的采集进程，
恰好对应 6 条 broker 身份错误。没有跟踪 read/write、输入正文或密码。

原因是本地连接已经排队，客户端却在 broker 轮询接收前结束。修复在验证 socket
凭据的 PID/UID 后单独处理 getpgid：ESRCH 直接丢弃连接并返回，不启动 root worker。
存活进程的进程组、活动会话和合成器校验全部保留；其他系统调用错误也继续返回。

修复后的生产 broker 源码编入真实会话测试启动器，以 8 jobs 单独构建。相同
12 次取消后的 broker 错误为零，随后两轮 DMA 与一轮 PipeWire 动画采集均通过：
各 90 帧、89 个变化样本，分别约 15.019、14.940、13.998 FPS。helper、socket、
导出进程和临时目录均回收，用户会话保持不变。

跟踪基线还在第一轮后续 DMA 动画采集发生 `DRM framebuffer changed`，导致无效
描述符包并失败。这条结果保留；不能凭后续未跟踪运行成功就断言它由 ptrace
导致。原生导出已有有限次数刷新重试，是否还需要调整须由进一步证据决定。

本次证明 root broker 的取消分类及后续采集恢复；当时修复尚未重建到 Desktop
supervisor，完整候选的正式窗口与 GDM 密码登录验收继续进行，不作为发布完成。
