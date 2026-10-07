# E8 会话重载后的正式工具复验

日期：2026-10-07。所有远程操作使用当前 AI 宿主的原生 `pixels.pab_*`，没有另启 stdio 客户端。
本机 MCP 已加载安装版 **1.2.32**（PID 70500，文件哈希与安装清单一致）。
Windows 90（211399447）Executor 为 1.2.29，Mac ARM（603527578）为 1.2.30，
Debian 12 无界面测试端（565893930）为 1.2.29。三端均成功重新连接并核验 OS/账户上下文。
本轮验证连接、终端关闭修复及 Linux 用户工作流增量，不代表整个 E8 已完成。

## 已安装终端的大输出及并发关闭

使用各平台指定用户打开真实 PTY，生成超过 640 KiB 的输出与中文末尾标记，确认输出准备完成后，
并发调用 `pab_terminal_read` 和 `pab_terminal_close`。读取先取 32,768 字节，close 必须归档剩余输出。
核对本机真实 `.bin` 归档和只读 SQLite 历史，四个会话均 closed=true，历史 completed，
记录的 offset/size 与实际文件长度相符，中文末尾存在。远程 ready 文件均通过原用户文件工具删除。

| 平台/用例 | 会话 ID | 归档字节 | 内容核验 |
|---|---|---:|---|
| Windows Administrator / 连续字符 | 18dac335-eb53-4c4e-842b-837b7f900111 | 742456 | 末尾标记存在；ConPTY 重绘产生重复字符，不以原始字符总数判断完整性 |
| macOS UID 501 | 44345f72-4ff0-4e31-a123-45822b8e37c7 | 701279 | BEGIN/END 之间 700003 个 Q，中文末尾正确 |
| Linux UID 23001 | 5bce3856-3992-40d2-a5c4-88ebded5c34d | 700382 | BEGIN/END 之间 700003 个 Q，中文末尾正确 |
| Windows / 编号行复核 | e70c1a79-c11c-43ca-a267-7fe001a4b799 | 749867 | 去除 ANSI CSI 后，0–6999 共 7000 条完整编号行，缺失 0，中文末尾正确 |

归档 SHA-256（与上表同序）：

```text
1169a53a913d238628fa045bbf83a4c964177bfaddf0faeaa13d3381265caefc
33ef0a302441b703699886363f8b63b71439baf870defcf5b51ce01480b86add
c5d6a00786fa664b0ecf2bf94cc2c15d5e743ab5c671ff667e9b40bcb69d2325
743ff9b75961c87728a0d6d4f8ca76ccfb762af5a75be9c70c7702f160f223a7
```

Windows shell 为 powershell.exe -NoLogo -NoProfile，Mac 为 zsh -l -i，Linux 为 sh -i。
真实身份分别为 Administrator（SID 尾段 500、WTS 1）、UID 501、UID 23001。
终端工具返回 session_id，不能把该 ID 当作通用 get_operation 的 operation_id；本轮归档查询使用 Desktop 本地历史。
可提交的检查结果为 [安装版终端结果](execution-e8-terminal-installed-2026-10-07.json)，
本机完整原始回执位于 `.build/execution-native-terminal32-receipts.json`。

## Linux 第二个普通用户：业务流程第一轮

UID/GID 23002，home=/home/pabuser2，无 DISPLAY/WAYLAND_DISPLAY。
独立测试树 `/home/pabuser2/pab-e8-132-u2-r1-17913636`，未修改用户全局 Git 配置或真实远端。

- 原生用户命令与终端核对身份；PTY resize 110×35、中文文件写入、输出读取和 close 均通过。
- mkdir/write/read、expected_hash 中文 patch、search、copy/move/hash、ZIP 创建/解压、目录列表均通过。
  search 指定 max_depth=2，遇到 .git 深度限制，回执明确 truncated；不当作完整无界搜索。
- 原生八种 Git 工具完成 status/diff/log/commit/checkout/fetch/pull/push。
  本地隔离 bare remote 与 peer 模拟远端新增提交；ff-only pull 后工作区干净。
  首提交 732422ba7a4714896dcd7e3ca2be2f2614cb3ca7，修改提交 5cbd549ffdc55a0d7629e733606cb41b03ab8748，
  最终 HEAD a1fe458365cf83c198d002443b588c49af628725。
- 上传、下载 262144 字节二进制，远端及本机 SHA-256 相同：
  `2312394bd99545d9de131c24efb781e765ac1aec243f2ed9347597a793a415e9`。
