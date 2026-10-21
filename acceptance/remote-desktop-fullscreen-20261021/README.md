# Windows 全屏与 90 安全桌面日志核对

Windows 1.3.14 已包含本项改动并发布 COS，见[发布记录](../windows-close-release-20261021/README.md)。下文开发阶段的“未发布/暂停”描述按当时状态保留。

## 全屏缺少画面区域

用户的 1920×1080 截图中，画面只使用上方 1032 像素，下方剩余 48 像素黑边，左右也因等比缩放出现黑边。

已用已发布的 Windows 1.3.12 隔离包复现“最大化后进入全屏”：本机显示器为 `[0, 0, 1920, 1080]`，任务栏工作区为 `[0, 32, 1920, 1080]`，全屏客户区仍为 `[0, 32, 1920, 1079]`。位置差异取决于各自任务栏设置；同属客户区被工作区裁小。

锁定依赖 Tao 0.35.3 的 Windows 无边框 `WM_NCCALCSIZE` 逻辑在窗口保持最大化时使用 `rcWork`。进入全屏前解除最大化，退出全屏后恢复用户原来的最大化状态；普通窗口原有位置和尺寸由窗口库恢复。修改仅作用于 Windows 远程桌面窗口。

## 90 的 Ctrl+Alt+Delete 中断

只读获取了 90 的 Executor 与桌面助手日志，时间均换算为北京时间：

| 时间 | 日志 | 事件 |
|---|---|---|
| 12:55:51.102 | `executor.log` | `remote desktop MediaUnavailable: local WebSocket closed` |
| 12:55:51.124 | `session-helper-winlogon.log` | `session helper paused for desktop switch` |
| 12:55:56.122 | `executor.log` | `remote desktop MediaUnavailable: local WebSocket closed` |
| 12:55:56.132 | `session-helper-winlogon.log` | `session helper paused for desktop switch` |

同一时段 Default／Winlogon 助手多次交接。日志能确认 90 本地媒体连接随桌面切换关闭，未取得用户截图所对应操作端的同一会话日志，因此不将截图中的最终失败唯一归因于某条记录。

同时修正 SAS 调用边界的一处缺陷：原先 `RuntimeError` 转成字符串后丢失媒体／传输错误的可恢复分类；改为保留错误类型，使用已有的有时限恢复流程。恢复循环只重新打开画面，不自动重发 SecureAttention，不改变认证策略。

本批没有重触发 90 的 Ctrl+Alt+Delete、锁屏或重启 90，也不替换本机正在使用的安装版。原始只读日志保存在 `.build/remote-desktop/fullscreen-workarea-20261021/`；公开证据只保留相关时间线。

## 验证结果

- Windows Release 后端构建通过；包含 SAS 类型保留与托盘兼容修改的最新 Rust 源码检查通过。构建最多 6 个作业、嵌套 1 个作业，监测其他会话并暂停／等待，进程限制在 10 个 CPU 内。
- 修复后的真实隔离 Tauri 窗口：最大化后全屏的客户区为 `[0, 0, 1920, 1080]`，与显示器边界完全一致；Esc 退出恢复最大化工作区。
- 普通窗口全屏也覆盖整块显示器；退出后客户区精确还原为 `[138, 162, 1238, 922]`，保持非最大化。
- 窗口验收使用配置中的本地 HTTP 前端入口。90 当时已有用户的远程画面会话，本批验证操作端客户区尺寸，没有打开第二路原生视频。SAS 的修复完成源码检查，未在用户使用中再次发送 SAS 验收。
- 所有隔离 Desktop、临时前端 HTTP 服务已停止，临时凭据槽已撤销，原凭据不变。本机安装版和 COS 均未更新。

具体坐标、构建范围、源码指纹及脱敏日志时间线见 [result.json](result.json)。
