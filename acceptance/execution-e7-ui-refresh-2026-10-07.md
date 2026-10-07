# Windows 1.2.32 完整包、本机升级与安装 UI 复验

## 构建

磁盘清理已释放 164.77GiB；源码、未提交修改、安装包和运行中测试服务保留。
先前 1.2.31 因空间不足失败，未发布、未安装，版本计数未回退。

通过统一入口 `python scripts/build.py desktop --profile debug --package` 分配 1.2.32，
设置 `CARGO_BUILD_JOBS=6`、`CARGO_INCREMENTAL=0`。完整重建 Executor、MCP、前端和 Desktop，
生成 Windows ZIP/NSIS，整体退出 0。日志 `.build/execution-ui-refresh-1.2.32.log`。
关闭增量缓存仅用于本次构建环境，没有修改产品代码或默认构建配置。

| 产物 | 字节 | SHA-256 |
|---|---:|---|
| pixels-agent-bridge-windows-x86_64-debug-setup.exe | 29591276 | a9c9514fa3c85b1f42194ec8d269745e6c4205466654297cba8622f0f56560c6 |
| pixels-agent-bridge-windows-x86_64-debug.zip | 48074637 | 7156233d5bf0f0e09d5a572b806781eb26e8e46317adf4d92f03e7e3825ef4d8 |

三个程序的构建哈希：

- Executor：`715fc6de817b2be5e23c37c3cacd6df09e13fc3c450771cd43357128a17c19c0`
- MCP：`286987a2fbf4817f5e8a8aa333ed246aa37d7d55f24cee697428cfea673e704b`
- Desktop：`b6d00689ea922b5ad994720d9b35595a5c01c301f502d0149be201a535291bd8`

## 本机安装

使用完整 NSIS `/S` 安装；一次性驱动在执行前校验安装包和构建版本，存在原结果时拒绝重放。
安装器退出 0，安装后三个程序的哈希与构建清单全部相同；endpoint key 保留，Executor 服务 Running。
结果 `.build/local-refresh-1.2.32/result.json`。

## 安装后界面

实际启动 Program Files 中的 Desktop，确认 ProductVersion 1.2.32。
采用独立 WebView 配置，读取原有 Bridge 数据库和终端归档，没有替换页面资产或注入原生调用/假任务。

- 系统默认英文、`pab.language` 最初为空；直接再次选择 English 后成功保存，不再依赖切换到另一种语言。
- 英文、简体、繁体 × 亮色/暗色共 6 组，语言及主题偏好保存正确，无页面异常。
- 实际任务 `ed64c82b-4c65-4da6-9de9-60c6e6776a4d` 的终端归档和执行身份
  `pabuser1` / `uid:23001` 均正常展示。
- 六组中任务列表 clientWidth/scrollWidth 都为 299px，所有任务行和右侧状态位于可见列宽内。
  修复前卡片约 400px，状态被截断；本次实际安装版已复验修复。
- 窗口文档宽度和视口均 1000px，无整页横向溢出。另人工查看英文亮色和繁体暗色截图。

证据 `.build/execution-installed-ui-1.2.32/` 中的 `verify.cjs`、`result.json` 和 12 张截图。
只截取设置/历史页面，没有截图首页密码。

测试主窗口已正常关闭，调试端口 19349 已退出，临时 HKLM WebView2 参数已移除，正常窗口已恢复。
删除临时浏览器目录的收尾命令被自动审批策略拒绝，因此保留该测试目录；没有用其他方式绕过删除限制。
具体状态见同目录 `cleanup.json`，原用户浏览器配置保持。

## 未覆盖与接续

- 本轮只升级本机。Windows 90 和 Linux 仍是 1.2.29；Mac ARM 仍为 1.2.30，Intel 仅构建/签名检查。
- Mac 仍需获取 UI 修复并完整打包、安装及实际界面复验，不能以同一前端源码替代 Mac 安装验证。
- 本机更新前后当前 Pixels 工具通道未重新加载；`Transport closed` 仍阻止正式远端调用。
  需重启会话后继续三平台两轮完整工作流、新 MCP 大输出并发终端、Windows helper 生命周期和剩余 E6 异常矩阵。
- 本次完成两项 UI 安装回归及本机包交付，不代表 E6/E8 全部完成。