- 最终实际检查测试树 180 个路径均属于 UID 23002；中文修改、peer 文件、解压文件和终端文件内容正确。
- 原生递归删除删除 180 个条目，completed、partial=false，测试树已清理。

| 证据 | ID |
|---|---|
| 上传 operation | 3743d1a3-0bb5-4da4-b49f-bb2bc225e4bb |
| 下载 operation | d3b59c5e-6201-4b98-992d-7522fe22b7f0 |
| 终端 session | 41c7e9c6-5d4d-4668-8ec9-49980a45d6ec |
| 最终归属/内容检查 task | 832feb90-d8f6-41f6-9737-00ca9153ce9c |
| 清理 operation | 41465eaa-cb26-4e8c-80d2-bd251f972ef6 |

完整原生回执保存在 `.build/execution-native-linux-u2-r1-1.2.32.json`。
首次 read 参数遗漏 mode=bytes 被参数校验拒绝，纠正后读取通过；没有重放已经完成的修改操作。

## Linux 权限、取消、去重与重连边界

另建 UID 23002 独占目录（0700）和文件（0600），仅使用该测试目录。

- UID 23002 原生 file_read 成功；UID 23001 并行读取返回 access_denied，实际身份仍是 23001，没有回退为 root。
- UID 23002 长命令启动父/子进程，子进程计划 20 秒后写入副作用文件。
  cancel_operation 先返回 cancel_requested，再查询原 task 确认 cancelled。
  20 秒后检查两个 PID 均已消失，副作用文件不存在。
- 同 request_id 和原参数再次调用返回同 task_id、cancelled；同 ID 改用 UID 23001 被拒绝，没有产生新任务。
- 另一任务先写 started，再等待 12 秒完成。运行中原生 disconnect/connect，之后查询原 task 得到 succeeded。
  实际文件只有一次 started，完成文件正确，没有重放。
- 重连后旧 context_ref 调用 id 被 EnvironmentChanged 拒绝；重新查询的 UID 23002 上下文可正常使用。
- 边界夹具最终删除 6 个条目，completed、partial=false。

关键任务/操作：

| 场景 | ID |
|---|---|
| 拒绝跨用户读取 operation | b90524cf-d966-469c-80cd-89925fbc24c0 |
| 取消/去重 task | 34d1517c-8919-40ab-82e3-aa8c87951a65 |
| 取消/去重 request | e81a32e0-f593-4be7-aa7c-1a1c67b60321 |
| 断线任务 task | e9a6351a-7a3f-4718-96be-3b323059e152 |
| 重连后实际效果核对 task | f2dfd26d-8dc6-426f-aba0-d037e9659e97 |
| 边界夹具清理 operation | fd9e93b3-4df2-4310-8c74-cd00015a5775 |

原始回执 `.build/execution-native-linux-boundaries-1.2.32.json`。
本例证明指定用户工作进程的父子回收，不推导 service 模式或任意脱离进程组的程序也具备相同保证。

## 未完成和明确边界

- Mac 再次查询仍为 Display 1 Shield 锁屏，各窗口没有可用 window_ref；未操作权限确认框、未重置 TCC。
  解锁后才能继续真实编辑器 Save As、退出阻塞与新包 UI 验收；旧测试 TextEdit 文档仍待正常关闭清理。
- Linux list_windows 被拒绝，当前错误为 interactive window helper is unavailable / remote_operation_failed，
  并非专门的 unsupported 错误码。本轮只证明无界面环境未执行桌面操作，不把错误语义记作已完善。
- Windows whoami /user 的本地代码页中文表头在 MCP UTF-8 文本中出现替换字符；ASCII SID 身份核对有效。
  本轮终端中文通过不能证明所有原生命令编码已经正确。
- 仍需 service/pabuser1/pabuser2 各两轮完整流程、运行中 Executor 重启恢复、双桌面两轮完整应用流程、
  Windows 多 WTS helper 生命周期及 E6 其余边界；本轮没有替代这些验收。
- 90/Mac 的最新 UI 修复升级及 Mac 实际 UI 复验仍待做；Intel 和多屏没有实机。
- 此前用户 1 的旧测试树继续保留，未删除已有升级持久化证据。本轮新建的两棵 Linux 测试树和四个终端 ready 文件已清理。

本轮未改产品源码、未重新编译或安装，不新增编译缓存。
