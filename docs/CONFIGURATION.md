# 程序配置格式

2026-10-15：项目自己定义的程序配置统一使用 TOML，适用于 Windows、macOS、Linux。第三方或操作系统强制要求的格式保持原样。这条要求也记录在根目录 `AGENTS.md`。

## 配置清单

| 用途 | 当前配置 | 旧格式迁移 |
| --- | --- | --- |
| 本地编译的客户端默认服务 | 不上 Git 的 `.env/client-defaults.toml`；可用 `PAB_CLIENT_DEFAULTS_FILE` 指定其他 TOML 路径 | 没有该文件时编译为空默认值，不回退到公共源码中的地址 |
| Windows 安装默认值、Executor、Desktop/MCP 连接默认值 | 安装目录 `config.toml`，通常为 `C:\Program Files\PixelsAgentBridge\config.toml` | 安装器读取旧 `settings.json`，包括 PowerShell UTF-8 BOM |
| macOS 安装默认值 | `/Library/Application Support/PixelsAgentBridge/config.toml` | 安装器读取旧 `settings.env`；原系统级 `operator-server.json` 的默认值由共同安装配置替代 |
| Linux 安装默认值 | `/etc/pixels-agent-bridge/config.toml` | 安装器读取 `/opt/pixels-agent-bridge/settings.env`，成功后将旧文件保留为 `.migrated` 备份 |
| Desktop 用户服务器设置，MCP 共享 | 用户数据目录 `operator-server.toml` | 第一次使用时读取旧 `operator-server.json`，校验后原子写入 TOML；旧文件保留备份，不再参与读取 |
| MCP 工具分组开关 | 用户数据目录 `mcp-tools.toml` | 第一次使用时迁移 `mcp-tools.json`，保留已有选择和版本升级规则 |
| Server | 原有 `pab-server.toml` | 已使用 TOML |
| Relay | 原有 `pab-relay-server.toml` | 已使用 TOML |
| 官网服务 | 原有 `site.toml` | 已使用 TOML |

用户数据目录：Windows `%LOCALAPPDATA%\PixelsAgentBridge`；macOS `~/Library/Application Support/PixelsAgentBridge`；Linux `$XDG_DATA_HOME/pixels-agent-bridge` 或 `~/.local/share/pixels-agent-bridge`。安装配置的 `[operator].data_dir` 可覆盖该位置。

## 读写与升级规则

共同读写实现位于 `crates/service-config`，完整配置示例位于 `packaging/desktop/config.example.toml`。Executor、Desktop、MCP 和开发用 Bridge/Enroll CLI 在启动线程前加载 TOML，现有内部组件通过启动时环境映射获得参数。

未传 `--config` 时读取所在平台安装配置；Desktop/MCP 随后应用用户 `operator-server.toml` 的服务器地址。显式 `--config PATH` 使用指定文件，并跳过用户服务器地址覆盖。显式文件缺失、TOML 语法错误、未知字段和字段类型错误都会报错，不会回退读取旧配置。没有安装配置的开发环境仍可使用成对的进程环境变量。

## 私有编译默认值与公开空配置

复制 `packaging/desktop/client-defaults.example.toml` 为 `.env/client-defaults.toml`，只在本地填写
`control_url` 和 `relay_urls`。三个平台的 Executor、Desktop、MCP 都由同一个 Cargo 配置库
读取此文件；也可以在构建进程中设置 `PAB_CLIENT_DEFAULTS_FILE` 指向另外的 TOML。
Linux Docker 构建会以只读方式挂载显式文件，默认文件则通过已有源码挂载读取。
换机器构建时须在该构建机器上准备文件或显式路径，Git 不会传递忽略的文件。

文件必须同时提供合法的 WSS 控制地址和 HTTPS Relay 列表；可同时留空。
指定文件不存在、格式错误或只填写一部分时构建失败，不混用旧地址，也不将 TOML
原始值写入错误消息。文件内容变化或删除会触发 Cargo 重建。未提供文件的开源构建
没有内置服务地址。

使用自建 Relay CA 时，可在这个私有 TOML 中增加
`relay_ca_cert = "relay-ca.pem"`，路径相对于该 TOML，也可以使用绝对路径。
编译只嵌入 PEM 证书公钥，不嵌入私钥；缺失、无效 PEM、包含私钥或超过 1 MiB
的文件会使构建失败。证书文件变更同样触发 Cargo 重建。CA 文件及私钥保存在
不上 Git 的本地目录；换构建机器时单独安全提供公钥证书，私钥不属于客户端构建输入。

嵌入的私有 CA 只补充 Relay TLS 的信任根，保留公共 CA、证书链及主机名校验，
不改变控制服务的信任配置。已有客户端需要更新到包含 CA 的发行包才能自动使用
对应节点；完成更新之前，新节点保持维护状态。自部署用户也可继续通过运行时
`relay_ca_cert` 指定自己的 CA 文件。CA 公钥证书随客户端二进制分发，不是秘密。

