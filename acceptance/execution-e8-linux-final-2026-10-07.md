# Linux headless 最终安装验收

日期：2026-10-07；Debian 12 x86_64，设备 565893930，最终安装 1.2.39。
正式宿主仍是原生 Pixels MCP 1.2.32。源码修复提交 `40f48f2`。

## 修复

Linux 不提供桌面产品。此前窗口请求被转给不存在的 helper，错误不够明确；
直接返回远端 Unsupported 又会让旧 Bridge 把已接受的系统查询保留为 unconfirmed。
现在窗口/显示器查询按原 ID 接受、持久化 failed 回执，并携带 unsupported_platform。
重复查询只返回原结果，不重新派发。截图及旧输入在派发 helper 前直接返回 Unsupported。
没有为 Linux 增加 GUI 依赖，也没有要求用户更换 MCP 会话来识别结果。

## 测试与安装

- Linux Executor 普通库回归：153 passed、0 failed、7 ignored；忽略项保留原生环境门槛，
  不算通过。新增 headless failed/重复请求用例：1 passed。
- 五个要求桌面产品的合成 QUIC/IPC 用例限定 Windows/macOS；没有移除支持平台覆盖。
  在最终源码 `40f48f2` 的 Mac ARM 上重新运行 local_ipc::tests：22 passed、0 failed，
  包括这些截图、输入、UI 去重/连接隔离用例。原生任务
  `c16d7537-5c08-421e-9f10-ada8dd0d83f7`，退出 0。
- 上述 Linux 全套测试在 1.2.38 阶段执行；其后补充持久化失败用例并构建完整 1.2.39。
  1.2.39 安装后验证了旧宿主的确定失败与原任务查询，不把较早全套回归标成最终源码全套重跑。
- 1.2.38→1.2.39 完整升级保留 endpoint/access、历史任务、事件、输出；安装核对程序
  确认二进制与清单匹配、服务运行且重新认证。用户目录不重建。
- user2 新命令 `4205168c-5c9b-4af2-88fb-c6e08fd3b8aa` 输出 UID/GID 23002、退出 0。
- 运行中服务重启原任务 `293a9a94-06b3-4591-8431-5368c101190a` 在升级后仍为 interrupted；
  原检查已确认仅一次副作用、无残留进程/延迟副作用，未重放任务。

窗口查询 `5be95395-0c07-4b58-96d4-f58e5323bf7e` 和显示器查询
`901cca01-0e61-408b-aa03-f6c4f2cbfd02` 均为 failed，错误明确为 unsupported_platform。
再次用 get_operation 查询原窗口操作仍得到同一确定失败，不是 unconfirmed。
截图（include_image=false）与合法旧 key release 请求同样明确 Unsupported。
首次输入样例误用 kind 而非 type，被参数校验拒绝，未派发；纠正后才验证平台拒绝。

两轮核心流程及各测试用户覆盖见 [完整工作流](execution-e8-final-rounds-2026-10-07.md)。
最后安装 timer/service 均 not-found/inactive；核对任务
`5e71e210-7b59-45e3-ae6e-cb4fa4d830e8`。原生操作索引见
[安装回执](execution-e8-installed-final-2026-10-07.json)。

## 产物

`pixels-agent-bridge-linux-x86_64-debug.tar.gz`，版本 1.2.39：
`67586aa8e88df94cbb10ebf182d99426ee9ee76f90964e97e97d928ce5fb12fe`。

Executor：`3199eead06a3d36ef456c6305b4e550514c3fe3148581afbd6d655d19df1874b`；
MCP：`f25a2857de50e4b642c112eb718891372a4cd645d6522fd35d61db6563a06a59`。
Linux 容器与持久化数据卷仍保留用于后续验收；私有升级备份不提交仓库。
