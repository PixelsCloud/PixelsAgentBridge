# Pixels 专属浏览器 V1：无人值守开发检查点

日期：2026-10-12。当前测试包版本：Windows Company 与 Mac 测试机为 1.2.91，90 测试机为 1.2.84，其余已安装设备仍为 1.2.83（均未公开发布）。编译并行度限制为 18。本机保持已安装版本，不覆盖安装，以免影响其他会话。

## 本轮完成

- 浏览器资料按目标 OS 用户、Pixels 账号、Chrome/Edge 分开。运行时持有跨进程资料锁；客户端“浏览器”设置页可以清除当前账号选定浏览器的资料。占用中拒绝清除，账号在确认期间变化也会拒绝。
- 登录接管增加取消状态，取消后 MCP 会话状态为 `cancelled`，`resume` 拒绝继续；工具说明要求用户直接在 Pixels 浏览器中输入密码、验证码并在恢复后核验网站状态。
- 新增本地站点工具烟测，覆盖表单、弹窗、上传下载、截图、标签页、拖放、网页日志、网络摘要和 PDF；下载事件与实际文件同时校验。持久资料测试增加第二份资料隔离和网站主动退出登录。
- Windows/macOS 打包的浏览器运行时仅包含固定版本 Node 和 Playwright MCP，不包含扩展。两平台安装包已构建、校验并覆盖安装。
- 所有 MCP 工具组默认开启，包括 5 个浏览器增强工具。旧版设置升级时会加入两个浏览器工具组；新版设置中用户明确关闭的工具组仍保持关闭。
- 浏览器工具的 `request_id` 改为可省略，由 MCP 自动生成 UUID 并通过 `operation_ref` 返回；显式 UUID 仍可用于安全重试。
- Playwright 运行时允许读取目标交互用户可访问的文件，避免普通文件上传被工作目录限制拒绝；后续再细化文件访问权限。
- 登录接管提示移至居中弹窗；接管开始时即使 Pixels 窗口隐藏，也会主动显示并切到前台。
- 点击取消时先显示二次确认；选择继续等待不会改变会话，只有再次确认才会取消。确认取消后浏览器帮助进程自动结束当前会话；服务端检查取消标记并解除桌面操作保护。等待 1 小时超时后自动结束会话，向 Agent 返回 `timed_out`；客户端显示超时提示。帮助进程断开时按进程移除服务端等待记录，旧进程的标记不再触发客户端弹窗。

## 已通过的验证

