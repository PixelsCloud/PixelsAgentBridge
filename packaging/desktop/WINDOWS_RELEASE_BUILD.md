# Windows 发布构建缓存

重复发布时保留固定的干净发布 checkout、相同的 `CARGO_TARGET_DIR` 和依赖目录；在该 checkout 快进到待发布提交后使用现有 `scripts/build.py` 或 `scripts/release.py`。不要每个版本创建新源码目录，也不要执行 `cargo clean`。仅共享 target 而每次改变源码绝对路径仍可能使本地 crate 失去缓存。

`scripts/build.py desktop --profile release --package` 会让 Cargo 检查输入变化，复用未变化的后端和依赖，更新前端和受影响的 Desktop 代码，再生成新安装包。安装包版本独立递增，不修改 Rust、npm 或 Tauri 组件版本；不能把旧版 Desktop 二进制直接换标为新版。

Desktop Release 使用 `opt-level = 2`、`lto = false`、`codegen-units = 16`、`incremental = true`。第三方依赖保留优化等级 3，产物继续剥离调试符号。首次切换 profile 会重编受影响的 crate；后续沿用相同参数和目录。后台 workspace 的 profile 保持原设置，以复用既有缓存。

Windows 本次沿用的固定发布 checkout 为 `D:/GoCloud/PixelsAgentBridge-windows-close-release-20261021`，固定 target 为 `D:/GoCloud/PixelsAgentBridge-remote-desktop-candidate-20261020/.build/release-target`。目录名称保留以避免改变 Cargo 输入路径。更新 checkout 前必须确认其中没有其他会话的改动。保留私有客户端默认值文件的路径和内容，不将其纳入 Git 或公开配置。

每机所有会话合计最多 10 个编译作业，嵌套 CMake 使用 1 个作业；发现其他构建占满预算时等待或暂停本次编译。发布前核对新安装包的构建回执、实际解包、公开空 TOML 默认配置、协议工具目录和本次功能；已通过且未受改动影响的远程登录矩阵不重复执行。发布后核对签名、更新接口和 COS 下载文件哈希，不替换本机安装版。
