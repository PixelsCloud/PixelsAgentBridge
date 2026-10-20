# 远程桌面 P2：可靠传输与会话基础验收

日期：2026-10-16。分支：`feat/remote-desktop-v1`。

## 结果与边界

Windows 本机完成 111 项测试，0 失败；另有 5 项既有真实 PTY 测试按原有
`#[ignore]` 保持跳过。这次验证的是会话、可靠分帧及 Bridge/Executor 接入，
不是跨机器原生视频性能验收。

新增 `pab-remote-desktop`，通过现有已认证连接建立控制与视频独立可靠流。
Executor 没有注册媒体后端时不声明能力；旧设备在 Bridge 能力探测阶段得到
不支持提示。生产启动路径尚未注册 Windows 媒体后端，因此本提交不会让安装版
提前出现一个不可工作的远程桌面入口。

测试使用真实本地 Iroh/QUIC 连接，但媒体负载为确定性测试字节，不是 H.264。
认证上下文由夹具提供，没有重复验收设备密码认证。本轮没有更新安装版、注册
测试设备、修改服务端、重启测试机或操作远端桌面。

## 测试记录

先一次编译相关测试目标，再依次运行测试二进制，测试线程数为 2：

```text
cargo test --no-run -p pab-protocol -p pab-remote-desktop -p pab-executor -p pab-bridge -j 8 --message-format=json
```

构建进程限制在 10 个逻辑处理器内，Cargo jobs 为 8，嵌套 CMake jobs 为 1。
发现其他构建活动时推迟启动。复用本机 Cargo 缓存，不替换安装目录的程序。

| 测试目标 / 过滤器 | 通过 | 忽略 |
| --- | ---: | ---: |
| protocol 单元测试 | 66 | 0 |
| protocol / target_context | 3 | 0 |
| remote-desktop 单元测试 | 3 | 0 |
| remote-desktop / reliable_sessions | 6 | 0 |
| bridge / connection:: | 19 | 0 |
| executor / remote_desktop | 2 | 0 |
| executor / task_service::desktop_terminal::tests | 3 | 4 |
| executor / task_service::terminal::tests | 4 | 1 |
| executor / task_service::transfer_tests | 5 | 0 |

原始汇总见同目录 `remote-desktop-reliable-2026-10-16.json`。既有忽略项需要显式配置
可销毁的用户 PTY worker，本轮没有把它们计为通过。整个仓库及所有平台的完整测试
不在本记录覆盖范围。

## 已验证行为

- 512 KiB + 13 字节帧通过 64 KiB 有序块完整传输；通用传输原有 256 KiB 上限不变。
- 在途四帧未确认时停止向媒体源索取新帧；同连接旧请求继续响应，累计确认后恢复。
- 切屏先释放旧源，再启动新 generation；新首帧必须标记关键帧。负坐标、DPI、
  旋转元数据保留，旧 generation 和旧显示确认被拒绝。
- 20 个并发申请只有一个成功；同账号另一条连接也不能占用或关闭已有查看器。
- 连接中断、控制流结束、账号上下文改变和租约到期清理占用；旧任务清理不误删新会话。
- 半帧超时或取消后当前视频 reader 不可复用，避免把剩余负载解释成下一帧头。
- 超长负载、错误 generation、错误序号和缺少首关键帧在读入负载前被拒绝。
- 旧设备能力兼容、未注册后端、注册测试后端后的实际 TaskService 分派均通过。
- 既有 Bridge 终端、截图响应校验、文件能力协商及传输测试，以及 Executor 终端/
  文件传输的上述非忽略用例通过。并行旧请求测试不等于受限带宽下的吞吐保证。

## 下一步

将 P1 Windows 直接纹理编码与硬解显示适配到新媒体接口，打通本机与 90 的真实
跨机器视频；然后验证强制 Relay、延迟/受限带宽、积压重建和真实多屏切换。
独立窗口入口、鼠标键盘、锁屏登录、macOS/Linux 媒体后端和 AI 人工协助仍待后续实现。
本轮没有新增应用配置或改变现有认证方式。
