# 客户端私有编译默认值与公开空配置

日期：2026-10-19。独立 `feat/remote-desktop-v1` worktree，基于 `dce42a1`。
本轮修改客户端配置及三平台安装入口，不修改现有远程传输、鉴权或输入行为。

## 已实现

- 共用配置库在构建时读取被 Git 忽略的 `.env/client-defaults.toml`，或显式
  `PAB_CLIENT_DEFAULTS_FILE`。没有可选文件时默认地址为空；显式文件缺失、非法、
  超限或只填一个地址会失败，错误不回显配置值。
- 新安装 `config.toml` 默认写空地址；客户安装配置优先于编译默认值，
  Desktop/MCP 用户覆盖再优先于安装配置。显式 `--config` 沿用既有优先级。
  两个服务地址必须成对，设置输入框不显示内置值。
- 公开空构建的 Executor 保持运行及本机 IPC，返回 `Unconfigured`，不注册设备。
  Desktop 显示配置提示，不显示空设备凭据，允许进入设置。设置页仍只修改操作端；
  被控端地址需编辑安装 TOML 并重启服务。
- Windows NSIS、Linux 包和 Unix 安装入口移除写死的客户端服务默认值；
  macOS PKG 将自有 `package.env` 改成原生解析的 `package.toml`。
  原有客户 TOML 和注释完整保留，旧 JSON/env 仍沿用不执行内容的迁移。
- 本地私有默认文件已从现有安装配置准备并确认被 Git 忽略；未提交其内容。
  不在源代码、公开示例或安装脚本中保留那一对地址。编入二进制的地址可被提取，
  此机制用于公开源码与本地打包分离，不是隐藏秘密的安全边界。

## 已验证

| 验证 | 结果 |
| --- | --- |
| 共用 Rust 配置测试 | 公开空默认值、私有测试默认值各 16 项通过 |
| Executor 状态回归 | 3 项通过，包含未配置时不返回旧设备凭据 |
| 真实已编译 build script | 可选文件缺失/有效、显式缺失/非法/半对/超限共 6 项通过；无配置值回显 |
| 真实 Executor `configure` | 新建空值、显式空参数、TOML 安装默认、逐字节保留客户注释、拒绝半对共 5 项通过 |
| Desktop 前端 | TypeScript 检查、Vite 构建通过 |
| Windows 安装入口 | PowerShell 语法检查及生产 NSIS 脚本空默认值/客户默认值两种真实编译通过，未执行安装 |
| Unix/macOS 入口 | 5 个 Bash 脚本语法检查通过；macOS 打包 Python 检查 6 项通过、3 项原生安装/PKG 用例因 Windows 环境跳过 |
| 真实隔离程序 | 新 Executor、Desktop、MCP 构建通过；空配置 → 客户覆盖后重启 → 清空覆盖后重启三轮通过 |

真实程序测试使用全新用户/机器数据根、单独的本机 IPC 和 WebView 调试端口，
没有复制登录账号、凭据或设备列表。更新检查也指定无服务的本机地址，未使用生产控制服务。
每轮均检查后台存活、主页待配置提示、隐藏设备凭据、设置输入和保存；
客户覆盖重启后仍可读，清空后两个输入框恢复空值，安装 TOML 原文未变。
MCP 初始化与工具目录读取通过；未注册设备、未创建设备私钥或设备凭据数据库。
所有专属进程退出；自动审批拒绝目录清理命令（仅返回 `blocked by policy`），
全新测试 IPC 密钥、用户/机器数据和 WebView 缓存仍保留在仅当前用户和 SYSTEM 可访问的隔离目录，未记为已删除。

汇总：[JSON](remote-desktop-client-defaults-2026-10-19.json)。真实调试产物 SHA-256：

| 程序 | SHA-256 |
| --- | --- |
| Windows Desktop | `c961f7613ab5a1b659cca80dbc02c6005c4ae9e614a90a30ba09448436741bbd` |
| Windows Executor | `4e9c298b39ccf66ae69909b7d3fb885e4cc1fbc4b180ee2a449bd84bdab2f06b` |
| Windows MCP | `f09fa6d91aa2048f17d8b8a3c5cedb784a712a25a4f00ace6927f693ea7d698e` |

原始日志在 `.build/remote-desktop/config-defaults-check/`，隔离运行记录在
`.build/remote-desktop/config-public-smoke-b24c6775502b/`。
Cargo 使用 4 作业、嵌套 CMake 1 作业，本任务的各次编译顺序执行；启动前核对其他编译进程，保留每机 10 作业上限。

按 `docs/AGENTS.md` 同步官网两个源文件的中英文配置说明，并从官网当前源码的独立
快照执行 `docs:sync`、`docs:check`，15 章与本项目指南一致。官网配置说明单独提交
`f048297`，分支 `docs/client-defaults-20261019-656ee7b7`；未吸收官网原有未提交改动。
官网工作目录仅补对应配置段落，字体生成和网页构建输出均在快照目录，未部署官网。

## 未覆盖

没有执行三平台正式安装/升级，没有通过客户另一套真实控制服务和 Relay 做远程桌面验收。
macOS/Linux 尚未运行本轮配置改动的原生程序；shell/打包检查不替代实机结果。
此前服务切换的旧身份清理实测仍见[活动协助与设置切换](remote-desktop-active-switch-mixed-dpi-2026-10-19.md)。
因此 09.4 保持部分完成，完整关闭数仍为 12/53；本轮没有更新本机安装版、push 或发布 COS。
