# E6 macOS 指定用户 Git 与独立 Keychain

日期：2026-10-07。源码基线 `58c8ca4`；Mac ARM64 / macOS 27，服务 root → huayang / UID 501。
本次是源码集成和原生用户命令验证，不替换已安装包或用户的凭据配置。

## 选型与隔离

先检查 [Git 当前 osxkeychain 实现](https://github.com/git/git/blob/master/contrib/credential/osxkeychain/git-credential-osxkeychain.c)，
其入口没有独立 Keychain 路径参数。测试复用 [Git v2.44.0 的独立 C 助手](https://github.com/git/git/blob/v2.44.0/contrib/credential/osxkeychain/git-credential-osxkeychain.c)，
运行时编译，只把默认 Keychain 参数改成显式引用，并由包装入口禁用系统交互。没有自行实现密码存储或认证，也不向产品打包此测试助手。
Git 与 Apple Security 的库调用完成真实凭据读取；当前用户的系统 Git、credential helper 和登录钥匙串均不替换。

测试源及其 COPYING 由官方仓库下载到忽略目录，编译前核验：

- C 源 SHA-256：`60be5c75ad00aa0f554cd302c8b83127ffdcd59426074080c3843d6ecd781029`。
- COPYING SHA-256：`5b2198d1645f767585e8a88ac0499b04472164c0d2da22e75ecf97ef443ab32e`。

`scripts/execution_keychain_acceptance.py` 在用户 home 的专用目录创建独立 Keychain、生成凭据与解锁密码、
编译临时 helper，并启动仅监听 127.0.0.1 的 HTTP Basic 认证 Git 夹具。
HTTP 服务只提供专用 bare 仓库，拒绝越出目录的路径，GET/HEAD 都检查认证；不暴露 Keychain 文件或解锁文件。
用户默认 Keychain、搜索列表在创建后和清理后均独立比较，保持一致；没有修改 TCC。

## 系统探针

通过正式 `pixels.pab_run_command`，显式选择 UID 501 调用 Apple Security API。
原生任务 `b5a0591a-bebc-444f-b90c-6601349f4676`，退出码 0：

- 未锁定读取与生成的凭据一致。
- 锁定后读取被系统拒绝，OSStatus 为 `-25293`；系统交互已禁用。
- 使用仅属于测试 Keychain 的密码解锁后，读取恢复。
- 专用 Keychain 删除，默认 Keychain/搜索列表保持一致。

## 产品 Git 路径

新增 `native_user_git_keychain_acceptance`。root 测试宿主查询用户上下文、选择 UID 501，
通过实际 user-worker 创建客户端仓库及执行 Git；credential.helper 只写入本次测试仓库，并先清空继承的 helper 列表。
HTTP 服务每次核对真实 Authorization，测试核对拉取 OID 等于夹具提交。

| 场景 | 结果 |
|---|---|
| Keychain 未锁定 | 原用户 fetch 完成，OID 一致 |
| 仅锁定独立测试 Keychain | fetch failed，保留 terminal prompts disabled 诊断，不伪装为 repository not found |
| 解锁该测试 Keychain | 新 fetch 完成，OID 一致 |
| 状态与身份 | 三个结果都包含实际 UID 501；单次在 20 秒界限内结束 |
| 结果隐私 | 完整结果 JSON 未包含生成密码的唯一前缀 |

首次集成任务 `b9ae9a7b-26bc-4100-997a-049544dc095f` 全部通过，2.07 秒，退出码 0。
为 fixture 的编译、Git、helper 和偏好读取补充超时后，最终任务
`cbabdc8c-cbc6-401f-a559-4857688e8ab4` 再次全部通过，2.18 秒，退出码 0。
每轮 HTTP 服务均观察到 7 次成功认证请求；锁定前后控制命令也核对原 UID 和系统锁定状态。

服务任务分别为 `7b8beeaa-b05d-4947-9054-a84992b6b7c4` 和 `5812d550-c53a-4c5d-b7e7-bbe4f7bb4558`。
每轮完成后才发送 stop 标记，等待服务正常关闭、删除专用 Keychain、密码文件、仓库及临时编译产物；
两轮均确认 `fixture_removed=true`、`keychain_preferences_unchanged=true`。

## 限制与接续

这是显式独立 Keychain、禁用交互的测试 helper，不是任意版本系统 osxkeychain、登录 Keychain、
第三方凭据管理器或所有 GUI 认证提示的兼容性声明。产品不会自动解锁用户钥匙串。
需要交互而未返回的凭据进程，其取消/超时清理另见
[慢凭据程序验收](execution-e6-slow-credential-2026-10-07.md)。
本次未新增产品 Keychain 后端；Git 继续使用目标用户自己的凭据助手。
Mac 仍在锁屏，桌面应用完整工作流尚未完成；新包安装及正式 MCP 全流程验收仍待进行。
