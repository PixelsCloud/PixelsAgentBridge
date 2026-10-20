# 独立光标会话接入

日期：2026-10-17；修改前基线 `4a715a9`。延续
[光标传输基础](remote-desktop-cursor-wire-2026-10-17.md)。本阶段接入会话、能力
协商和资源生命周期；原生 helper、采集、渲染尚未接入，LT-03.5 不关闭。

## 最终行为

- Environment 增加可选的 `remote_desktop_cursor_schema_version`；后端未声明
  支持时省略。Bridge 从原有已验证设备环境读取，不新增认证或连接。
- 原 v2 Open/Control/Video 请求和 Opened 回复保持原格式。光标是额外的
  Cursor 请求与 CursorReady 回复；旧设备/未知光标版本仍能继续视频与输入。
- Host 复用原来的 owner、connection、account 和 generation 检查；同一目标
  仍只有一个观看者。每个视频代次只接受一条光标流。
- 切屏和关闭先取消旧代次，再等待视频及光标源释放。保留旧 reader 的调用方
  也会收到失效状态，不能把缓冲中的旧图像交给新画面。
- 光标流失败单独释放光标源，视频、输入和通用连接继续工作；重新选择当前
  屏幕可建立新代次及新光标流。丢弃光标 handle 也不释放视频的观看占用。
- 光标源不等待视频帧变化或视频展示额度。反向方向发送严格匹配序号的 8 字节
  小端消费确认；最多一份未消费更新，避免可靠流缓存大量旧位置。确认代表
  接收并校验，不代表原生窗口已经绘制。
- 源启动、部分传输和消费确认都有时限。静止快照可以等待，关闭/切屏/账号
  改变仍能取消正在等待的源。像素继续走有界二进制，不进入日志。

## 验证

使用本机真实 Iroh/QUIC 连接和生成的视频/光标字节，不采集或操作用户桌面。
没有替换本机安装版、远端服务或 Mac 授权包。编译总并发保持在 10 以下：基础
检查使用 8 jobs；检测到其他原生构建后暂缓新编译。集成检查用 6 jobs，后续
复跑遇到另一个明确 `-j 2` 的 Cargo 构建时，核对合计为 8 后才启动；无法确认
预算的原生构建存在时直接拒绝启动。原生嵌套构建 1 job，测试线程 2。

```text
cargo test --locked -p pab-protocol --lib -j8
cargo test --locked -p pab-remote-desktop --lib --test reliable_sessions -j8
cargo test --locked -p pab-executor -p pab-bridge --lib remote_desktop -j6
```

新增会话测试覆盖：

1. 静止视频和视频四帧展示额度耗尽时，光标仍能移动。
2. 切屏后旧缓冲图像不能继续返回，新源从新代次/序号 1 开始；源并发峰值为 1。
3. 同账号另一连接、重复附加和错误光标版本被拒绝，复用原占用与身份规则。
4. 没有或不认识光标能力时，原有视频及通用请求继续可用。
5. 光标源异常后视频、按键输入/释放和旧业务继续；新代次能重新取得光标。
6. 关闭光标 handle 只释放它的源；保留 handle 也不能延长已关闭会话。
7. 账号变化取消正在启动的光标源，释放视频和光标资源，不遗留占用。
8. 未确认第一份更新前，不产生第二份；错误消费确认只终止光标流。

Executor 既有接入测试已扩展为视频、光标和 Presence 同走 TaskService，并修正
原测试写死的早期远程桌面版本号，改为校验当前协议常量。旧 Environment JSON
仍能原样往返；无后端时两项远程桌面能力均不公告。

最终结果：protocol 68 项、remote-desktop 单元 6 项、可靠流 25 项、Bridge
相关 2 项、Executor 相关 2 项全部通过，共 103 项。消费确认加入后已重跑
可靠流与 Executor/Bridge 接入测试。日志在 `.build/remote-desktop/` 下的
`pab-protocol-cursor-tests.log`、`pab-remote-desktop-cursor-tests.log` 和
`cursor-bridge-executor-tests.log`。

当前生产 NativeBackend/HelperMediaBackend 保持默认不公告光标能力，不能用
这些生成快照测试宣称系统光标已显示。没有进行本轮三平台 GUI 或弱网实机验收。

## 下一步

为现有已验证媒体 helper 增加独立光标 IPC，绑定正在采集的同一会话和布局，
避免与等待新视频帧共用串行请求队列。再实现平台采样及原生叠加；明确处理
视频内含光标与独立光标的互斥、热点/缩放/裁剪、反色、隐藏和代次切换。
最后在实机多屏和登录场景验收，Mac 按用户本轮单屏条件执行。
