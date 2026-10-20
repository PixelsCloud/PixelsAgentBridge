# 路径迁移、实际输入反馈与同连接旧业务

日期：2026-10-20。代码检查点 `652de0d`，复用已冻结产物，没有重新编译或修改产品代码。
Windows 独立正式窗口 → Linux X11，1306×768；使用现有账号与设备授权、可靠通用传输。
本批关闭 **06.5**；不同延迟整形、限速和积压恢复直接复用既有证据，没有重跑已通过条件。
结构化结果及原始文件哈希见[本批 JSON](remote-desktop-path-migration-latency-2026-10-20.json)。

## 自动迁移与画面恢复

在临时 Linux Executor 自有 UDP 源端口上注入丢包，IPv4/IPv6 各两条有界 `tc flower` 规则。
TCP Relay、控制服务和 SSH 保持可用；没有修改其他进程或整台虚拟机的防火墙。
规则命中记录为 282 packets、203 drops、136642 bytes，三项分别记录，不混算为丢包数。

- 已观察 **Direct → Relay → Direct**，没有关闭重开窗口、手动重试或另建终端。
- 迁移时视频进入 reconnecting，输入/首帧就绪门控关闭；恢复的新视频 session 默认只查看。
- Relay 文件下载结束附近又发生一次视频重连，独立观察器约 0.93 秒后的下一次采样已恢复。
  该差值是操作端观察间隔，不能作为精确停顿时长或跨机器故障到恢复耗时。
- 最后回到 Direct 时视频 session 仍为 `1bc98acc-7927-475a-9c65-4dd6d9094619`，
  `frame_ready=true`、`input_enabled=false`，采集端报告 15 FPS，Windows D3D11 硬解。
  本次不是重新测量正常显示帧率；实际显示 >12 FPS 的既有证据继续保留。

视频 session 可以在恢复中更新，不能把新 session 的 generation=1 当作代次倒退。
本批未在断流瞬间按住修饰键；按住输入的释放、恢复只查看复用
[积压/释放证据](remote-desktop-backlog-evidence-reaudit-2026-10-20.md)，不冒充新增实测。

## 点击到可见反馈

在自己的全屏 GTK 夹具中绘制点击计数、互补位和守卫色。Windows 以真实 `SendInput`
向自有远程窗口发送一次原子 move/down/up，读取该进程所属原生画面的解码后像素，直到新计数出现。
起止使用同一操作端单调时钟；包括目标处理、采集/编码、网络、解码/合成和采样开销。
这不是纯网络 RTT，也没有相减 Linux 与 Windows 的墙上时间。

| 条件 | 样本数 | 中位耗时 | 范围 |
| --- | --- | --- | --- |
| 初始 Direct | 5 | 221.8 ms | 179.0–268.5 ms |
| Relay，同时传输 4 MiB 文件 | 5 | 868.0 ms | 211.4–1014.3 ms |
| 撤销网络规则后，仍在 Relay | 5 | 231.6 ms | 210.4–242.5 ms |
| 自动回到 Direct | 5 | 160.6 ms | 101.5–181.3 ms |

有效样本共 20 次，像素计数从 1 连续增加至 20，互补/守卫校验无无效样本。
单次取样最高开销约 40.75 ms；小样本只说明本环境实际反馈，不外推百分位、性能承诺或七方向最终基准。
Relay 同时传文件时有约一秒的反馈，不能把该结果写成与空载直连相同的延迟。
结束时目标 clicks=20、releases=20、left_held=false，恢复后新计数可见，没有停留在旧画面。

## 同一个操作端连接的原有业务

视频、命令、终端、文件均调用同一 Desktop 的 Tauri operator runtime。
独立 MCP 只做启动准备，没有拿另一个 MCP 端点能执行命令来代替受损连接证据。
同一 PTY `478e4e66-cd7b-4cf4-b71d-3e10ed3bafcc` 从初始 Direct 保留到 Relay，再保留到恢复 Direct，
每一阶段均收到新 printf 标记，期间没有重新打开 PTY。命令任务均 Succeeded。

| 阶段 | 原 PTY 新标记 | 命令完成 |
| --- | --- | --- |
| 初始 Direct | 约 894 ms | 约 1285 ms |
| Relay | 约 469 ms | 约 516 ms |
| 撤销规则后 Relay | 约 429 ms | 约 470 ms |
| 返回 Direct | 约 363 ms | 约 362 ms |

Relay 下上传和下载分别 4194304 字节，均 completed，约 15.537 s / 11.194 s。
源文件和下载文件 SHA-256 均为
`d8355986551be103c9e77437cb0c32cd210fe52f2c14ebbac970d8d9cdc2a2c6`。
每个有副作用的操作只提交一次，已经完成的传输没有重放。

## 保留的失败与修正

1. 第一轮点击前没有等 operation-enabled watch 生效，前提不成立；目标没有收到点击。
   随后补显式就绪等待，原失败文件保留。
2. 自有 GTK 夹具把系统合成的双击通知也计入按下数。修正为只计 `BUTTON_PRESS`，
   有效计数重新从 0 开始；没有改产品输入逻辑，也没有将准备轮次计入 20 个有效样本。
3. `business-relay.json` 原脚本最后立即断言首帧就绪，刚好处于视频自动重连，故其 `passed=false` 原样保留。
   其中命令、原 PTY、两次文件传输均已完成；独立时间线及后续新像素、原 PTY 结果证明自动恢复。
   没有为了让脚本变绿重新执行已完成业务。
4. 故障夹具用带 pref 的 `tc -j` 查询做清理验证，返回结构不符导致清理和备用清理均失败。
   后续全量读取规则，核对精确 protocol/pref、源端口和 drop 动作，只删除四条自有规则。
   原 root qdisc 已逐项比对一致。最后停止已到期 timer 报 unit-not-loaded，独立终态检查确认 timer 非活动、规则已撤销。

## 环境与进度

- 原 Linux Executor 已恢复运行；安装二进制、systemd unit、TOML 配置哈希均未变，原 DropInPaths 恢复。
- 自有 fixture、media、watch、rollback 均退出，运行时媒体目录消失；无自有残留进程。
  故障夹具的 failed 单元状态保留作诊断，不是仍活动的网络规则。
- 同一 X11 session 22 仍 active/unlocked。未切换会话、锁屏或改分辨率。
- 自有 Windows Desktop/MCP 退出，临时凭据槽撤销，原凭据槽未变。
  限制 ACL 的本地 profile、Linux root-700 管理状态、媒体产物与测试文件仍保留，不声称已删除所有私有副本。
- 没有更新本机安装版，没有编译、push 或 COS 发布；没有启动暂缓的 2/8 小时测试。

原 06.5 的直连、强制 Relay、150 ms 单向/128 kbit 整形、限速/断流/恢复及积压证据保留：
[解码状态与整形](remote-desktop-decoder-status-2026-10-18.md)、
[限速与 helper](remote-desktop-helper-budget-weak-network-2026-10-19.md)、
[新 helper 同会话恢复](remote-desktop-display-wake-recovery-2026-10-20.md)、
[积压证据核对](remote-desktop-backlog-evidence-reaudit-2026-10-20.md)。
加上本批双向路径迁移、实际输入反馈和同连接旧业务，关闭 06.5。
06.6 仍缺最终候选资源/七方向数据，不能因补了本次输入反馈整条关闭。
下一步按主清单补 Mac/Linux 操作端尚无证据的目标登录组合，已关闭条款和已通过条件不再重做。
