# E8 Linux 正式工具首轮（部分完成）

日期：2026-10-07；已安装 1.2.27；设备 `565893930`，Debian 12/systemd，无显示环境。
本轮所有远程操作均通过本会话原生 `pixels.pab_*` 调用。
用户发现返回的连接绑定上下文选择 `pabuser1`（UID/GID 23001），工作目录为：
`/home/pabuser1/pab-e8-native-20261007-pabuser1-round1`。

## 已验证

- 命令与交互终端实际 UID=23001、home/cwd=/home/pabuser1，终端中文文件内容正确；resize 100×30 确认。
- 文件创建、UTF-8 读取、带 expected_hash 的精确中文补丁均成功，回执带实际用户身份。
- 在专用 repo、peer、origin.git 夹具中完成原生 status/diff/log/commit/checkout/fetch/pull/push 八种工具调用。
  仓库初始化、peer clone 和本地 fixture 作者配置仅用于准备测试，未改用户全局配置或真实远端。
- 首次提交 `7e868a33ae751cd1453e0de4a94eaa099cf85525`，peer 提交 `2ed526f5c0f1cd0b3434938d125f6d9e0a050866`，
  补丁提交 `642a27257f24fc2d815b4e79b95b8701b37d64a2`。分离检出原始提交再切回 main，最后工作区干净。
- 上传、下载同一 262,144 字节二进制夹具，SHA-256 均为
  `2312394bd99545d9de131c24efb781e765ac1aec243f2ed9347597a793a415e9`。
- 最终原生用户命令核对测试树 168 个路径的实际所有者均为 UID 23001，中文文件和终端写入文件内容正确。

关键原生操作 ID：

| 操作 | ID |
|---|---|
| 第一笔 Git push | 404fa595-ffb0-4e07-aff6-56e5f0f01485 |
| 从 fixture 远端 ff-only pull | d4fd7e68-dd25-4241-8e47-beee197b5117 |
| 中文 expected_hash patch | 0825c763-10ab-4a00-8a02-135a6e06164e |
| 最终 Git status | a409eeb3-4227-4f80-a22b-2f35869b7fe7 |
| 上传 | ea9d9319-141f-49b9-9ecd-9bf3c4170452 |
| 下载 | 54752235-4c88-48c0-b59d-534831480059 |
| 所有者及文件核对 | 2563545e-8716-4040-b4cb-d6814a34deff |

本机完整回执保存在忽略目录 `.build/e8-native-linux-first-round-evidence.json`，不包含设备密码或密钥。

## 失败项与接续

终端 `d7a86737-f8f9-4a3f-ba5d-a2a588db8bcc` 的 close 返回 NotFound，Executor 实际已 closed。
根因、源码修复和三平台回归见 [终端关闭报告](execution-e8-terminal-close-2026-10-07.md)。
因此本轮不能记为完整通过，不是两轮全部完成的证据，也不覆盖真实网络 Git 凭据验证。

容器、数据卷和本测试树暂时保留，用于新包升级后的身份/数据核对和正式复验，最后再通过原生文件工具清理。
剩余包括 service/pabuser1/pabuser2 各两轮完整工作流、运行中取消/断线/重启及最终安装后验收。
