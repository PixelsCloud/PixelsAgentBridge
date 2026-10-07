# E8 正式宿主完整核心工作流与恢复增量

日期：2026-10-07。宿主为当前会话原生 Pixels MCP 1.2.32，使用公开 `pab_*` 工具；
没有另起 stdio 客户端替代正式工具。逐步操作 ID、实际检查结果与清理数量见
[机器可读记录](execution-e8-final-rounds-2026-10-07.json)。

## 完整核心流程

| 设备 / 身份 | 本轮完成 | 结果 |
|---|---|---|
| Windows 90 / Administrator WTS 1 | 两轮 | 通过 |
| Mac ARM / huayang UID 501 | 两轮 | 通过 |
| Linux headless / service root | 两轮 | 通过 |
| Linux headless / pabuser1 UID 23001 | 两轮 | 通过 |
| Linux headless / pabuser2 UID 23002 | 第二轮 | 通过；第一轮见 native-reload 报告 |

每轮均包括实际账户/home/cwd、中文文本写入/读取/带哈希修改，八种 Git 操作
（status/diff/log/commit/checkout/fetch/pull/push）、独立 bare remote 与模拟其他提交者，
文件搜索/复制/移动/哈希/ZIP 创建和解包/目录列表，262144 字节二进制上传下载，
PTY 创建/resize/中文输入/读取/关闭，以及真实文件内容、归属和 Git 干净状态核对。
下载到本机的九份二进制 SHA-256 均为
`2312394bd99545d9de131c24efb781e765ac1aec243f2ed9347597a793a415e9`。

所有本轮核心流程目录均通过原用户公开删除工具清理：Windows/Linux 每轮 180 条，
Mac 每轮 182 条，均 completed、partial=false。Linux 无 DISPLAY/WAYLAND_DISPLAY。
Windows 默认管理员文件所有者可能是 Administrators 组；实际进程 SID 为指定 Administrator，
检查接受该原生令牌的默认所有者，不把所有文件必须属于个人 SID 当成 Windows 语义。
Windows 第一轮核对脚本默认 GBK 读取 UTF-8 文件失败，改为显式 UTF-8 后只重做核对，
没有重放修改操作，保留原失败回执。

## Windows 安装与编辑器两轮

90 已完整升级到 1.2.32，安装退出 0，三程序哈希一致，设备身份保留、服务 Running。
一次性安装任务已移除。本机 1.2.32 的六种语言/主题组合证据见 E7 UI refresh 报告。

两轮均通过公开工具启动记事本、原生控件写入中文、正常关闭触发未保存提示，
在另存为对话框设置中文完整路径并保存，文件工具回读精确内容，再核对所有者。
打开已有文件复用现有应用工具，未强杀编辑器；两份文件最后均已删除。
系统拒绝 foreground focus 时保留失败结果，不宣称成功激活。

1.2.32 下终止测试 application-helper 39772 后，supervisor 自动重建为 44912，
已打开的测试记事本 7084 保留。随后正常关闭成功，旧 window_ref 查询返回
process already exited、action_dispatched=false。最终检查所有本轮测试编辑器均消失。
恢复任务 `43caec22-e3bb-4927-bf7e-3e97da0386d5`；最后核对任务
`396936b3-8592-4bbc-9fcf-fcbe9fe02bb5`。
WTS 2 的既有断开会话仍可按其实际 SID 执行 whoami，但没有应用上下文；
这不等于两个活动桌面或 split-token 测试已通过。

## Linux 运行中服务重启

UID 23002 任务先写一次计数和自身 PID，再等待后续副作用；通过一次性 systemd
定时作业重启 Executor。重新连接后原任务
`293a9a94-06b3-4591-8431-5368c101190a` 返回 interrupted。
实际检查计数只有一次、原进程不存在、后续副作用不存在。只观察原记录，没有重放。
新测试目录删除成功。最后只读检查 timer/service 均 not-found、inactive，
核对任务 `991828d8-1a24-4312-b71d-54220178babd`。

旧 UID 23001 第一轮目录此前已用于升级持久化核对；本轮最终清理 168 条，
操作 `b709a033-2238-4029-93fc-9b5f2fcd45e6` completed、partial=false。
持久化证据保留在 E7 Linux/refresh 报告中。

## 证据边界

本报告的核心流程不替代 Mac 实际编辑器保存及安装 UI 测试。Mac 保存面板暴露的
AXTextArea 可写状态与跨进程 AX 控件问题已修复并完成 1.2.37 安装复验；
见 [Mac 安装验收](execution-e8-macos-installed-2026-10-07.md)。
Intel、多屏、两个同时活动的 Windows WTS 桌面没有对应实机条件，不能标记通过。
命令输出文本按 UTF-8 解码；原生程序使用其他代码页时须配置其输出为 UTF-8，
这与 PTY 中文、UTF-8 文件工具通过是不同的合同。
