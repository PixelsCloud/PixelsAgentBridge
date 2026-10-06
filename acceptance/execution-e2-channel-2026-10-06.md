# E2 用户工作进程本地通信（阶段记录）

## 实现

- 复用现有Tokio：Windows named pipe，Mac/Linux Unix socket，不新增TCP监听、Bridge Host或服务凭据转交。
- Windows通过目标账户SID和服务账户SID限制管道访问，拒绝网络客户端；双方使用内核API核对对端PID。
- Unix使用独占私有目录创建socket，再允许目标UID访问；双方核对内核peer PID，服务端同时核对UID。
- 同一握手总超时包含所有错误连接尝试；PID不符的连接不会得到命令或数据。
- 服务持有子进程句柄直到完成握手，失败会回收原型工作进程；原型验证关闭通道后worker退出。
- socket仅删除自建文件和空目录，不递归删除；不会在用户目录写root测试文件。

依赖依据：[Tokio ServerOptions](https://docs.rs/tokio/latest/tokio/net/windows/named_pipe/struct.ServerOptions.html)、
[Tokio UnixStream peer_cred](https://docs.rs/tokio/latest/tokio/net/struct.UnixStream.html#method.peer_cred)。
原生身份与令牌启动沿用E0；本通道没有复制第三方进程管理框架。

## 实测

`execution_channel_probe` 是源码测试fixture，不是单独安装产品或MCP替代入口。
正式Pixels工具用于传输fixture、发起平台编译和查询原始任务结果。
每次发送307201字节二进制，逐字节核对；故意先接入错误PID的客户端，再验证真正worker可以连接。

| 环境 | 结果 |
|---|---|
| Windows本机 | 两项管道测试通过，包括错误服务端PID拒绝和错误客户端的总等待超时 |
| Windows90 SYSTEM → WTS1 Administrator | 实际SID/会话/登录代次吻合，二进制正确，服务身份不变，EOF退出 |
| Windows90 SYSTEM → WTS2普通账户 | 同上；未激活或输入用户桌面 |
| Mac root → huayang UID501 | 同上；socket清理；两项通道测试通过 |
| Linux root → UID23001 | 无GUI/logind，二进制、身份、拒绝错误PID和EOF退出通过 |
| Linux非root UID23001 → 同一UID | 同上，验证普通服务账户无需提权即可工作 |

远端通过记录：Windows `bc5114fd-9734-4d22-8bc6-7d5259e3ab03`；Mac `17e080a2-9980-40a8-a0e8-a8e25d4bc305`。

## 发现并修复

Windows初版在拒绝客户端后复用原NamedPipe实例，跨用户fixture读到UnexpectedEof。
改为在旧实例仍持有管道名时创建新实例，再丢弃旧实例；保持名称无空档，避免残留读取状态。
修复后90的两个真实账户通过同一用例。不能只依赖同进程单元测试判断跨账户可用。

Mac客户端已退出时，peer_cred可能返回NotConnected；现跳过该连接并在原总时限内等待正确worker。
测试也拆开了“拒绝服务端PID”和“服务端主动关闭错误客户端”，避免并发关闭掩盖待断言错误。

## 边界

尚未将命令、终端、Git、文件及传输路由到此通道；产品worker协议、进程树回收、任务持久化仍待完成。
本报告不代表E2完整完成，也不代表已安装工具支持execution参数。未打包安装或消费产品版本。
