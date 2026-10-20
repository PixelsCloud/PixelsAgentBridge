# 窗口恢复、输入释放与 Mac/Linux AI 协助验收

日期：2026-10-20。基线为 `0953dbf`，本轮仅修改 Desktop 的窗口恢复与解码提交等待逻辑。三项用户指定工作已有实机结果；未发布、未更新本机安装版。结构化结果及私有证据哈希见[JSON](remote-desktop-window-input-assistance-2026-10-20.json)。

## 修复与验证结果

本轮确认并修复两个产品问题：

1. Linux 从设备菜单再次打开已最小化的窗口时，窗口仍停在 Iconic。Tao 的取消最小化进入异步队列，紧接着的聚焦仍读到旧状态。改为在 GTK 主线程执行 show、deiconify、present；托盘主窗口恢复复用同一入口。
2. Mac 长时间最小化后，原生显示暂停消费，有界队列产生正常背压，却被五秒的解码提交超时判为故障。软件后备也会遇到相同情况，最终出现 `native decoder submission timed out`。改为保留同一个提交 future，只扣除窗口可见时的等待预算；隐藏期间不重建视频会话、不消耗等待用户恢复的重连时限。可见时的真实卡住仍受五秒预算约束，关闭和账号变化仍走原取消路径。

没有修改协议、授权、可靠传输或输入坐标算法。正式恢复和正常首击已通过的旧证据继续有效；不为特殊夹具增加点击重发或放宽坐标断言。

| 操作端 | 本轮真实观察 | 结果范围 |
| --- | --- | --- |
| Linux X11 → 90 | 最小化前目标实际 Ctrl 为 `[17]`，之后为 `[]`；等待超过原超时预算后走设备菜单恢复，仍为原生窗口 `0x6002b7`，Normal、聚焦；输入固定标记后目标确认长度 18、完整匹配、无按键残留 | 最新构建的最小化、恢复和继续输入通过，未以关闭重开代替恢复 |
| Linux X11 窗口 | GTK 修复的中间构建已通过同窗口菜单复用、全屏/退出、定位及拖出释放；退出全屏后目标点击 `(399,298)`，期望 `(400,300)`；拖出后按下/释放计数配对，held 为空 | 这些输入/全屏代码未被随后等待预算修复改变，复用对应原始证据 |
| Windows → 90 | 最新构建最小化后菜单恢复同一 HWND、同一视频 session；原子源点击请求/实际鼠标均为 `(428,507)`，目标为 `(399,300)`，期望 `(400,300)`；全屏为 1920×1080，退出恢复 1122×816，定位仍正确；关闭重开为只查看 | 恢复后的首击误差 1 像素，无第二个查看窗口，无自动重新启用新会话操作 |
| Windows → Apple | 同一构建此前已通过原生全屏/退出、Shift 最小化释放和拖出释放；目标观察到 Shift 解除，鼠标 down/up 配对、系统左键未按下 | 复用真实目标观测，不以本地发送成功计为输入通过 |
| Apple → 90 | 新签名测试包最小化超过一分钟后，从设备菜单唤回同一 CG 窗口 587，无手动重连，继续输入完整 18 字符标记；拖出前 down=3/up=2，之后 up=3、held 为空 | 长时间最小化、同窗口恢复、继续输入和拖出释放通过 |
| Apple 窗口 | 先前包的设备菜单复用、1920×1080 全屏及退出恢复到 `(410,80,1100,760)` 已有结果；窗口和输入映射代码未变 | 保留原构建归属，不冒充在最新包重复全套测试 |

恢复后继续同一代次时保留该窗口的操作状态；关闭重开、会话重建及协助终止按现有规则回到只查看。上述两种行为分别观察，没有混为一项。

## Mac/Linux AI 协助

两平台各核对六个场景，JSON 保存脱敏后的 revision、delivery、业务状态及窗口绑定变化。隐藏/最小化除了 MCP 状态，还检查测试 Desktop 的真实原生窗口；不是只看到 `displayed` 就算弹窗成功。

| 场景 | 两平台的观察 |
| --- | --- |
| 主窗口隐藏 | 先确认自有主窗口不可见；新请求从 pending 到 displayed，同一主窗口恢复可见并显示协助提示；没有自动打开远程查看窗口 |
| 主窗口最小化 | 请求使主窗口实际恢复并提示，用户明确打开后才进入 active |
| active 中取消 | 先确认 active，再取消，得到 cancelled/requester_cancelled；原视频窗口保留为只查看，完成入口消失 |
| 请求方退出 | 结束本次请求的 MCP；持久化快照变为 cancelled/requester_unavailable，未误报 completed，用户视频窗口保留 |
| Desktop 重启 | 旧 active 请求恢复为同 ID 的 waiting/displayed，清除旧窗口绑定；仅提示，不自动重开远程窗口；用户明确打开后绑定新窗口并进入 active |
| active 过期 | Linux 20 秒、Mac 45 秒请求均先进入 active，再变为 expired/expired；保留只读视频，不再显示本次完成入口 |

Linux 通知和生命周期使用 GTK 修复后的中间构建；Mac 隐藏/最小化、取消和请求方退出沿用本轮较早包的真实结果，最新签名包另通过 Desktop 重启及过期。后续代码变化仅涉及观看窗口的提交等待和恢复，不改协助状态机。没有重做 Windows 已通过的协助正常闭环，也没有重新申请 Apple 系统权限。

