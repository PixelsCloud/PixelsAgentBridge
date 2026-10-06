# E7 Linux 已安装设备的持久化验收

日期：2026-10-07。实际安装包 1.2.27；源码基线与上一份 E7 报告一致。
环境：`pab-execution-e8-linux`，Debian 12/systemd，无桌面，设备码 `565893930`。
持久数据卷：`pab-execution-e8-linux-data`，挂载到 `/var/lib/pixels-agent-bridge`。

## 方法与结果

使用已经向真实服务器注册、此前已通过原生 Pixels MCP 执行命令的容器。
在变更前以 SQLite 只读事务取得设备凭据及原任务的摘要，读取 endpoint key 和已安装二进制摘要。
原任务：`b8244140-ccd1-4446-a277-8ff1a44cef2a`。未输出密码、密钥或其摘要。

每一步完成后，要求服务 active，且产生时间晚于该步开始时间的 `Authenticated` heartbeat，
再比较基线。只检查进程存在或复用旧 heartbeat 不算通过。

| 场景 | 身份与凭据 | 原任务/事件/输出 | 二进制 | 服务器认证 |
|---|---|---|---|---|
| systemctl restart 服务 | 保留 | 完全一致 | 不变 | 新 heartbeat 已认证 |
| docker restart 容器 | 保留 | 完全一致 | 不变 | 新 heartbeat 已认证 |
| 相同 1.2.27 完整包重新安装 | 保留 | 完全一致 | 不变 | 新 heartbeat 已认证 |

重装前核对压缩包 SHA-256：
`d222ac23c1986f040a628704c35eeed7ce688395783ab94c9813f432e39e99e2`。
解包前校验路径留在临时目录且没有符号链接/硬链接，再执行包内 install.sh，沿用现有服务器地址。
结果为 active/running，NRestarts=0；临时安装目录已删除，没有安装 pab-desktop。
容器、数据卷和两个测试用户保留用于后续正式宿主验收。

本机原始布尔结果：`.build/linux-installed-persistence-result.json`；
一次性测试夹具：`.build/verify_linux_installed_persistence.py`。两者均未加入版本库。

## 边界

这是已完成任务的持久化及**同版本重装**测试，不是跨版本升级、运行中操作恢复或重复请求去重测试。
新 heartbeat 证明 Executor 与服务器重新认证，不代表当前 AI 宿主重连成功。
本次原生 `pixels.pab_connect` 实际返回 `Transport closed`：本机安装结束了旧 MCP 进程，
当前宿主尚未重载，仍只有旧工具目录。不能用独立 stdio 脚本替代 E8 正式工具验收。

剩余：新会话加载 68 个工具后，执行指定用户、文件/传输/Git/PTY 两轮工作流以及运行中恢复矩阵。
