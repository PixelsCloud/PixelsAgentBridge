# E3 指定用户终端阶段记录

## 本次增量

- 复用 portable-pty 0.9；用户 PTY 由既有 Executor 内部 user-worker 创建，默认 service 行为保留。没有引入另一个常驻 Host。
- `pab_open_terminal` 增加可选 execution 参数，使用当前连接发现的 user/context_ref；先协商 terminal v2，再打开。旧设备不支持时明确拒绝。
- 开启时原生复核身份；响应返回实际身份，Executor SQLite 事务记录 selection、identity、连接和初始尺寸。历史记录不伪造身份。
- 终端会话固定身份并绑定网络连接。重复打开同一请求与参数返回原会话；变更选择冲突。另一连接即使 actor 相同也不可输入。
- 本地 IPC 控制消息与 PTY 二进制分开；有界队列串行处理操作，调用方取消等待不会破坏半帧读取。
- 网络连接 guard 独立于长命令任务生命周期；断开会关闭该连接的 PTY。已接受的普通命令继续按原有规则运行。
- 明确关闭后等待 worker 退出；重复关闭内部会话幂等。丢失操作结果记录为 unconfirmed/interrupted，不重放输入或自动另开会话。
- 修复原 TerminalSession 析构对已经回收的子进程仍发送 kill 的问题，避免 Unix PID 复用后误发信号。

## 已验证

正式 Pixels MCP 用于传输源码/测试程序、远端编译、运行与查询；实际新 execution 参数由源码集成 fixture 验证，尚未安装到当前 MCP 宿主。

| 环境 | 结果 |
|---|---|
| Windows90 SYSTEM → WTS1 Administrator | 命令和用户 PTY 各1项原生集成通过 |
| Windows90 SYSTEM → WTS2普通账户 | 同上；未切换用户桌面 |
| macOS root → UID501 | 命令和用户 PTY 各1项通过 |
| Linux root → UID23001，无GUI/logind | 命令和用户 PTY 各1项通过 |

PTY 集成验证：真实 SID/UID、中文计算结果、resize、接受时身份入库、重复打开、同ID改选择冲突、跨连接拒绝、重复关闭、关闭后输入失败、连接 teardown 中断原记录并拒绝新会话。

原始任务：Windows `b1d42b3f-164b-48b3-9795-5e2db5fe80e1`；Mac `0fc6af10-1ade-4d96-86c1-f9530122c40b`；Linux容器原测试进程退出0。

独立回归：无上下文不创建service终端、丢回复保持未确认记录通过；MCP终端参数拒绝伪造用户名/额外字段/desktop模式通过。
portable-pty封装测试 Windows7项、Mac5项、Linux5项通过，包含自然退出后重复close/drop；Mac回归任务 `af80096b-6156-49fa-bd9e-a7febdcd4c4b`。
Workspace tests 编译、Desktop Rust tests 编译通过。最终操作错误分类调整另由本地回归覆盖，未重复整套远端fixture。

## 剩余边界

- 仍需 E6 完整 QUIC/MCP 安装后路径验收、父进程崩溃、开始时取消、长时间输入阻塞、shell后台/脱离session子进程及资源残留检查。
- 用户记录目前写入 Executor，并随打开响应返回；Bridge归档/UI身份展示在E5补齐。
- Git、文件、传输尚未接入指定用户，应用发现/启动/打开文件及最小UI仍待完成。
- 未打包安装或增加产品版本；本阶段不代表完整E3/E8交付。