## 构建、检查及异常记录

| 产物 | SHA-256 |
| --- | --- |
| Windows Desktop debug | `f53f5803e45b844bef2963a79b0d02b1137d08e3c1b12571f0ce6840cd5619ab` |
| Mac Desktop aarch64 release，签名后 | `b5240fad63f3d3c7febc3aba833d3e020c717e5503c28f398751572195bee0ba` |
| Linux Desktop x86_64 release | `3abb44595c25bbbd1cc13662d83485c748ca07ae9cdaf2b3e7a210b4046baebe` |
| Linux GTK 恢复修复中间包 | `d3ff46082e077d459bea901d100438cf89a7a8b981497a3bce13f4b34814d9b9` |

三平台构建通过；每台 Cargo 4 个作业、嵌套 native 1 个作业，并核对其他会话占用。Windows recovery 模块 7 项测试通过，新增的 3 项分别覆盖隐藏超过预算仍保留同一提交、可见停滞超时、恢复可见后继续扣除预算。最终仅做 rustfmt 排版，没有再改变功能；JSON 分列最终源码哈希和测试产物哈希。

核心验证命令如下；Windows 使用独立 target 目录，Linux 在限 4 CPU 的开发容器内构建，遇同机其他编译时暂停容器，Mac 在 Apple 本机执行。各构建另设置 `CARGO_BUILD_JOBS=4`、`CMAKE_BUILD_PARALLEL_LEVEL=1`，Windows native 使用 `/MP1`。

```text
cargo test --locked --manifest-path apps/desktop/src-tauri/Cargo.toml --lib -j4 operator::remote_desktop::recovery::tests -- --test-threads=1
cargo build --locked --manifest-path apps/desktop/src-tauri/Cargo.toml --features tauri/custom-protocol -j4 --bin pab-desktop
cargo build --locked --release --features tauri/custom-protocol --manifest-path apps/desktop/src-tauri/Cargo.toml -j4 --bin pab-desktop
cargo +1.95.0 build --locked --release --target aarch64-apple-darwin -j4 --manifest-path apps/desktop/src-tauri/Cargo.toml --features tauri/custom-protocol --bin pab-desktop
```

最终三份 Rust 文件的 rustfmt 检查、Git diff 空白检查、165 个文档链接、53 条清单/33 条剩余编号去重、最终源码与 124 份私有证据哈希核对均通过。实机操作由自有原生窗口及生成夹具观察，步骤和异常前提按上表记录；没有将系统输入 ACK 当作应用收到文本。

保留以下未通过或不确定的观察，避免将重测覆盖原记录：

- 旧 Linux 菜单恢复失败和旧 Mac 最小化超时是真实产品失败，修复后分别补了受影响验证。
- 较早 Windows→Apple 恢复后一次点击期望 `(100,100)`、观察 `(97,172)`；该驱动没有记录按下瞬间的真实源坐标。暂停前的原子点击补查在最小化后中止，实际未点击。恢复任务后 Windows→90 的原子首击通过，但不能据此将先前 Apple 观察改写为通过，也不能称为已修复坐标算法。04.2 保留该诊断边界；如按完整焦点/几何记录再次复现，再定位产品。
- 恢复任务时，90 原服务缺少候选媒体能力，随后发现机器处在自动锁屏。准备隔离后端并确认回到 Default 后才记录有效普通输入。一次登录准备脚本在代次交接时停止，没有重放密码；本轮不据此增加密码登录通过记录。
- 一次 Windows 驱动未取得前台焦点，保护条件在注入前中止；不计为目标输入失败。有效首击单独记录了前台窗口、原生表面、实际源坐标和目标计数。

## 清理与清单更新

90 原服务已恢复且运行，临时服务进程、计划任务和新建输入夹具已退出清理；held release flags 为零。Linux 没有替换原服务，隔离 Desktop 已停止。Mac 三组临时服务已恢复/解除回滚任务，安装文件保持原哈希；测试 Desktop 已停止，原账号凭据哈希不变，克隆 Keychain 槽已删除。Windows 测试 Desktop 已停止，克隆凭据槽不存在，原槽未变。没有替换本机安装版。

Mac 本轮运行数据副本已删。部分本地临时 SQLite 副本的删除被自动审批拒绝，工具仅返回 `blocked by policy`；Linux 数据目录部分文件属 root，用户态清理未完成。保留受限私有目录及原始证据，没有绕过删除限制，不将清理记作全部完成。

本轮关闭 **03.1、03.6**；合计 **20 已关闭、30 部分完成、3 交付待验收/执行，共 53 条**。03.2 仍有完整慢消费边界，04.2/04.3/04.4 仍有输入和异常组合，08.3～08.6 仍有设备离线/删除、另一实际账号和新服务等缺项。三项指定工作完成，不表示这些原始条款全部关闭。

下一步按[长期清单](../REMOTE_DESKTOP_LONG_TERM_TASK.md#当前完成的收尾与下一步)补尚缺的键位/输入法和动态显示组合，再补离线/删除设备的协助联动；其后处理媒体边界、最终候选和交付。2 小时/8 小时继续暂缓，未启动、未预约；没有 push 或 COS 发布。
