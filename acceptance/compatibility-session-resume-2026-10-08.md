# 重载后正式工具验收

基线：本机/90/Linux 1.2.41，Mac ARM 1.2.42；新会话实际加载 encoding、include_base64、new_instance。
全程使用原生 pixels.pab_*；编码切换只读原任务，没有重跑旧命令。

## 编码（通过）

- 90 历史 whoami 任务 a7c210e4-98d3-4672-9c09-1de282ffd978：2485 bytes 按 GBK 正确显示中文，替代标记 false。
- 新任务 fcb9e12e-7dc2-4e2f-bb78-d7b6517c9658：stdout 中文、stderr 错误均正确，Base64 与独立指定的原字节一致。
  同一任务改按 UTF-8 重读出现替代标记，再改 GBK 正确；原字节始终相同。
- Linux UTF-8 分段任务 58684090-5021-427a-b302-d514498963cd：先输出中文前两个字节，返回 pending_bytes=2、next_offset=0、空文本及空 Base64。
  后续从原偏移读取，4-byte 上限依次返回“中”、emoji、换行，游标 0→3→7→8，无丢字。
- Mac 任务 4f0677a2-f971-41e1-92cf-e8db2ed11685：UTF-16BE 中文/emoji 正确；代理对跨读取边界保留，游标 0→2→6；奇数 offset 明确拒绝。
  同一任务 stderr 用 Big5 重读正确，错误 UTF-16 解码的真实不完整尾字节会标记替代。
- 90 任务 016ae585-bf1b-4a02-b631-ee024bd7d43c：UTF-16LE 中文/emoji 正确。
- Linux 任务 ac958e24-2c45-45e1-9bef-4054494ff41e：GB18030 中文及四字节 emoji 正确。
- 90 任务 92e2f4fd-8d81-43d6-8255-0bb79ab008f3：EOF 截断 UTF-8 明确标记替代，原字节保留；contains 只过滤展示文字，Base64 不过滤。
- max_bytes=3 被参数校验拒绝；目录约定最小 4，后续边界测试使用 4。

## 应用实例与文稿

- Mac 锁屏后通过已有输入工具解锁，恢复 AX 窗口引用，无重新授权或 TCC 重置。
- 默认 TextEdit 启动复用 PID 575，reused_instance=true；显式 new_instance 创建 PID 61215，随后实际观察到独立窗口。
- Windows new_instance 请求 c87274b2-dfef-46e9-b403-a8568b78a21b 在派发前返回 unsupported_platform，明确没有启动应用。
- 新实例富文本：AX 设置中文/emoji，关闭出现保存 Sheet，在 Sheet 保存为自有 RTF，textutil 回读精确匹配，窗口关闭。
  回读任务 69813ac9-3157-4a8d-8a30-5658d503994f。
- 旧实例纯文本：新建、Cmd+Shift+T、重新查询文字控件、AX 设置、关闭、原生 Sheet 保存，UTF-8 文件 33 bytes 精确匹配。
  SHA-256 ff376913c9fd532ccb741cb1e3c5440c6dc6ae9e5fb197bdd36323058641fced，回读任务 2609c3bb-55e5-49fb-9f5f-36ecf1400c6c。
- 两个测试文件已通过文件工具删除，原操作均确认 completed；对应文稿窗口已关闭。
- 上轮空文件/富文本不提示问题本轮没有复现，不能因此宣布根因已定位或已修复。继续保留显式保存和回读建议。

## 确认的修正

关闭等待保存确认时，原工具错误地提示 application may constrain geometry。
现在分别说明“窗口仍存在，检查保存/确认对话框，不要重放关闭”和“前台状态未确认”。
仍返回 unconfirmed，不擅自点击保存/丢弃，不改变窗口操作行为。源码提交 2caaa27。
同时修正 AXSelectedText 注释：AX 值匹配不保证文稿 dirty 状态或持久化。

Mac 原生桌面库 47 passed。全仓 cargo fmt --check 受已有其他文件格式差异阻挡；本次两个文件单独格式化、diff --check 通过。
Mac 1.2.43 ARM/Intel Release 构建均退出 0（任务 b9c40cb0-2035-4e11-bfb5-7b79d93646d7），
版本自动递增并提交 eeb9ff5；两种架构的 TypeScript/Vite 构建均通过。

## 1.2.43 安装与产物

- Mac ARM 安装成功，退出 0；所有清单文件哈希匹配，固定证书约束、设备身份和抽查任务历史保留。
  私有回执位于 `/private/var/tmp/pab-compat-release-1.2.43/result.json`，读取任务 d68c7b30-93b7-4268-9970-e4b5044b9ee8。
- 无 TCC 重置或重新授权，安装后实际截图及 AX 控件查询通过。新 helper 为 599d0d79-e74e-4627-ae63-05795f0c54e0。
- 在专用空白文稿输入文字，正常关闭触发原生保存 Sheet；操作 8b7db0e1-8042-44c3-b68c-dcbd10cd8fdd
  返回 unconfirmed，具体提示 `close requested but the window is still present; inspect its save/confirmation dialog or application state; do not replay close`。
  未误报关闭完成，也未提及窗口尺寸。最后仅丢弃该测试文稿，并确认窗口消失。
- 一次性 launchd 安装作业已 bootout，私有升级备份保留。本机 MCP 保持 1.2.41，不需要再重启当前会话。
- 两个 PKG 均通过原生 download 工具复制到本机 `.build/packages` 并独立核对哈希；Intel 只有交叉编译/签名验证，未声称 Intel 实机运行。

| 包 | 大小 | SHA-256 |
|---|---:|---|
| pixels-agent-bridge-macos-aarch64-release-setup.pkg | 30901407 | 5c665c5e0e833f5f67496dcdb12b01e941c1370eb8b629e1f4a3af0aa9de28e6 |
| pixels-agent-bridge-macos-x86_64-release-setup.pkg | 34008691 | 5c18776919701ba16c959e468977bb9212965a7c24b93e730cc3bd4e9dd5c963 |

安装脚本首次上传因目标目录不存在失败，沿原 ID 0e4cc97c-c79c-4a23-a41c-594cc34e656f 确认 terminal failed，
检查目标文件不存在后创建私有目录，再提交新的上传；没有盲目重放 unconfirmed 操作。
编号见 [本轮操作索引](compatibility-session-operation-index-2026-10-08.json)，仅保存工具名、设备码、编号和状态，不含参数/私有输出。

## 仍保留的边界

普通用户 GUI/UAC、多活动 WTS、Intel 运行、多屏及 Windows/Mac 全新系统卸载缺少独立实机条件，未冒充通过。
Linux 无界面版保持原有支持范围。
