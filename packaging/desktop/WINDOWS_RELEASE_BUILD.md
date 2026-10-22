# Windows 发布构建缓存

开发、编译和发布只使用主仓库 `D:/GoCloud/PixelsAgentBridge`，使用现有 `scripts/build.py` 或 `scripts/release.py`。禁止创建额外 Git worktree 或复制源码进行构建，包括 `.build` 中的源码副本。其他会话正在修改构建输入或占用编译预算时，等待其完成，不另建目录绕过。保留已有修改、固定的 target 和依赖目录，不执行日常 `cargo clean`。

`scripts/build.py desktop --profile release --package` 会让 Cargo 检查输入变化，复用未变化的后端和依赖，更新前端和受影响的 Desktop 代码，再生成新安装包。安装包版本独立递增，不修改 Rust、npm 或 Tauri 组件版本；不能把旧版 Desktop 二进制直接换标为新版。

Desktop Release 使用 `opt-level = 2`、`lto = false`、`codegen-units = 16`、`incremental = true`。第三方依赖保留优化等级 3，产物继续剥离调试符号。首次切换 profile 会重编受影响的 crate；后续沿用相同参数和目录。后台 workspace 的 profile 保持原设置，以复用既有缓存。

默认使用主仓库 `target`，已有专项缓存可保留在主仓库 `.build` 中并明确设置 `CARGO_TARGET_DIR`；不能再引用已删除的外部工作目录。私有客户端默认值使用主仓库 `.env/client-defaults.toml`，保留已有签名密钥、设备凭据与用户配置，不将其纳入 Git 或公开配置。源码路径本次统一回归主仓库会使部分本地 crate 首次重新编译，后续保持路径稳定。

每机所有会话合计最多 10 个编译作业，嵌套 CMake 使用 1 个作业；发现其他构建占满预算时等待或暂停本次编译。发布前核对新安装包的构建回执、实际解包、公开空 TOML 默认配置、协议工具目录和本次功能；已通过且未受改动影响的远程登录矩阵不重复执行。发布后核对签名、更新接口和 COS 下载文件哈希，不替换本机安装版。