| 范围 | 结果 |
| --- | --- |
| Windows Chrome、Edge | 独立资料登录保持、第二资料隔离、退出登录和工具烟测通过 |
| macOS Chrome | 同类登录保持、隔离、退出登录和工具烟测通过；Edge 未安装，回退路径已在 Windows 验证 |
| 资料锁与清除 | Windows/macOS Rust 单测各 3 项通过；在资料被占用时拒绝清除，另一个账号资料保留 |
| 协议与 MCP | 浏览器协议 4 项、MCP 工具定义 1 项通过 |
| 前端与安装脚本 | TypeScript 检查、打包测试、Windows 覆盖安装/回滚脚本测试通过 |
| 工具默认值 | 设置迁移、99 个默认工具目录和 MCP 启动后目录测试通过 |
| 设备覆盖安装 | 本机和三台 Windows、Mac 均安装 1.2.83；Windows 注册版本与服务、浏览器运行时核验通过；Mac 安装成功，服务运行且 MCP/Executor 二进制与构建产物哈希一致 |
| 正式远程路由 | Windows 与 macOS 成功启动专属 Chrome；Windows 90 测试机通过页面读取、查找、表格提取、点击、表单、悬停、拖放、坐标点击、弹窗、上传、下载、截图、PDF、标签页和前进后退。登录接管已验证会暂停浏览器操作，等待真人完成 |
| 自动请求编号 | 新构建的 MCP 二进制经 `tools/list` 和省略 `request_id` 的调用验证，工具目录不再强制 Agent 提供 UUID |
| 90 测试安装 | 1.2.84 测试包的远端 SHA-256 一致；静默覆盖安装成功，计划任务退出码为 0，注册表版本与服务运行状态正常 |
| Company 登录接管 | 1.2.83 中模拟登录由真人确认后，Agent 恢复并读到已登录页面；关闭并重开专属 Chrome 后登录状态仍保留 |
| Company 弹窗验收 | 1.2.85 测试包覆盖安装成功；Pixels 从隐藏状态自动显示并切到前台，用户确认居中弹窗可见；点击弹窗确认后 Agent 恢复并读到已登录页面；测试浏览器、服务与安装任务已清理 |
| Company 取消与异常中断 | 1.2.86 测试包覆盖安装成功；模拟取消后 `get_session` 返回 `cancelled`，`resume` 明确拒绝；强制结束专用帮助进程后，新进程自动恢复并能建立新浏览器会话。测试残留已清理。弹窗“取消”按钮尚未由真人点击验收 |
| Company 登录等待与二次取消 | 1.2.87 测试包覆盖安装成功，注册表版本 1.2.87、安装任务退出码 0、Executor 服务运行；登录等待改为 1 小时，等待期间不受普通会话时长提前终止。前端自动化测试证实第一次点击取消不会发出取消请求，继续等待保留会话，二次确认才取消 |
| Company 知乎真实登录 | 通过正式 Pixels MCP 浏览器路由打开 `https://www.zhihu.com/hot`，未登录时跳转知乎登录页；用户在专属 Chrome 完成登录并在 Pixels 弹窗确认后，原会话恢复并读到热榜。关闭浏览器会话、重新 attach 后再次访问热榜，无需重新登录。测试后关闭浏览器会话；仓库未保存账号或登录资料 |
| 弹窗选择传给 Agent | 知乎测试中，在收到用户的聊天回复前，`pab_browser_get_session` 已返回 `ready_to_resume`。重复的聊天选择是 Agent 多发的确认，不是状态传输缺失；MCP 工具说明现明确要求只提示一次，并以 `get_session` 返回的确认、取消或超时状态为准，不要求用户再回复聊天 |
| Company 知乎重测与最大化 | 在 Pixels 专属 Chrome 内通过知乎页面的“退出”正常登出，仅影响知乎会话；重访热榜跳到登录页。1.2.88 测试包覆盖安装成功，注册表版本、安装任务与服务状态正常；新开的专属 Chrome 窗口自动最大化。用户在 Pixels 弹窗确认后，Agent 未要求聊天回复，直接读到 `ready_to_resume`、恢复会话并重新读取热榜。关闭再重开专属浏览器后仍直接进入热榜，登录状态保留；最终测试会话已关闭 |
| Windows 浏览器黑窗口 | 1.2.89 在 Company 覆盖安装成功，服务运行正常。启动 Pixels 专属 Chrome 后，`browser-runtime/node.exe` 进程正常运行，窗口列表只有最大化的 Chrome，没有旧版出现的以 `node.exe` 为标题的 Windows Terminal 窗口。测试浏览器会话已关闭；macOS Accessibility 错误码已改用 `accessibility-sys` 的命名常量 |
| Company 登录接管与取消 | 通过正式 Pixels MCP 路由打开本地模拟登录页；用户点击二次取消后，`get_session` 返回 `cancelled`。再次登录并在弹窗确认后，`resume` 成功，重新打开专属浏览器仍保持模拟站点登录。正式路由脚本两次通过 attach、导航点击、关闭和登录保持检查 |
| Company 安装故障与恢复 | 卸载重装验收中，旧版静默卸载停在临时 `Un.exe` 进程，该进程占用 NSIS 安装锁；卸载脚本已移除 Executor 服务，导致设备离线。源码的静默失败分支会弹出不可见的错误框，这是阻塞原因的推断，具体 PowerShell 异常未捕获。结束核实的临时卸载进程后恢复服务；通过交互式计划任务完成 1.2.89 覆盖安装。1.2.90 修复版让静默失败返回错误码并记录日志，卸载失败时尝试恢复服务；Company 正常覆盖安装成功，安装任务返回 0，注册表版本为 1.2.90、服务运行且浏览器资料保留。卸载失败恢复逻辑通过隔离故障注入测试，未重试完整卸载验收 |
| Windows 卸载根因与最终复测 | 1.2.90 完整卸载返回 1，新增日志确定失败于运行中的 `pab-mcp.exe` 被帮助进程重启并占用，删除时访问被拒绝；失败恢复让 Executor 保持运行。1.2.91 卸载先移开程序启动路径，再停止并清理旧映像，阻断重启竞争。Company 的完整静默卸载、新装退出码均为 0；注册表版本 1.2.91、Executor 运行，安装目录无旧映像，系统无残留 NSIS 卸载进程。重装后正式 MCP 路由的浏览器启动、导航、点击、关闭与登录保持全部通过 |
| Mac 最终包及新装 | 1.2.91 arm64 PKG 构建完成，安装前 SHA-256 校验通过；覆盖安装后安装的 Desktop、MCP、Executor 与构建产物逐字节哈希一致，应用签名验证通过，真实图形会话的正式 MCP 浏览器路由通过。随后一次性 `KeepAlive=false` 的独立任务完成完整卸载与新装，卸载检查无应用文件残留、安装退出码 0、Executor 恢复运行；重装后正式路由再次通过且模拟网站登录仍保留。第一次临时 `launchctl submit` 安装任务意外重复执行，发现后已移除并确认不再运行；最终新装使用不会重启的任务完成 |
| Mac 知乎扫码登录 | 通过正式 Pixels MCP 路由打开知乎登录页并暂停会话；等待期间浏览器操作被拒绝。用户在 Mac 专属 Chrome 扫码并点击 Pixels 居中弹窗后，`get_session` 返回 `ready_to_resume`；`resume` 直接读取到已登录首页。导航至热榜显示已登录账号状态；关闭浏览器再重新 attach 并访问热榜，仍保持登录。最后关闭测试浏览器会话；仓库未保存账号或登录资料 |
| Company Edge 正式路由 | 未卸载或修改 Chrome。仅在忽略 Git 的 1.2.92 临时测试包中让 Edge 优先，覆盖安装退出码 0；正式 `pab_browser_attach` 返回 `browser=edge`，本地测试页导航、读取、点击后的计数变化、关闭重开及模拟网站登录保持全部通过。正式路由脚本输出 `PIXELS_ATTACH_OK`、`PIXELS_NAVIGATE_CLICK_OK`、`PIXELS_CLOSE_OK`、`PIXELS_LOGIN_PERSISTENCE_OK`。随后从哈希核实的 1.2.91 包覆盖恢复 Chrome 优先，正式 attach 返回 `browser=chrome`；注册版本 1.2.91、Executor 运行，测试浏览器、站点进程与计划任务均已清理。临时任务初次设置了稍后的单次触发，因手工立即运行又在预定时间运行一次；发现重启后已移除任务，后续恢复任务不设触发器 |

