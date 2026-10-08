# Pixels Agent Bridge

**通过 AI Agent 远程操控你的设备。**

[English](README.md) · 简体中文

Pixels Agent Bridge 通过 Model Context Protocol（MCP），将本机 AI Agent
连接到远程计算机。Agent 可以选择设备、确认操作系统、执行原生命令、传输文件、
使用交互终端，以及调用受支持的桌面能力。桌面应用提供设备管理、任务记录，
并实时展示正在操作设备的 MCP 进程及其状态。

项目处于持续开发阶段。Windows、macOS、Linux 的远程命令与文件流程已经完成真机验证；
各平台支持范围与验证状态见下文。

## 目录

新增 **Server Web 管理端**：React + Ant Design，支持账号、设备列表与实时在线状态、用户限速、Relay 策略健康和管理变更记录；管理员直接管理全站设备，无需设备认领。提供简体中文、繁体中文、英文及亮暗主题。远程任务记录仅保存在本地。见[部署指南](WEB_DEPLOYMENT.md)和[完整开发与测试计划](WEB_DEVELOPMENT.md)。

- [主要能力](#主要能力)
- [工作流程与组件](#工作流程)
- [快速开始](#快速开始)
- [MCP 工具](#mcp-工具)
- [连接管理与活动展示](#连接管理与活动展示)
- [常见问题](#常见问题)
- [平台与实现状态](#平台与实现状态)
- [开发与构建](#开发与构建)
- [服务端部署](#服务端部署)
- [仓库结构](#仓库结构)

## 主要能力

- **Agent 操作设备**：通过统一的 `pab_*` MCP 工具选择目标并执行操作，支持 Codex、Kimi Code、Claude Code、DeepSeek Harness、OpenCode 接入。
- **执行原生命令**：指定程序和参数数组，查询任务状态，读取标准输出和错误输出。
  返回结果携带经过验证的目标操作系统。
- **传输文件**：用绝对路径上传、下载二进制文件，校验完整性，并显式决定是否覆盖。
- **交互终端**：打开远程终端、发送输入、读取输出、调整尺寸和关闭会话。
- **桌面能力**：在受支持的桌面会话中列出窗口、向 Agent 返回桌面或指定窗口的原分辨率 JPEG，
  发送鼠标、键盘或 Windows 安全注意序列事件。
- **设备管理**：保存历史设备、重命名、复制信息，分别显示在线状态和连接状态；
  已连接卡片展示当前使用 P2P 还是 Relay。
- **任务记录**：按设备筛选、分页查看，也可在单个设备面板中查看该设备的历史任务。
- **MCP 活动**：在左侧“MCP 连接”页查看进程、客户端身份、设备连接、工具调用、任务和传输摘要。
- **自部署**：使用 Docker Compose 部署控制服务、PostgreSQL 和 Relay。

桌面支持亮色、暗色主题，以及简体中文、繁体中文和英文。
首次启动根据系统语言选择，用户手动切换后保存选择。

## 执行身份与平台范围

命令、PTY、Git、文件及传输默认使用设备服务账户。先用
`pab_list_execution_contexts` 查询，再显式选择 `user`；重连或登录状态变化后重新查询。
Windows/macOS 应用工具使用 `desktop_user`。账户不可用时不会退回服务身份，
结果及本地任务历史保留实际执行身份。

Linux 当前提供无界面的 Executor/MCP，不提供 Linux 桌面版。核心工具无需显示服务
或图形登录；桌面调用明确返回不支持。命令输出默认 UTF-8；`pab_read_output` 和
`pab_run_command` 即时输出支持 `encoding`：`utf8`、`gbk`、`gb18030`、`big5`、
`utf16_le`、`utf16_be`。任务记录提供相同的显示选择，切换只重读原字节，不重新执行。
增量读取保留 `next_offset` 并沿用编码；末尾未完整字符留到下次，EOF 坏字节报告替代。
`include_base64` 返回未筛选的 `[offset,next_offset)` 原字节。任意偏移/尾读起点不保证
恰好是字符边界。文件文本工具另有编码选择。
结果未确认时沿原请求 ID 查询，不重新执行修改操作。

安装后的完整流程证据及硬件未测范围见
[执行验收报告](acceptance/execution-e8-handoff-2026-10-07.md)。

## 工作流程

![Pixels Agent Bridge 工作流程动画](diagram/workflow/agent-workflow.svg)

[打开 SVG](diagram/workflow/agent-workflow.svg) ·
[查看静态 PNG](diagram/workflow/agent-workflow@2x.png)

在浏览器中打开 SVG 可以观看动画，不支持动画的图片预览仍可以显示完整架构。

1. **发起请求**：AI 客户端通过本机 `pab-mcp` 调用 Pixels 工具，当前 MCP 传输使用 stdio。
2. **寻址与认证**：Bridge 确认设备并认证访问；控制服务提供设备身份、在线状态、
   授权后的网络地址和 Relay 策略。
3. **建立连接**：iroh 使用 P2P 或 Relay 回退建立设备连接，网络变化时路径可以切换。
   动画依次展示两种路径，实际传输根据连接情况选择。
4. **执行操作**：目标 Executor 在设备的原生环境中处理命令、文件、终端和桌面请求。
5. **返回结果**：Agent 查询任务状态、进度和输出；任务记录与 MCP 上报让 Desktop 展示执行情况。

**控制服务负责协调接入，操作数据通过设备连接传输，使用 P2P 或 Relay。**
每个 MCP 进程有自己的 Bridge Runtime 和设备连接；Desktop 接收状态并读取任务记录，
不会作为所有 Agent 共用的执行代理。

### 组件职责

| 组件 | 职责 |
|---|---|
| `pab-desktop` | Tauri 界面、设备管理、远程工具、任务记录和 MCP 活动展示 |
| `pab-mcp` | 向 AI Agent 提供设备工具的本机 stdio MCP 服务 |
| `pab-bridge` | 操作端 Runtime、远程操作和本地持久化共享 Rust 库，也提供开发用 CLI |
| `pab-executor` | 目标设备后台服务，认证连接方并执行操作 |
| `pab-server` | 账号、设备、端点、权限和 Relay 策略的 TLS 控制服务 |
| `pab-relay-server` | 按授权转发数据，执行配置的带宽限制 |
| PostgreSQL | 中央账号、设备、成员关系及授权数据 |
| SQLite | 本机设备身份、凭据、任务和操作记录 |

Windows 安装还包含桌面会话辅助进程，提供需要交互桌面或安全桌面的能力，
职责与可见的主界面窗口不同。

## 快速开始

### 1. 安装完整客户端

在操作端和目标计算机安装完整桌面包。Windows 包包含 Desktop、Executor、MCP
和安装脚本，EXE 安装程序启动后台服务并创建快捷方式。

安装需要 WSS 控制地址和 HTTPS Relay 地址，需要互相通信的计算机
应连接同一控制服务。自部署先完成[服务端部署](#服务端部署)。
安装包可按下文的[构建说明](#构建-windows-安装包)生成。

Windows 无人值守访问由机器级服务承担，被操作端无需一直打开桌面主窗口。

### 2. 保存设备连接

1. 在目标计算机的“我的设备”页面查看九位设备码和密码。
2. 在操作端填写对方设备码及密码。
3. 成功连接一次，将设备和凭据保存到本机用户的 Bridge 数据库。
4. 选择已保存的设备，查看基本信息、任务记录或远程工具；双击设备 item 建立连接。

界面将设备码分组显示。MCP 参数和复制的 ID 使用不带空格的九位数字，例如 `123456789`。

### 3. 启用 AI Agent

先为当前系统用户安装需要的客户端。在“设置 → AI Agent”中分别启用
Codex、Kimi Code（2.x 及以上）、Claude Code、DeepSeek Harness 或 OpenCode。
应用验证同包 MCP 程序，并合并当前用户的 Pixels 配置和工具允许规则；不放开其他工具。
存在冲突规则或损坏配置时显示错误，不覆盖原配置。新建会话或重启客户端后生效。
“已配置”不代表客户端已连接；停用配置也不终止已有会话。

OpenCode 使用本地 MCP 命令数组及
`pixels_*` 允许规则，保留已有 JSONC 注释、其他服务和无关权限。
实时 Agent 会话在左侧“设备列表”下方的独立“MCP 连接”页展示。

也可手动写入 Codex 配置；Windows 示例，程序路径按实际安装位置调整：

```toml
[mcp_servers.pixels]
command = "C:\\Program Files\\PixelsAgentBridge\\pab-mcp.exe"
default_tools_approval_mode = "approve"
```

`approve` 表示预先批准工具调用；`auto` 仍可能根据工具声明触发审批。
远端设备认证和操作系统权限仍然有效。

macOS 桌面同样提供上述配置入口。Linux 无界面版和其他 MCP 客户端可手动注册安装目录中的
`run-mcp.sh`。配置目录、客户端版本和实际验证范围见[接入开发与验收记录](acceptance/agent-integrations-2026-10-08.md)。
当前源码改动需要更新 Desktop 和 MCP；之前的安装包不会自动获得这些功能。

Linux 当前只交付 **Executor + MCP 无界面版**，不依赖 Desktop 或图形登录。
[Linux 安装说明](packaging/desktop/unix/INSTALL-LINUX.txt) 包含 systemd 服务、容器前台运行、
设备码/密码查看、升级及数据保留方式。新的无界面打包与隔离安装测试已通过，
新版安装后的正式宿主远程验收仍待完成。

### 4. 让 Agent 操作设备

示例提示：

> 连接设备 123456789，先确认操作系统，再查看系统盘的剩余空间。
> 使用 Pixels 工具，并根据目标操作系统选择命令。

> 将 C:\build\app.zip 上传到设备 123456789 的 C:\Users\Operator\Desktop\app.zip，
> 覆盖现有文件，上传后校验 SHA-256。

将设备码和路径替换为自己的实际信息。

## MCP 工具

当前源码提供 **68 个工具**。宿主可能显示命名空间，例如 `pixels.pab_connect`。

| 工具 | 用途 |
|---|---|
| `pab_list_devices` | 列出本机保存的设备，控制服务离线时也可读取 |
| `pab_list_execution_contexts` | 查询原生服务/用户身份及可用的已验证桌面用户会话 |
| `pab_list_apps` | 查询指定 Windows/macOS 用户会话中的已安装或运行中应用 |
| `pab_launch_app` | 按系统标识或绝对应用路径启动/激活应用 |
| `pab_open_file` | 用指定或默认应用打开目标机器上的本地文件 |
| `pab_connect` | 认证目标设备，返回经过验证的操作系统和执行环境 |
| `pab_run_command` | 按程序和参数数组启动原生程序 |
| `pab_get_task` | 查询状态、进度、完成信息和输出范围 |
| `pab_read_output` | 按偏移读取保存的 stdout 或 stderr |
| `pab_list_directory` | 分页列出目录内容 |
| `pab_list_windows` | 获取包含窗口引用、位置、尺寸、PID、显示器和状态的有限快照 |
| `pab_list_monitors` | 列出显示器 ID、原点、尺寸、主屏、缩放和旋转 |
| `pab_focus_window` | 聚焦指定引用窗口，遵守系统前台限制 |
| `pab_window_control` | 最小化、最大化、还原或请求正常关闭窗口 |
| `pab_type_text` | 向明确指定的前台窗口输入 Unicode 文字 |
| `pab_ui_query` | 在明确窗口或控件内查询有界可访问性控件树 |
| `pab_ui_get` | 读取指定控件属性，可显式读取非保护字段值 |
| `pab_ui_action` | 执行控件支持的调用、填值、勾选、选择、展开、收起或聚焦 |
| `pab_ui_wait` | 异步等待控件条件，采样间释放输入队列 |
| `pab_capture_screenshot` | 以采集分辨率返回桌面或指定窗口的 JPEG，附带坐标、尺寸和哈希 |
| `pab_desktop_input` | 发送原有单事件，或按顺序执行绑定窗口的桌面操作批次 |
| `pab_open_terminal` | 打开交互终端 |
| `pab_terminal_input` | 发送终端输入 |
| `pab_terminal_read` | 读取终端输出 |
| `pab_terminal_resize` | 调整终端尺寸 |
| `pab_terminal_close` | 关闭终端会话 |
| `pab_upload_file` | 异步上传二进制文件，校验后可显式覆盖已有普通文件 |
| `pab_download_file` | 异步下载二进制文件，校验后可显式覆盖已有普通文件 |
| `pab_get_operation` | 按原请求 ID 读取持久化的传输、命令、文件操作或系统查询结果 |
| `pab_list_operations` | 列出本 MCP 的传输、命令、文件操作和系统查询，支持设备/状态过滤及游标 |
| `pab_cancel_operation` | 请求取消操作，查询原请求以确认结果 |
| `pab_disconnect` | 断开本 MCP 与设备的连接，与 Desktop 和其他 MCP 相互独立 |
| `pab_file_stat` | 查看目标文件、目录和链接的元数据 |
| `pab_file_read` | 按行或字节范围读取文本，返回 SHA-256 版本 |
| `pab_file_write` | 暂存后发布文本，显式覆盖，可检查原文件版本 |
| `pab_file_patch` | 核对原版本后进行精确替换 |
| `pab_file_search` | 按名称或文本搜索，支持 glob 和明确的结果上限 |
| `pab_file_hash` | 后台分块计算 SHA-256，支持进度查询及取消 |
| `pab_mkdir` | 创建目录，显式控制父目录和已存在行为，报告部分结果 |
| `pab_file_copy` | 目标机内复制文件或显式递归目录 |
| `pab_file_move` | 复制并核对目标后移除源，支持跨文件系统 |
| `pab_file_delete` | 删除有界、显式递归的计划项，报告实际删除结果 |
| `pab_archive_create` | 按明确源路径生成 ZIP，校验后发布 |
| `pab_archive_extract` | 解压 ZIP，预检路径、类型、冲突和限额，逐文件校验 |
| `pab_system_info` | 查询主机、OS、CPU、内存/Swap 和 Executor 身份，可选 NVIDIA 显卡信息 |
| `pab_list_disks` | 查询磁盘/挂载点、文件系统、容量和可用空间 |
| `pab_list_processes` | 一次采集有界进程列表，支持 PID/名称/用户过滤，不实时分页 |
| `pab_get_process` | 查询指定 PID 当前可获得的身份和资源信息 |
| `pab_list_network_interfaces` | 查询网卡地址、MAC、MTU、状态和累计收发字节 |
| `pab_list_network_connections` | 查询 TCP/UDP、监听端口和可见 PID，支持地址/端口/状态过滤 |
| `pab_resolve_dns` | 通过目标机器配置的 DNS 服务器查询记录 |
| `pab_list_sessions` | 通过 Windows WTS 或 Linux logind 查询 OS 登录会话 |
| `pab_terminate_process` | 核对原生进程身份后请求退出，可显式允许超时强制终止 |
| `pab_list_services` | 按名称和状态查询 Windows SCM / Linux systemd 服务 |
| `pab_get_service` | 查询指定服务的状态、启动方式和运行信息 |
| `pab_service_control` | 异步启动、停止、重启服务或修改启用/禁用配置 |
| `pab_git_status` | 查询仓库状态、分支/HEAD、上游和冲突 |
| `pab_git_diff` | 按明确文件读取工作区、暂存区或提交间的有界差异 |
| `pab_git_log` | 固定起始提交后分页读取历史 |
| `pab_git_commit` | 提交明确选择的文件，保留其他已暂存修改 |
| `pab_git_checkout` | 切换已有本地分支，或检出指定提交的 detached HEAD |
| `pab_git_fetch` | 异步获取已配置远端的提交 |
| `pab_git_pull` | 显式选择 ff-only、merge 或 rebase，异步拉取 |
| `pab_git_push` | 异步推送指定分支，强制模式使用明确的远端引用租约 |
| `pab_list_containers` | 按名称、状态和标签查询目标 Docker 容器 |
| `pab_get_container` | 查询固定容器的状态、镜像、端口、挂载、网络和健康检查 |
| `pab_container_logs` | 按时间和流读取有限、有界的 Docker 日志 |
| `pab_container_control` | 异步启动、停止、重启，并核对观测状态 |

指定屏幕输入新增 `monitor_input`（Executor system v8 / helper v3）。从 `pab_list_monitors` 复制 `input_target`，使用屏幕内逻辑坐标，并核对应用实际效果；屏幕几何或 helper 实例改变时拒绝执行。见[坐标约定与可重复验收](acceptance/README.md)。


先调用 `pab_connect` 并保留目标环境。大部分设备工具需要 `device_code`；
终端后续操作使用打开终端时返回的 `session_id`。

打开终端还会返回实际 `startup.arguments` 与 `startup.mode`：Windows 为
`powershell.exe -NoLogo -NoProfile`（`interactive_no_profile`），macOS 为
`/bin/zsh -l -i`（`interactive_login`），Linux 为 `/bin/sh -i`（`interactive`）。
Mac 会按选中账户读取登录/交互 shell 配置，Linux 遵循 `/bin/sh` 的交互配置规则；
非交互命令工具本身不加载 shell 启动文件。旧端缺少 `startup` 时保留未知，
不根据操作系统猜测。Desktop 终端栏同步显示实际 shell 和参数。
密码从本机 Bridge 数据库读取，不作为工具参数传递。

### 工具分组设置

在 **设置 → MCP 工具** 中选择六组工具。默认启用全部68个工具，延续现有使用方式。连接与任务（15）必选；文件（15）、系统（12）、桌面（14）、Git（8）、Docker（4）可以分别关闭。保存后通过AI客户端重启MCP进程生效；只重启Desktop不会重新加载已运行的MCP。

当前用户的配置保存在PAB数据目录（设置了 `PAB_DATA_DIR` 时使用该目录）下的 `mcp-tools.json`。每个MCP只在启动时读取一次，运行中的进程和已接受的操作继续使用原有行为。关闭可选分组后，已有操作的查询和取消仍可用。工具目录与调用入口使用同一选择，手动调用已关闭工具会在初始化Runtime前失败。分组控制工具发现与调用范围，不改变设备权限或工具自动执行配置。

配置使用版本1和必需的 `enabledGroups` 数组，支持 `core`、`file`、`system`、`desktop`、`git`、`container`，其中 `core` 必须保留。未知字段/分组、重复分组、未知版本或损坏配置会明确阻止MCP启动，不静默启用全部。设置页显示读取错误，允许用户重新选择并保存修复。保存复用tempfile的原子替换，避免MCP启动读到半个JSON。

### 执行用户与应用工具

这些增量已实现并进入首批验收包，后续修复及正式宿主验收进度见[执行上下文规划](EXECUTION_CONTEXT_ROADMAP.md)。
旧版 MCP/Executor 不会因为文档更新而自动获得新能力。

先在同一设备连接中调用 `pab_list_execution_contexts`，原样使用返回的 `selection`。
`service` 是命令/文件原有默认账户；`user` 用于指定原生账户执行命令、终端、Git、文件和传输；
`desktop_user` 用于指定已核验的 Windows/macOS 交互 helper 执行应用操作。
指定用户的命令/终端/Git/文件/目录/传输分别要求 v3/v2/v11/v5/v6/v2 能力。
显式用户不可用时不会回退到服务账户。引用绑定设备、调用者和连接，重连或登录变化后重新查询。
Linux 无界面端支持原生用户执行，不依赖桌面或 logind；应用工具明确返回不支持。

应用工具要求 system-query **v12** 和可用的用户 helper。
`pab_list_apps` 的 `scope` 为 `installed`（默认）或 `running`，可选字面搜索 `search`，
`limit` 为 1–200（默认 100）；返回有界快照，不做实时分页。
`pab_launch_app` 的 `application` 使用 `{"kind":"id","id":"<返回的系统标识>"}`，
或 `{"kind":"path","path":"<绝对可执行文件或 .app 路径>"}`。
`pab_open_file` 的 `path` 是目标机器上已有文件的绝对路径，省略 `application` 时使用该用户的默认应用。
不支持 URL scheme 或任意启动参数。三个工具都必须在 `execution` 中传入 `desktop_user` 选择。

macOS 的 `pab_launch_app` 还支持显式 `new_instance: true`，用于需要单独实例的场景；默认仍由系统复用。
该选项要求 system-query v13 和 application helper v2，旧 helper 在派发前拒绝；Windows 不支持。
应用自身仍可能限制单实例，两种模式都不保证窗口已就绪，需要观察返回的进程和窗口列表。

控件文字写入不等于文档保存完成。“另存为”应分别选择目录、输入文件名，最后读取落盘文件核对内容。
快捷键打开新窗口后焦点会变化，批量输入可能返回 unconfirmed；先观察新窗口，不应直接重放快捷键。

已知 macOS TextEdit 限制（1.2.42 安装版实测）：AX 修改后，未保存文稿直接关闭可能不提示保存，
或在关闭提示中保存出空文件。明确保存、读取文件核对内容后再关闭的流程已验证；此问题尚未修复。
详细证据见 [兼容性验收记录](acceptance/compatibility-release-2026-10-08.md)。
后续[会话复验](acceptance/compatibility-session-resume-2026-10-08.md)中，富文本和纯文本均通过关闭对话框保存并回读；
上轮异常本轮未复现，根因仍未确认，保留上述说明。

结果包含实际执行身份及可核实的进程实例。系统接受请求不等于创建了新进程、窗口已就绪或文档内容已改变。
继续使用已有窗口/控件工具观察、聚焦、输入及请求正常关闭；不会跳过未保存提示。
缺少桌面、锁屏或会话变化不会自动选择其他账户。
遇到 running/unconfirmed 时保留 `request_id`，用 `pab_get_operation` 查询原记录，不自动重新启动。
应用动作不支持取消或回滚，底层系统调用也不承诺硬中断。操作数据只保存在本机 SQLite 记录中。

Desktop 的设备“远程命令”面板中，命令、浏览目录、终端和文件传输共用“执行用户”选择。
刷新可重新发现用户；显式选择过期后禁用提交，必须重新选择。“应用管理”单独指定桌面会话。
任务详情显示实际观察到的账户/会话，旧记录显示“未记录”。
传输复用 MCP 的持久化队列；取消只代表请求，不能确认远端是否已写入文件时保留“结果待确认”，
使用“查询原操作”核对结果，不重新发送文件。上述 UI 已通过源码/浏览器测试，
Windows 1.2.32 安装版已验证三语、亮暗主题设置、真实终端任务身份展示，以及同值语言选择保存和任务卡片/状态布局修复；
这不等于完整业务流程或 Mac UI 已验收。见[安装界面报告](acceptance/execution-e7-ui-refresh-2026-10-07.md)。

Git 使用所选账户自己的配置和凭据助手。选择用户不会自动解锁钥匙串、取得不可用的 SSH agent，
也不会获得交互认证权限。后台工作前应配置好该账户的凭据；认证失败或超时时查询原操作。
Windows、macOS、Linux 的独立 SSH agent 实测已通过；Mac 独立 Keychain 的可读、锁定拒绝及恢复读取
已通过指定用户 worker 测试，不代表任意登录钥匙串或第三方凭据助手都能自动访问。

### Docker 容器操作

四个工具复用 [Bollard 0.21.1](https://docs.rs/bollard/0.21.1/bollard/) 访问 [Docker Engine API](https://docs.docker.com/reference/api/engine/)，要求目标 system-query 能力 **v6**，以及 Executor 运行账户能够访问的 Docker Engine，不依赖 Docker CLI。连接采用本地 `DOCKER_HOST`（Unix socket／Windows named pipe）或标准本机端点；不会隐式复用 Docker CLI context、登录用户的 Docker Desktop 环境或远程 TCP／SSH 端点。Rootless／非标准 socket 需配置给 Executor。协商后的 API 版本通过 Bollard 请求修饰接口明确应用到请求路径。

列表默认只显示运行中的容器（`all=false`），`all=true` 包含已停止容器。名称按不区分大小写的字面子串匹配，`states` 使用准确状态，标签使用 `key` 或 `key=value`。不做实时分页或原子快照；最多扫描 10000 项，返回 `limit` 项（默认100、最多1000），同时受实际序列化结果 32 KiB 上限约束，明确报告截断。

详情、日志和控制接受**准确名称或完整64字符ID**，拒绝缩写ID。解析后固定完整ID，后续不重新按名称选择容器。结果还返回Engine的OS、架构和版本，与Executor的OS分别描述。详情只返回状态、健康检查、镜像、端口、挂载、网络、重启策略和日志驱动等选定字段，不返回环境变量、命令参数或原始 inspect；标签与日志仍可能包含应用提供的敏感内容。列表字段、字符串和详情集合都有明确上限。

日志默认 `tail=200`、时间戳和两种流开启，`follow=false`。`since`／`until` 是 Unix 秒，最大2147483647，until须大于0；tail使用Docker自身的行数选择，最多1000，不保证每种流分别返回该行数。文本保留字节受 `max_bytes`（默认／最多16384，最少1024）和序列化结果预算共同约束。返回按流分组，不保证跨流次序；TTY输出混合，不能仅选stderr。同流跨块UTF-8先拼接，非法编码明确提示替换。这是有限的日志尾部，不是无损续读游标；不支持的日志驱动和Docker错误直接返回。

控制支持 `start`、`stop`、`restart`。约250ms后仍运行就返回操作引用，使用原ID和 `pab_get_operation` 查询。启动／停止已处于目标状态时不提交动作；重启需运行状态且 `StartedAt` 改变。停止／重启采用Docker正常超时（`stop_timeout_seconds=10`，0–120），期限后Docker可能kill容器。仅收到HTTP成功回执不会标记完成。工具本身不创建／删除容器或拉取镜像。

结果分别记录 `submission_started`（进入提交阶段，可能已发送动作）、`daemon_acknowledged` 和 `desired_state_observed`。取消只结束等待，不撤销Docker已接收的动作。超时、断线或重启可能留下 `unconfirmed`，相同ID不重执行；该Engine／容器上未确认的已提交控制会持续阻止另一PAB控制，包括Executor重启后。查询原请求，只读核对原Engine和完整容器ID；目标状态匹配不等于能确定是谁执行的。外部Docker客户端仍可能并发改变状态。

HTTP替身、QUIC、stdio及Windows named pipe连接Docker Desktop Linux Engine的临时容器流程已通过，包含中文stdout、stderr、启动／重启／停止、详情；测试容器已清理。macOS 替身及协议测试也已通过；真实 macOS Engine、安装后的 Pixels 调用、Linux 运行时及 Windows 容器尚未验收。需更新 MCP 和 Executor、重启 AI 客户端；macOS 安装包见 [MACOS.md](MACOS.md)。

### Git 操作

Git 工具要求目标 system-query 能力 **v5**，以及原生 Git（`switch` 需要 2.23 或更新版本）。复用成熟的 [Git](https://git-scm.com/docs) 仓库、传输、凭据和 hooks 实现，通过明确的程序参数调用，不拼接 Shell。不传 `execution` 时使用 Executor 服务账户的 Git 配置：Windows SYSTEM 服务不会自动取得登录用户的密钥和提交身份。需要使用该账户时，先发现并传入其 `user` 选择（要求 system-query **v11**）。缺少 Git、认证和权限问题直接返回错误。远端参数使用已配置的名字，如 `origin`，不接收 URL 或密码。

`repo` 必须是**目标计算机**的绝对路径。Status 是有界的当前观测，不是原子仓库快照。Diff 禁用外部 diff/textconv，二进制变化返回摘要。历史分页保留返回的 `start_commit`，后续传入 `start`，并使用返回的 `next_skip`；因字节预算截短时也不会跳过未返回的提交。单个结果最多 32 KiB，明确标记截断；请求总量最多 60 KiB。

`pab_git_log` 参数示例：

```json
{
  "device_code": "123456789",
  "repo": "C:\\work\\project",
  "limit": 20,
  "request_id": "8b7f1714-32dc-4b79-bd87-96a1d2101af0"
}
```

Commit 要求明确的相对文件路径和提交说明，支持已跟踪文件的删除，拒绝目录。只暂存指定路径并提交其当前工作区内容，保留无关的暂存修改；失败时已选文件可能仍留在暂存区。Checkout 不强制覆盖、不自动 stash。Pull 要求干净且已检出分支的工作区，必须指定 `strategy`：`ff_only`、`merge` 或 `rebase`；冲突保留给后续检查，不自动 abort。

修改类操作约 250 ms 后仍运行就返回操作引用，保留 `request_id`，通过 `pab_get_operation` 查询；相同 ID 永远不重执行。网络操作默认期限 300000 ms，其他默认 30000 ms。取消请求停止本次 Git 进程，不承诺回滚；service 模式的 SSH/hooks 子进程可能继续存在。显式 user 操作会在释放仓库锁前回收所属工作进程树，但不能撤销既有副作用或停止已交给独立服务的工作。已开始的修改遇到超时或取消，结果可以是 `unconfirmed`。同一实际 Git 目录上的其他 PAB 操作返回忙；外部程序仍由 Git 自身锁保护。

Push 固定选定的提交 ID，默认 `force=false`；`force=true` 使用刚观测的远端引用作为明确 lease。推送回执丢失后，查询可核对原提交与远端引用，不会重推；引用匹配只证明目标状态已经存在，不归因于某个进程。不自动 reset、强推、创建提交身份或绕过 hooks。请求身份对提交说明做摘要，Git 输出与任务历史仍可能包含提交说明。

Windows/macOS 临时仓库、本地 bare 远端、取消、持久化、QUIC 和 stdio 测试已通过。三平台指定用户 worker 的真实 SSH 认证测试也已通过；安装后的 Linux 指定用户已用八种 Git 工具完成隔离本地远端流程，见[Linux 报告](acceptance/execution-e8-linux-first-2026-10-07.md)。这些证据不代表正式宿主 SSH 认证或完整两轮工作流已验收。需要重新构建 MCP 与目标 Executor，并重启 AI 客户端；macOS 安装包及验证范围见 [MACOS.md](MACOS.md)。

### 系统查询

进程终止和服务管理要求 system-query 能力版本 3。先用 `pab_get_process` 获取 `termination_identity`，再传入 `pab_terminate_process` 的 `identity`；不能用秒级启动时间代替。Windows 使用原生创建时间并在操作期间持有同一个进程句柄。Linux 持有原进程的 pidfd，身份租约有效 10 分钟、最多 256 个；过期或 Executor 重启后需要重新查询，不回退到按 PID 发信号。

终止默认 `force=false`、等待 5000 ms。Windows 对顶层窗口发送 WM_CLOSE；无窗口进程明确返回不支持正常退出，显式 `force=true` 才直接强制终止。Linux 先发 SIGTERM，超时且 `force=true` 才发 SIGKILL；强制后的退出确认另有最多 5 秒等待。只操作一个进程，不终止进程树，不允许终止 Executor 自身或系统 init。

服务查询使用 Windows SCM 或 Linux systemd 系统总线，Linux 要求完整 `.service` 名称。列表按名称字面子串、状态精确过滤，不实时分页；列表是摘要，详细配置使用 `pab_get_service`。Windows 列表不含内核驱动，Linux 会包含未加载的已安装服务（`not_loaded`）。启用/禁用只改启动配置，Windows 启用设为自动启动；Linux 修改持久化 unit 链接，不强制解除屏蔽。启动、停止、重启等待实际状态，Linux 还核对原 job 完成信号。Windows 不额外停止依赖服务；systemd 仍可能执行 unit 定义的依赖事务。不能停止或重启 Executor 自己所在的服务。

这两个控制工具是异步操作：约 250 ms 后仍未完成就返回 `running`，使用原 `request_id` 和 `pab_get_operation` 查询结果。相同 ID 永远不重放，断开或调用方取消不会撤销已提交动作，服务超时也不回滚。结果保留阶段、是否提交过修改、Linux job 路径、观测状态和错误；未确认不等于未执行。运行中目前只报告总状态，细分阶段在最终结果中返回。相同资源的并发控制返回 `resource_busy`；不增加审批流程，OS 权限错误直接返回。服务控制超时默认 30000 ms，范围 100–60000 ms；原生阻塞 SCM 调用不能硬中断。Windows 专用窗口、临时服务及隔离 QUIC 验证已通过，Linux 原生后端已交叉编译检查，Linux 真机及安装后验收仍待完成。

系统查询使用 `sysinfo`，要求目标支持 system-query 能力版本 1。每次返回带采集起止时间的一次结果，不持续监控，也不是 OS 原子快照。列表默认 100 项，最多 1000 项，同时受 32 KiB 实际序列化预算限制；`truncated=true` 时使用过滤缩小范围，不做实时分页。可选字段不可读取时返回 null，不采集进程命令参数和环境变量。

`sample_cpu` 在系统信息中默认开启，在进程查询中默认关闭。开启后按库的最小间隔采样两次，`cpu_sample_ms` 表示区间；`cpu_usage_basis_points` 中 10000 等于 100%，进程跨核 CPU 可超过 100%。容量单位为字节；频率是第一个逻辑 CPU 的 MHz，不是所有核心的平均值。网卡返回库提供的累计计数，不是瞬时速度。

`include_gpu=true` 时通过动态加载的 `nvml-wrapper` 查询 NVIDIA。GPU 分区单独报告状态与字段错误；NVML 不可用不能解释为没有显卡。AMD/Intel 后端尚未实现。系统读取最多等待 5 秒后返回当前状态，未完成时用原 ID 查询；暂不支持取消阻塞 OS/驱动读取。Executor 同时执行最多 2 个系统查询，共接受最多 16 个执行中或排队请求；等待执行槽位和采集器锁各最多 5 秒。队列满返回 `executor_busy`，排队超时返回 `queue_timeout`，不会重新采样。Git/Docker 排队请求支持取消，取消后不派发执行。每个 MCP 最多 16 个未解决的系统查询。相同 `request_id` 通过 `pab_get_operation` 读取原采样，省略 ID 才产生新采样；中断或结果未确认时不自动重跑。Desktop 任务记录显示查询类型和返回项数。

这组工具已通过 Windows 本地测试，包含真实进程生命周期、结果持久化和隔离 QUIC。安装后的宿主、Windows/Linux 双机和完整 NVIDIA 硬件验收仍待完成；现有安装包不含本批改动。

网络连接、DNS 和 OS 会话查询要求 system-query 能力版本 2，C1 工具继续兼容版本 1。复用原采样持久化、输出预算、任务记录和原 ID 查询规则。

连接表使用 `netstat2`，支持 TCP/UDP、IPv4/IPv6、精确 IP/端口/PID 和 TCP 状态过滤，不做实时分页。UDP 没有可观察的远端及状态；TCP 监听条目的远端为 null，空 PID 列表表示未观察到归属。扫描受到 100000 项和软性 5 秒期限约束，截断明确报告；这不能硬中断库的阻塞枚举调用。

DNS 使用 `hickory-resolver`，每次读取目标系统 DNS 配置，不回退公共 DNS、不使用本地解析缓存。默认 A，另支持 AAAA/CNAME/MX/NS/PTR/SOA/SRV/TXT；PTR 可输入 IP 或反向域名，域名使用 ASCII/IDNA。返回名称、类型、TTL 和 DNS 展示文本，长值有 `value_truncated` 标记。查询超时默认 5000 ms，范围 100–10000；读取系统配置属于另行完成的原生 I/O。这是 DNS 查询，不等同于系统原生解析器：不读取 hosts，不处理 mDNS 或 Windows NRPT/VPN 分流策略，上游 DNS 仍可能有缓存。

会话查询列出 OS 会话，不是账户、MCP 会话或 PAB 终端。Windows WTS 可以包含尚无登录用户的服务/监听会话。Linux 通过系统总线访问 logind，采集期限为 5 秒；logind 缺失、访问失败或不支持的平台明确失败，可选字段失败写入每项 `errors`。用户名和状态过滤均为精确匹配。这三个工具已通过 Windows 本地测试、本地 DNS 替身和隔离 QUIC；Linux 采集库交叉编译检查通过。macOS 原生采集和自动化也已通过，会话使用 utmpx，见 [MACOS.md](MACOS.md)。Linux 运行时和安装后的宿主验收仍待完成。

文本工具要求目标 Executor 也升级。支持 UTF-8、UTF-16 和 BOM 检测，不静默替换
无法解码的字节。普通按行/按字节读取及修改的文件上限 4 MiB，单次最多返回 16 KiB UTF-8 文本，写入及补丁
输入上限 128 KiB。继续读取时使用 `next_offset`，把返回的 `metadata.sha256`
作为 `expected_hash`。补丁的每项包含 `find`、`replace` 和 `expected_matches`
（默认 1），全部针对原文匹配，不允许重叠。写入与修改返回 `operation_ref`，
重复提交沿用 `request_id`；结果未确认时用 `pab_get_operation` 查询原请求。
这一组有界文本操作等待完成返回，暂不支持取消。文件正文使用设备二进制通道，
不写入任务记录。版本检查与 PAB 路径锁不等于针对外部编辑器的操作系统级原子 CAS。

搜索、Hash 和创建目录要求目标 Executor 的文件能力版本为 2，原有文本工具仍兼容版本 1。搜索默认使用字面子串；默认按名称、区分大小写，glob 使用 `/` 分隔的相对路径，例如 `**/*.rs`。会包含隐藏文件，不应用 gitignore。每次最多返回 100 项；内容匹配返回行号、最多 160 字符的行首预览和该文件的 SHA-256。扫描受到 4096 项、64 MiB 读取计费预算、5 秒和输出字节上限约束；通过 `truncated`、`stop_reason`、跳过数和有限警告说明不完整结果，目录不会被当作原子快照；新版支持带版本复核的续查，见下文。

文件能力 v4 在原工具上增加以下参数：

- `pab_file_read` 的 `mode: "tail"` 读取文件末尾，`tail_bytes` 默认 16384；`mode: "follow"` 使用 `cursor` 或原始字节 `offset` 续读，`wait_ms` 为 0–5000。这两种模式按块读取大日志，不受普通文本读取的 4 MiB 文件上限约束，不计算整文件哈希，也不接受 `expected_hash`。`max_bytes` 为 4–16384，返回文本也最多 16 KiB。每次返回 `result.log.cursor`；下一次携带该 cursor 和原编码，使用新读取请求。末尾不完整的 UTF-8 字符、UTF-16 单元/代理对保留到下一次，`incomplete_character` 明确标记；没有新内容时返回 `wait_expired`。文件身份由 `same-file` 获取，cursor 校验文件头及读取边界附近的字节，观察到轮转、截断或相关字节变化时返回 `log_changed`。这不是整文件历史快照，不能检测所有中间区域的外部改写。尾部读取从字符边界开始，可能省略窗口开头的不完整字符；UTF-16 无 BOM 时仍需显式指定编码。
- `pab_file_search` 增加 `regex`、`exclude`、`context_lines`、`cursor`。正则采用 [Rust regex](https://docs.rs/regex/1.13.1/regex/struct.RegexBuilder.html)，使用有限编译预算；不支持的语法明确报错。排除规则继续使用 `globset`，例如 `["node_modules/**", ".git/**"]`，匹配目录时剪枝。上下文最多前后各 5 行，每行预览最多 160 字符。返回 `result.search.next_cursor` 后，保持查询及过滤参数继续调用；会复核排序后的目录元数据清单和续读文件哈希，观察到变化时返回 `search_changed`。清单因深度、条数或时间预算不完整时不发续查 cursor；检查 `truncated`、`stop_reason`、warnings。5 秒为按 I/O/行检查的扫描预算，不承诺硬中断单个底层调用或正则匹配。
- `pab_file_patch` 增加 `dry_run: true`。复用已有多处非重叠替换和哈希检查，返回 `patch_preview` 的原哈希、预计结果哈希/大小、是否变化和各项匹配次数，不发布文件。正式应用使用**新 request_id**、`dry_run: false` 和原 `expected_hash`；文件期间变化则拒绝写入。预览不是锁定文件的预约，也不是新审批步骤。

例如读取最后 8 KiB 并继续等待日志：

```json
{ "device_code": "123456789", "path": "C:\\logs\\app.log", "mode": "tail", "tail_bytes": 8192 }
```

```json
{ "device_code": "123456789", "path": "C:\\logs\\app.log", "mode": "follow", "cursor": "上次 result.log.cursor", "wait_ms": 5000 }
```

这些增强需要新 MCP 和支持文件能力 v4 的 Executor；旧端会在执行前被拒绝，不会忽略新参数。基础参数仍保持旧版线格式及请求去重指纹。Windows/macOS 本机自动化和隔离 QUIC 已验证；安装版真实宿主及 Linux 原生验收仍待完成。

`pab_file_hash` 在远端确认接收后返回 `operation_ref`，后台按 256 KiB 分块计算，不全量载入大文件。使用 `pab_get_operation` 查询 `progress.completed_bytes`、`progress.total_bytes` 和最终 `metadata.sha256`；使用 `pab_cancel_operation` 请求停止。只有 `cancelled` 才确认已停止。Executor 同时最多 4 个 Hash 作业，每个最多 30 分钟，单次读取超时为 30 秒。相同 `request_id` 返回原操作，文件后续变化也不会触发重算。观察到大小、修改时间或可用身份变化时明确失败，不宣称提供外部并发写入下的原子快照。MCP 会周期刷新活动记录；离线查询可能返回最后保存的状态，应结合进度时间判断新鲜度。

`pab_mkdir` 默认 `parents=false`、`exist_ok=false`，通过 `created_paths` 报告实际创建目录及失败前的部分结果，不自动回滚。结果不明时查询原 ID，不自动重放。每个 MCP 最多保留 32 个未解决的活动文件操作，达到上限应先核对已有结果。上述工具不主动跟随符号链接或 Windows reparse 点；路径复核及 PAB 自身锁不能消除外部程序的全部并发竞态。

复制、移动、删除和 ZIP 要求目标 Executor 的文件能力版本至少为 3。远端接收后返回 `operation_ref`，随后后台执行；通过既有操作工具查询或取消。源和目标是精确路径，不自动附加文件名。默认 `recursive=false`、`overwrite=false`；目录复制/移动显式覆盖时可合并目录，保留目标中的无关条目。移动在同盘和跨盘都采用复制、校验、删源的顺序，需要额外 I/O 和暂存空间。

后台使用 64 KiB 缓冲区，Executor 同时最多 4 个批量作业，协作式期限为 30 分钟。默认 4096 项、1 GiB 文件数据、64 层；`max_bytes` 最大可设 8 GiB。解压的项目限额包含隐含父目录和目标根目录；ZIP 输入和暂存最多为 `max_bytes + 2 MiB`，中央目录元数据最多 2 MiB。只解压未加密的 Stored/Deflate ZIP，路径必须是 UTF-8 可移植名称；拒绝链接、越界、大小写重名和文件/目录冲突，逐文件通过 CRC、长度和暂存 hash 后才发布。`max_ratio` 默认 200，范围 1–1000；高压缩率的正常压缩包也可能因限额被拒绝。

`mutation` 返回阶段、计划/处理项数、写入/删除数、`partial`、`source_removed` 和有限逐项结果（64 项或 8 KiB）。取消在检查点停止，阻塞 OS I/O 可能延迟停止；只有 `cancelled` 确认 worker 已退出。失败或取消保留已生效项，包括后续 ZIP 条目 CRC 损坏前已解出的文件，不自动回滚。删除只移除计划项，拒绝盘符/根目录。结果未确认时查询原 ID，不重新执行。PAB 路径锁覆盖祖先和子路径，但不提供对外部程序的原子目录操作；ZIP 不保留 ACL、属主和扩展元数据。

新文件工具已完成 Windows 本地自动化验证，包括隔离 QUIC 与 MCP stdio。安装后的宿主调用、物理跨盘和 Windows/Linux 双机验收仍待完成；旧安装包不含这些新增工具。

### 控件查询与操作

四个 `pab_ui_*` 工具要求系统能力 v9、桌面 helper v4。Windows 复用精确固定的
`uiautomation 0.25.1`（Apache-2.0）及微软官方绑定；macOS 复用
`accessibility`/`accessibility-sys 0.2.0`（MIT/Apache-2.0）。原生对象运行在可回收的
内部 `--ui-worker` 进程中，仍属于桌面程序，不增加网络连接。

先用 `pab_list_windows` 获取窗口引用，再调用 `pab_ui_query`：

```json
{"device_code":"123456789","scope":{"type":"window","window_ref":"<返回的 UUID>"},"selector":{"role":"text_field","name":"搜索"}}
```

从返回结果明确选择一个 `element_ref`，供 get/action 使用；名称不保证唯一。
填值使用 `"action":{"type":"set_value","value":"hello"}`。query 和 action 不返回字段值，
需要时显式 get 并设置 `"include_value":true`。密码字段不读取也不写入。
无名表格行可以查询子树、读取单元格后确定父行引用。只执行 `supported_actions` 声明的能力，
失败不隐式改用坐标点击或键盘输入。

查询默认100个结果、深度6、3秒，最大500个结果、深度12、访问2000节点、10秒。
响应不超过32 KiB并标明截断；应缩小范围，不对变化中的树做offset分页。
wait支持exists/absent/enabled/value_equals/checked/selected，默认5秒、250毫秒采样，
最长30秒。部分查询不能证明控件消失，需要唯一目标的条件遇到多个匹配会报ambiguous。

引用绑定当前认证连接和helper会话；窗口关闭、worker重启或引用过期后需重新查询。
可以通过 `expected` 前置条件拒绝已经变化的控件，但原生校验和动作不是原子事务。
约250毫秒未完成时返回操作引用；保留 `request_id`，用 `pab_get_operation` 观察，
未确认的动作不能盲目重放。取消不撤销已派发效果。`verification` 区分原生API返回和
实际属性达成；调用按钮不等于已经完成保存等业务。本地摘要不显示控件名和输入文字；
显式query/get的结果会在本地保留，AI宿主也可能保留这些返回内容。

开发范围和当前验收证据见[控件长任务](UI_AUTOMATION_ROADMAP.md)及
[验证报告](acceptance/ui-u0-2026-10-06.md)。本轮不包含Linux可访问性、安全桌面及任意自绘控件。

### 窗口引用、控制与 Unicode 输入

显示器与窗口枚举复用 [xcap](https://github.com/nashaofu/xcap)，文字输入复用
[Enigo](https://github.com/enigo-rs/enigo)，外部窗口操作使用 Windows 官方绑定或
[x11rb](https://github.com/psychon/x11rb) 的标准 EWMH 消息。这些工具要求 Executor
系统能力 v4 和新版活动桌面 helper。Windows 与 Linux/X11 适配已实现；Linux 图形环境
实机验收待补。Wayland 明确返回不支持。macOS 已实现公共 Accessibility 窗口控制与
Enigo 输入，需要屏幕录制/辅助功能授权；原生构建和自动化已通过，真实图形操作因权限
跳过。完整安装、验证结果和限制见 [macOS 适配文档](MACOS.md)。

先用 `pab_list_windows` 获取 `window_ref`，再对该引用执行操作。输入前先调用
`pab_focus_window`。窗口列表最多 64 个，显示器最多 32 个，响应最多 32 KiB，超限明确
标记截断。枚举遵循 xcap 的筛选规则（Windows 排除 helper 自身进程及隐藏窗口等），
采样中消失或无法读取属性的条目跳过。坐标采用 xcap 原生坐标，不能直接当作缩略截图像素坐标。引用仅属于当前
helper 连接；重连、切换桌面、窗口销毁或身份标记丢失后失效。执行前校验 PID 与窗口
身份标记，但外部应用仍可在校验与操作之间改变窗口或焦点，这不是原子窗口锁。

文字限制为 1–4096 个 UTF-8 字节，不允许 NUL，不替换剪贴板。输入 API 接受不代表应用
最终文本正确；焦点变化或应用行为可能造成部分输入。关闭只发送正常关闭请求，不结束
进程；保存提示框或应用拒绝关闭时，会返回失败并标明副作用尚未确认。

保留 `request_id` / `operation_ref`，同一个请求不会重复执行。慢操作先返回 `running`，
断线后通过 `pab_get_operation` 获取原结果。已派发操作丢失 helper 后标记未确认，Executor
重启后也不重放已接受的修改操作。这些操作不能取消或撤销。持久化记录仅保存输入文字的
BLAKE3 摘要，不保存原文；摘要不是加密，不能防止对可预测文字的猜测。目标应用和 AI
宿主仍可能保留输入文字。

### 桌面批量操作

`pab_desktop_input` 保留原有 `event`，并新增互斥的批量形式：`window_ref`、
`actions`、可选 `request_id` 和 `timeout_ms`（默认 5000，范围 100–10000）。
批量操作要求 Executor 系统能力 v7、桌面 helper v2；旧 helper 仍可使用原有单窗口工具。
列出窗口后，用实际返回的引用替换示例值：

```json
{
  "device_code": "123456789",
  "window_ref": "替换为返回的窗口引用",
  "actions": [
    {"type": "focus"},
    {"type": "key_chord", "modifiers": ["control"], "key": "a"},
    {"type": "type_text", "text": "Hello 世界"}
  ]
}
```

每批 1–32 步，支持 `focus`、`control`、`type_text`、`key_chord`、`click`、
`scroll`、`wait`，全部绑定同一窗口。文字合计最多 4096 UTF-8 字节；单次等待
1–2000 ms，等待总和须小于整批期限。快捷键使用不重复的 `control/alt/shift/meta`
修饰键，以及小写字母、数字、`f1`–`f12` 或 `enter/tab/page_down` 等命名键。
点击坐标相对执行时的窗口外框，采用 xcap 原生坐标，不是截图像素。点击前校验鼠标
实际指向的窗口；滚动要求鼠标已位于目标前台窗口内，`axis` 为 `horizontal/vertical`，
`amount` 为 -100 至 100 的非零值，正数向右/下。

当前 helper 将整批作为一个请求处理，其他 Agent 的 helper 输入不会穿插其中，但
无法排除用户或其他软件的操作。输入前校验窗口身份和焦点，每步之间及等待期间校验
活动桌面。任一步出错即停止后续步骤；`result.data.snapshot.batch` 返回从 0 开始的
索引、完成数、失败步骤，以及各步 `completed/failed/unconfirmed/skipped` 状态。
已发生的效果不回滚，输入 API 接受也不代表应用内容正确。普通错误路径会释放按键和
鼠标按钮；进程被强制结束时无法保证清理。

期限控制后续步骤是否开始，不能硬中断阻塞的系统调用。耗时请求可先返回 `running`，
使用原 `operation_ref` 查询；同一 `request_id` 不重放。持久化请求身份对输入文字和
快捷键值做哈希。helper 断开或 Executor 重启可能留下结果未确认的记录，不自动重试。

### 当前桌面与窗口截图

截图复用 [xcap](https://github.com/nashaofu/xcap) 与 [image](https://github.com/image-rs/image)。
`pab_capture_screenshot` 按采集分辨率编码一次 JPEG，默认质量 85（可选 `quality`：30–95）。
不缩放、不自动降低质量，不限制图片字节数。MCP 不提供 `max_width`、`max_height`、
`max_bytes`、PNG 或原始图片模式。每次获取当前画面，编码后释放源像素；不保留原始画面
缓存、`capture_id` 或同帧区域查询。

```json
{"device_code":"214601537"}
```

指定窗口时，先从 `pab_list_windows` 获取 `window_ref`：

```json
{"device_code":"214601537","window_ref":"<窗口列表返回的引用>"}
```

JPEG 截图要求截图能力 v3 及新版活动 helper 连接；窗口截图不主动聚焦或还原窗口。引用失效、
最小化、窗口不可用，或采集中观察到窗口位置/尺寸/显示器变化时明确失败，不截取其他窗口。
采集遵循 xcap 的可见性和权限限制，不保证保护窗口或 GPU 渲染应用可获取有效内容。
`window_ref` 不能与 `monitor_id` / `region` 混用；桌面区域仍相对所选显示器，越界报错。

元数据包含窗口引用及客户区坐标范围、采集时间、显示器、原点、源/预览尺寸、实际质量、
字节数和 SHA-256。`preview_to_desktop` 按 `桌面坐标 = 原点 + 预览像素 × 比例` 映射到
xcap 原生坐标，即使 DPI 使源图像像素数与窗口坐标范围不同也按窗口范围计算。映射描述
采集时刻，输入前应重新核对窗口；不能直接当作 0..65535 的归一化鼠标输入值。此 JPEG
模式不沿用 16 Mi 像素、单边 16384 的旧预算；JPEG 格式本身每边可表示至 65535。
位置校验也不是原子锁。

已有截图历史只保存返回的压缩图片。`destination` 可省略，指定时必须绝对路径、`.jpg` 或
`.jpeg` 扩展名，且不覆盖已有文件；`include_image=false` 仅返回压缩文件和元数据。
Base64 仅在 MCP 图片内容块，不重复进入 JSON、SQLite 或活动上报。助手和网络均分块
传输二进制，不限制图片总大小，并校验实际编码、尺寸、长度和哈希。桌面端截图和实时
预览也使用此 JPEG 模式。

JPEG 模式的显示器和窗口采集均要求 v3，旧 helper 返回升级提示。内部旧客户端的预览和
主屏 PNG 请求保持兼容。已验证 Windows 专用子进程窗口截图、负原点/DPI 映射、大分辨率
JPEG 编码及超过 8 MiB 图片的隔离助手/QUIC/历史记录/MCP 返回；
物理 4K/多屏、安装后宿主呈图，以及 Linux/macOS 图形环境仍待验收。


### 命令、等待和输出增强

- `pab_run_command` 支持 `request_id`、`env`、`stdin_text`、`timeout_ms` 和 `wait_ms`。相同 ID、相同参数读取原任务，参数不同报冲突；重试时沿用原 ID。
- `env` 合并到目标进程继承的环境，最多 64 项、总计 8 KiB；`stdin_text` 最多 16 KiB UTF-8，写完关闭标准输入。`timeout_ms` 为 1–86400000，省略则不增加执行期限。超时停止直接子进程，不保证清理全部后代；输出排空失败也会明确报告。
- 命令 `wait_ms` 默认 0，最多 30000，仅控制远端接收后的结果等待，不限制建立连接或提交的时间。等待内完成时附带 stdout/stderr 各最后 8 KiB；更早的内容通过 `pab_read_output` 读取。等待到期返回原任务引用，不表示执行失败。
- `pab_get_task` / `pab_get_operation` 支持 `wait_ms`、`wait_until: "change" | "complete"`、`after_revision`，返回 `revision`、`changed`、`wait_expired`。下一次查询带上上次的 revision；等待预算包含首个快照读取，但首次读取本身不会被强行取消。
- `pab_read_output` 支持 `max_bytes`（4–65536）、`tail_bytes`（1–65536，与 offset 互斥）、`contains` 和 `wait_ms`。使用 `next_offset` 续读；过滤只针对当前返回块的文本行，不保证跨块匹配，游标仍跳过全部已扫描字节。保留期导致的缺口通过 `gap` 报告；非完整 UTF-8 解码替换通过 `decoding_replacements` 报告。
- `pab_connect` 默认等待 5000 ms，可设置 0–30000。未完成时返回 `state: "connecting"` 和 `connection_ref`；再次调用相同设备复用正在进行的尝试，`pab_disconnect` 可停止等待中的尝试。单次后台连接最多约 120 秒，失败后返回明确错误，可重新发起。
- 工具错误保留原消息，同时返回 `error.code`、`phase`、`retry_action` 和可用的请求引用；网络结果未确认时应查询原操作，不自动换 ID 重做。

`env`、`stdin_text`、`timeout_ms` 需要目标 Executor 支持 command schema v2；旧端会在发送命令前被明确拒绝，不会静默丢弃参数。旧的普通命令保持兼容。以上是源码能力，升级 MCP 和目标 Executor 后生效。

### 示例：执行命令与读取结果

以下 JSON 是工具参数，不是终端命令。

调用 `pab_connect`：

```json
{ "device_code": "123456789" }
```

确认目标为 Windows 后，调用 `pab_run_command`：

```json
{
  "device_code": "123456789",
  "program": "powershell.exe",
  "args": ["-NoProfile", "-NonInteractive", "-Command", "Get-PSDrive -PSProvider FileSystem"]
}
```

Linux 目标可以使用 `program: "df"` 和 `args: ["-h"]`。
Bridge 不会隐式添加 Shell，使用 Shell 语法时需要明确指定解释器。

将命令结果中的 `task.task_ref.task_id` 和 `device_code` 传给 `pab_get_task`，
查询至任务完成。再调用 `pab_read_output` 读取标准输出：

```json
{
  "device_code": "123456789",
  "task_id": "TASK_ID_FROM_THE_COMMAND_RESULT",
  "stream": "stdout",
  "offset": 0
}
```

### 示例：异步传输

上传下载立即返回 `operation_ref`，后台继续连接和传输。可选 `wait_ms` 最多等待
5000 毫秒，它不会限制传输时长。两端路径均须使用各自系统的绝对路径，最多 4096 个字符。

```json
{
  "device_code": "123456789",
  "source": "C:\\work\\artifact.zip",
  "destination": "C:\\incoming\\artifact.zip",
  "overwrite": true,
  "request_id": "c746c0d6-f349-4e4d-92fa-9e3fb25abcf4"
}
```

将返回的引用传给 `pab_get_operation`，取消时使用相同参数调用 `pab_cancel_operation`：

```json
{ "device_code": "123456789", "operation_id": "c746c0d6-f349-4e4d-92fa-9e3fb25abcf4" }
```

相同 `request_id` 和参数会返回原记录，完成后或重启后重复提交也不会再次执行。
省略该参数会生成新 ID。`cancel_requested` 表示取消意图，`cancelled` 才表示已确认停止；
已经发布的文件仍为完成。`unconfirmed` 表示正在核对原请求，不能改用新 ID 重做。
每个 MCP 同时最多接受 8 个结果尚未确认的传输。传输须结束或核对出结果后再断开；
断开后的设备需要显式调用 `pab_connect` 重新连接。已退出会话的记录可查询，但其他会话不能取消。
缓存的系统环境会标记为 `remembered_device`。

重新构建安装并重启 AI 客户端后才能加载新工具。旧 MCP 安装包保留原工具集和行为；
使用文本工具还需要升级目标机器上的 Executor。

## 连接管理与活动展示

Desktop 和每个 MCP 进程分别维护设备连接。在 Desktop 中断开设备，
不会自动断开 Agent 的 MCP 连接；多个 Agent 也分别拥有自己的 Runtime。

默认访客模式下，每个 MCP 在用户数据目录的 `mcp-endpoints/guest-<槽位>.key`
中保留独立身份。操作系统文件锁保证运行中的进程不会占用同一槽位；正常退出或崩溃后
释放占用，后续进程可复用空闲槽位，连接重试则保持原身份。
Desktop 继续使用 `guest-endpoint.key`，设备记录和保存的密码仍共用原数据库。
MCP 运行期间不要删除槽位密钥或锁文件。手工配置账号模式（`PAB_MCP_GUEST=0`）时，
并发进程需分别配置已注册的 `PAB_ENDPOINT_SECRET_FILE`，占用中的密钥会明确报错。
安装此修复后，需要重启已有 MCP 进程才能生效。

Desktop 主进程在 `0.0.0.0:26035` 提供 Axum 状态服务：

| 入口 | 用途 |
|---|---|
| `GET /health` | 上报服务健康检查 |
| `GET /api/mcp` | 查询当前 MCP 进程快照 |
| `WS /ws/mcp` | MCP 注册、心跳和快照上报 |

MCP 通常连接 `ws://127.0.0.1:26035/ws/mcp`，上报客户端身份、进程与会话、
设备码及名称、连接阶段、P2P／Relay 路径，以及任务和传输摘要。
上报不包含密码、密钥、原始命令参数或命令输出；任务详情持久化在 SQLite 中。

该监听服务当前没有认证，用于状态汇总，不是 HTTP MCP 工具执行入口。
Desktop 重开后 MCP 会重新上报；状态服务不可用不会阻止 MCP 操作设备。

## 常见问题

**在线与已连接有什么区别？**

在线表示设备当前出现在控制服务中；已连接表示当前操作端 Runtime 已建立设备连接并完成认证。
设备在线时，Desktop 仍然可以处于未连接状态。

**Desktop 显示未连接，为什么 Agent 仍然可以操作？**

`pab_connect` 使用保存的凭据，为该 MCP 进程建立或复用连接。
Desktop 的连接状态属于自己的 Runtime，可以在左侧“MCP 连接”页查看 Agent 的活动。

**多个 Agent 可以同时操作同一台机器吗？**

它们分别使用自己的连接、任务 ID 和终端会话 ID，但操作仍发生在同一台计算机上。
同时修改同一个文件、向同一个桌面发送输入，可能互相影响；使用不同文件路径，并协调共享资源的操作。

**MCP 程序意外退出后怎么办？**

宿主支持重启 MCP 连接时使用其重启功能，否则重新打开 Agent 会话。
单独启动另一个 `pab-mcp` 进程，无法接回已经断开的 stdio 流，宿主自动恢复仍在研究中。

## 平台与实现状态

| 范围 | 当前状态 |
|---|---|
| Windows 桌面、命令、文件和终端 | 已实现，核心远程流程完成真机验证 |
| Windows 截图和输入 | 已验证登录后桌面，也验证了未登录截图和安全注意序列 |
| Linux 命令和文件 | 已完成真实远程 Linux 设备验证 |
| Linux 交付 | Executor + MCP 无界面版；当前不提供 Linux 桌面包，旧图形组件不在本轮交付范围 |
| macOS | 原生 app、launchd 安装包和后端已实现；ARM 自动化与 Intel 编译通过，图形及正式部署验收受权限/环境限制，见 [MACOS.md](MACOS.md) |
| Codex | 已验证一键注册和真实工具调用 |
| 其他 MCP 宿主 | 可手动接入 stdio，仍需逐宿主验证 |

当前不包含连续远程视频流、共享 Bridge Host 或 HTTP MCP 执行入口。
stdio 进程意外退出后的恢复依赖宿主，自动恢复仍在研究中。

## 开发与构建

### 环境要求

- Rust **1.95.0**，由 `rust-toolchain.toml` 选择。
- Node.js **20.x 的 20.19+，或 22.12+**，对应当前 Vite 要求，以及 npm。
- Python 3，用于打包和仓库脚本。
- 平台对应的 Tauri 依赖：Windows 需要 MSVC 构建工具和 WebView2，
  Linux 依赖示例见 `packaging/desktop/Dockerfile.linux`。
- Docker 和 Compose，用于服务端部署或 Linux 构建。
- 生成 Windows EXE 安装包时，将 NSIS **3.12** 放在 `tools/nsis`。

### 启动开发窗口

```sh
git clone git@github.com:PixelsCloud/PixelsAgentBridge.git
cd PixelsAgentBridge/apps/desktop
npm ci
npm run dev:desktop
```

原生 Tauri 窗口支持前端热更新。开发命令关闭 Rust 自动监听，修改后端后需要重启。
在普通浏览器预览页面不会获得 Tauri 原生设备接口。

### 构建 Windows 安装包

在仓库根目录安装前端依赖，然后统一编译并打包：

```powershell
npm --prefix apps/desktop ci
python scripts/build.py desktop --package
```

需要更换控制端和中继地址时，对同一批产物重新生成安装包：

```powershell
python packaging/desktop/build_nsis.py --profile debug --control-url "wss://control.example.com/control" --relay-url "https://relay.example.com"
```

执行前替换部署信息。输出位于 `.build/packages/`，包含
`pixels-agent-bridge-windows-x86_64-debug-<version>-setup.exe` 和校验清单。
安装、升级测试使用完整包；Release 打包需要对应程序和明确的 profile。

每次构建自动分配一个产品/安装包版本，首次 `1.2.0`，之后每次 patch 加1；
`1.2.99 → 1.3.0`，`1.99.99 → 2.0.0`。后续打包不再递增；每次生成的 EXE/PKG 安装包文件名都包含已核验的构建版本号。
仅安装包发布版本递增；所有 Rust、npm 和 Tauri 内部版本保持不变，不改写清单或锁文件。Cargo 根据源码变化增量编译，其余产物复用缓存。
完整构建入口及增量规则见 [BUILDING.md](BUILDING.md)。

### 检查命令

桌面 Rust 项目独立于核心 Cargo 工作空间：

```sh
python scripts/verify_iroh_vendor.py
cargo test --locked --workspace
cargo test --locked --manifest-path apps/desktop/src-tauri/Cargo.toml --lib
cargo clippy --locked --workspace --all-targets -- -D warnings
```

在 `apps/desktop` 执行 `npx tsc --noEmit` 检查前端类型，不消耗版本号。PostgreSQL 集成测试需要通过
`DATABASE_URL` 配置可丢弃的测试数据库；依赖特定环境或标记 ignored 的测试需额外准备。
这些是检查入口，不表示所有平台或可选测试均已通过。

## 服务端部署

Compose 包含 PostgreSQL 17、控制后端和 Relay。启动前：

1. 将 `packaging/docker/example.env` 复制为 `packaging/docker/private.env`。
2. 配置数据库密码、域名、证书路径和端口映射。
3. 准备与域名匹配的后端、Relay TLS 证书，以及需要的 CA 文件。
4. 创建环境配置指定的私有 Relay 控制密钥文件。

在仓库根目录运行：

```sh
docker compose --env-file packaging/docker/private.env -f packaging/docker/compose.yaml build
docker compose --env-file packaging/docker/private.env -f packaging/docker/compose.yaml up -d
docker compose --env-file packaging/docker/private.env -f packaging/docker/compose.yaml ps
```

后端和 Relay HTTPS 端口默认监听回环地址，便于接入反向代理；Relay QUIC 发布 UDP 7842。
PostgreSQL 位于私有网络，命名卷在普通容器替换后保留数据。
`PAB_RELAY_IMAGE` 可独立指定 Relay 镜像。

证书文件名、密钥、持久化和配置细节见 [Docker 部署说明](packaging/docker/README.md)。

## 仓库结构

| 路径 | 内容 |
|---|---|
| `apps/desktop` | React、TypeScript、Ant Design 和 Tauri 应用 |
| `crates/bridge` | 操作端 Runtime、持久化、CLI 和 MCP 工具 |
| `crates/executor` | 目标服务及远程能力 |
| `crates/server` | 中央控制服务和 PostgreSQL 数据模型 |
| `crates/relay` | Relay 及策略同步 |
| `crates/agent-core`、`protocol`、`transport`、`task-runtime` | 身份、协议、网络和任务基础模块 |
| `crates/platform`、`terminal`、`windows-sas` | 原生平台、终端和 Windows 安全注意序列 |
| `packaging` | 桌面安装包及 Docker 部署 |
| `patches/iroh-relay`、`vendor/iroh-relay` | 固定版本 Relay 补丁、校验资料和源码 |
| `diagram/workflow` | 对外工作流程 SVG 和 PNG |

更多说明：[开发文档](DEVELOPMENT.md)、[桌面应用](apps/desktop/README.md)、
[桌面打包](packaging/desktop/README.md)。

反馈或贡献时，请说明平台、操作来自 Desktop 还是 MCP、复现步骤，并提供移除凭据后的日志。
[提交 Issue](https://github.com/PixelsCloud/PixelsAgentBridge/issues)。