新安装的 `config.toml` 默认始终写成：

```toml
control_url = ""
relay_urls = []
```

空值在运行时使用已编译的默认服务，不把该默认值回写文件或设置输入框。
客户填写的安装 TOML 优先于内置默认值；Desktop/MCP 的用户覆盖再优先于安装值，
显式 `--config` 仍跳过用户覆盖。每一层地址都按完整的一对切换，不从不同层拼装。
安装命令显式提供的地址只用于首次安装，已有 TOML 及注释始终保留。
macOS PKG 的安装默认值也使用 `package.toml`，由 Executor 原生解析，不执行 shell 配置。

没有内置地址且安装配置为空时，后台服务保持运行及本机 IPC，状态为 `Unconfigured`，
不注册设备、不生成设备身份；Desktop 显示等待配置并允许进入设置。
编辑安装配置后重启后台服务。设置页只修改当前用户的操作端地址；要让本机也成为
被控设备，还需配置安装 TOML。已有 MCP 也需由 AI 客户端重启。

`[executor]` 和 `[operator]` 分开设置数据与日志目录，避免 MCP 意外使用机器服务的设备身份和数据库。相对路径以 TOML 文件所在目录为基准。设备密码、账号登录态和端点私钥继续由原有凭据存储管理。

安装器通过新包的 `pab-executor configure` 创建或迁移配置，在停止运行服务前完成校验。已有 TOML 会完整保留，包括注释；安装命令的服务地址参数只设置首次安装默认值。后续修改请编辑 TOML，随后重启相应服务与 Desktop/MCP。Linux/macOS 的启动脚本不再 `source` 配置文件。

旧 env 仅按受限的字面量赋值解析，不执行 shell。遇到无法无损解析的表达式、未知参数或非法配置时停止升级，用户需手动转换 TOML。用户偏好通过临时文件原子替换，避免另一 MCP 进程读到写了一半的文件。

## 操作端服务器覆盖与切换

Desktop 服务器输入框只显示用户保存的覆盖值，不回显安装默认地址。`operator-server.toml` 不存在，或内容如下时，Desktop/MCP 保留安装配置或开发环境提供的默认连接：

```toml
controlUrl = ""
relayUrl = ""
```

自建服务必须同时填写 `controlUrl`（WSS）和 `relayUrl`（HTTPS）。只填写一项或配置无效时报告错误，不混用新旧地址，也不静默回退。显式 `--config` 仍优先；此时 Desktop 拒绝从设置页修改连接，需编辑指定文件。

两个地址输入框分别提供“验证”按钮，直接检查尚未保存的输入值；空值时按钮禁用。控制服务验证数据库健康接口及 WSS 控制协议响应，中继验证健康接口及 Iroh Relay WebSocket 子协议。校验 TLS，不跟随重定向，整体最长 10 秒；结果区分格式、证书、超时、不可达、协议不符和健康检查失败。验证不保存配置、不登录账号、不注册端点；改动地址后旧结果失效。中继通过仅表示入口连通，实际转发权限及与控制服务的配套关系仍需登录、连接设备后确认。

设置页保存不同的控制服务后，会取消未完成的 GitHub 登录，清除旧服务和目标服务的本地账号登录态与令牌。不会删除服务器上的账号，也不会自动迁移账号；重启后需在目标服务重新登录或注册。仅修改 Relay 不清除账号。网络连接在重启后切换，已有 MCP 进程也需由 AI 客户端重启。

操作端设备密码库、设备列表、任务记录、队列及访客身份改为保存在用户数据目录的 `operators/<控制服务 URL 哈希>/` 下。Desktop 与 MCP 共用同一服务器的数据库，但各 MCP 进程仍有独立端点。新服务重新注册访客身份；切回旧服务可以读取该服务自己的历史记录，但设置页切换后需要重新登录账号。旧版本没有标记所属服务的根目录数据库与私钥保留原地，不自动导入任何新命名空间，以免把旧服务凭据提交给新服务。旧设备可在对应服务登录同步，或重新添加。

语言、主题、MCP 工具开关、AI 集成设置、机器级 Executor 的身份和配置保持原有位置。

## 保持原格式的内容

- Claude、Kimi、OpenCode、Gemini、VS Code 等第三方客户端要求的 JSON/JSONC/YAML；Codex 自身要求的 TOML。
- Tauri 的配置和 capabilities、npm/TypeScript/Playwright/Vite 的构建配置、Docker Compose、launchd plist、systemd unit、SSH 配置、COS CLI 配置等第三方或系统格式。
- 设备身份及登录凭据、数据库、心跳、集成回执、浏览器进程通信、验收结果、构建版本记录、发布目录与签名清单等运行数据。

这些文件不会因为扩展名是 JSON/YAML 而统一改名；只有项目拥有格式定义权的程序配置进入 TOML 改造。