## 测试安装包

- Windows：[1.2.83 测试安装包](../.build/packages/pixels-agent-bridge-windows-x86_64-release-1.2.83-setup.exe)，SHA-256 `f4f7e2c2527972e7a126adc7e887eb8e8278607f3959abefff8e2a97db369e3d`。
- Windows 90 测试机：[1.2.84 测试安装包](../.build/packages/pixels-agent-bridge-windows-x86_64-release-1.2.84-setup.exe)，SHA-256 `f0da6c074bb3bd1977cb9013dbe5c787ed6128121608c43ffd4ca93989f33704`。
- Windows Company 测试机：[1.2.85 测试安装包](../.build/packages/pixels-agent-bridge-windows-x86_64-release-1.2.85-setup.exe)，SHA-256 `b95cc0fb5736b04ddc7a2eb3982601f8f30f1ee60f0ee461a75d7aad8b8fe1f8`。
- Windows Company 测试机：[1.2.86 测试安装包](../.build/packages/pixels-agent-bridge-windows-x86_64-release-1.2.86-setup.exe)，SHA-256 `6a0af06135fbe80b9b3dc5f9af467adbc4205bccc6f79b1423e0219c609892e3`。
- Windows Company 测试机：[1.2.87 测试安装包](../.build/packages/pixels-agent-bridge-windows-x86_64-release-1.2.87-setup.exe)，SHA-256 `cbe6bdfa71a4a2dbf615a86f7be7493c4b4b828226c9f144902e92474b45cf2a`。
- Windows Company 测试机：[1.2.88 测试安装包](../.build/packages/pixels-agent-bridge-windows-x86_64-release-1.2.88-setup.exe)，SHA-256 `e61d1e063e84f969edc1a75b8397612fc547ba2d13fad5bff871c33a7bca01eb`。
- Windows Company 测试机：[1.2.89 测试安装包](../.build/packages/pixels-agent-bridge-windows-x86_64-release-1.2.89-setup.exe)，SHA-256 `91fa6bd2aa7927ad110e9722045ae6dbc60263b04f47dd56f848a2362152eafc`。
- Windows Company 安装修复：[1.2.90 测试安装包](../.build/packages/pixels-agent-bridge-windows-x86_64-release-1.2.90-setup.exe)，SHA-256 `08bcfe37d510c0333dff8e50536fe10d14525085f2c912d419c177bd40bcfc71`。
- Windows Company 最终验收：[1.2.91 测试安装包](../.build/packages/pixels-agent-bridge-windows-x86_64-release-1.2.91-setup.exe)，SHA-256 `e404f28f241bfa296eed53a3d877be6761493376604c5bdebfe175ecf7b385e0`。
- Windows Company Edge 路由专用：[1.2.92 临时测试安装包](../.build/packages/pixels-agent-bridge-windows-x86_64-release-1.2.92-setup.exe)，SHA-256 `e6a4300253a670aa7d5a08c5ca6a7b1db1ab0c5d536862b8e3f62d7a4274a4c9`；仅改变浏览器选择顺序，未暂存或发布，Company 已恢复 1.2.91。
- macOS arm64：[1.2.83 测试安装包](../.build/packages/pixels-agent-bridge-macos-aarch64-release-1.2.83-setup.pkg)，SHA-256 `5301f81230edc42d33b9cadd89e13aa8422c1dbeb8a3405f7c0aa13f0671042c`。包未使用 Apple Developer ID 签名或公证；内部应用沿用项目本地自签名流程。
- macOS arm64 最终验收：Mac 独立测试源码 `.build/packages/pixels-agent-bridge-macos-aarch64-release-1.2.91-setup.pkg`，SHA-256 `95ef7359a71a5766dc43c3ace6c70286d5071ac3a28d40733bf9793624e70f84`。PKG 未购买 Developer ID 签名或公证；内部应用使用本地签名并通过验证。

安装包存放在忽略 Git 的 `.build/packages`；文档链接只在本工作区有效。macOS 构建使用测试机上的独立源码副本，没有修改其原项目目录或现有安装。仓库新文档和测试脚本未记录私人设备码。

## 验收结论

- Windows 与 Mac 的正式 MCP Chrome 路由、人工登录、完整卸载、新装、覆盖安装及安装后回归已通过；Company 的正式 MCP Edge 路由也已通过关键流程。Company 和 Mac 的知乎真实登录及关闭重开后保持登录已通过。按用户要求，1 小时等待只做代码检查，不实机等待满 1 小时。
- Chrome 通过正式路由的完整工具矩阵，Edge 通过独立运行时完整探针和正式路由关键流程；没有逐项重复 Edge 的 30 个工具正式路由矩阵。
- Mac `rust-objcopy` 缺少本机 `libLLVM.dylib` 仅产生调试信息剥离警告；构建成功且最终安装包、程序签名与运行均已验证。
