# Apple 当前源码构建

2026-10-19，源码基线 `90adfbc`。Apple arm64 / macOS 27.0.1 上顺序完成：

- 媒体库 release 单测：30 项通过、0 失败、1 项手工性能测试忽略。
- 完整 Desktop release 构建（`tauri/custom-protocol`）：通过。
- 完整 Executor release 构建：通过。

Cargo 并发 2，原生构建并发 1；未启动并行编译。完整任务约 459.65 秒。
源码压缩包、两项产物 SHA-256、任务 ID 和原始记录哈希见
[构建记录](remote-desktop-macos-matched-build-2026-10-19.json)。

本轮只生成构建产物，没有替换安装版或固定的已授权测试 App，没有申请 TCC 权限。
该结果用于后续同版本平台验收，不代表新的 GUI、系统登录、性能或长期稳定性通过。
