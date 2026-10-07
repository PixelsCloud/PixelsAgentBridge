# E8 Windows 安装版界面与源码回归（2026-10-07）

## 范围与结果

实际运行 `C:\Program Files\PixelsAgentBridge\pab-desktop.exe` 1.2.29，使用独立 WebView
浏览器配置目录，但读取原有 Bridge SQLite 和终端归档。没有注入 Tauri invoke、假任务或替换安装资产。
正常关闭原主窗口后测试，Supervisor/Helper 保持运行；测试结束后正常关闭测试主窗口并重新启动正常安装入口。

- 英文、简体中文、繁体中文 × 亮色/暗色共 6 组完成设置切换、实际任务选择与截图。
- 实际 Linux 终端任务 `ed64c82b-4c65-4da6-9de9-60c6e6776a4d` 显示执行身份 `pabuser1`、
  account ID `uid:23001`，终端归档可见。
- 窗口宽度与文档宽度均为 1000px，无整页横向溢出；没有页面异常。
- 语言实际发生切换时，语言和主题偏好均正确持久化。
- 证据在忽略目录 `.build/execution-installed-ui-1.2.29/` 的 `verify.cjs`、`result.json` 和 12 张截图。
  截图仅设置和任务记录，不包含首页密码。

## 安装版发现的问题与源码修复

1. 首次按系统语言显示英文时，再选择 English 不触发 Select 的 `onChange`，
   `pab.language` 仍为空。源码改为 `onSelect`，明确选择当前值也能保存。
   安装版矩阵为完成其余检查，先切到另一种语言再切回；不能把矩阵通过当成此问题已在安装版修复。
2. 任务列表可见宽度约 299px，但长标题将隐式 Grid 列撑至约 400px，右侧状态被截断。
   源码将列表列宽设为 `minmax(0, 1fr)`，文字区可收缩并省略，图标和状态不收缩。
   适用于主任务页和设备内任务记录。

新增浏览器用例直接渲染 SettingsPanel/ActivityPanel，原生设置与历史为确定性夹具：

- 3 种语言选择当前默认值后保存并在刷新后保留。
- 3 种语言 × 2 种主题，主任务列表和设备内列表面对超长路径时，卡片不横向溢出、标题不覆盖状态。
- 新增 9 项与原有执行上下文 UI 11 项全部通过；TypeScript/Vite 前端构建通过。

这两项修复尚未进入新的安装包，不代表安装版复验完成；Mac 实际界面也尚未验收。

## 调试环境与清理

Elevated WebView2 忽略环境变量调试参数，因此仅为测试进程临时设置 HKLM WebView2 的
`AdditionalBrowserArguments\pab-desktop.exe`，CDP 监听 `127.0.0.1:19349`。
进程启动成功后立即在 finally 中移除此值；测试结束检查无该值、无监听端口，测试主进程正常退出。
依据：[Microsoft WebView2 security](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/security)。

删除独立浏览器配置目录和空注册表键的整条清理命令被自动审批策略拒绝，未执行。
已改为正常关闭窗口和只读核验；测试配置目录与空键保留，没有持续调试监听。
原用户浏览器语言/主题配置未修改。

## 尚待完成

本次没有覆盖完整双轮业务流程、Mac 安装 UI、Windows helper 完整故障注入或新版 MCP 的大输出并发归档。
本机升级后原 MCP 已退出，当前工具调用仍返回 `Transport closed`；需会话重新加载安装好的 MCP 后继续正式远端验收。
