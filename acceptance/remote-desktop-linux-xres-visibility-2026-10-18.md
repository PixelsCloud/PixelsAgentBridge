# Linux X11 隐藏光标：XRes 可行性检查

日期：2026-10-18。基线 `ffe99b9`。本轮是诊断，没有修复生产光标源，
LT-03.5 / LT-07.3 保持未关闭，完整关闭数仍为 10/53。

## 结果与决定

XRes 能观察 `XFixesCursorHideCount` 资源是否存在，嵌套 hide/show、多个客户端
同时隐藏、客户端断开释放均可观察。但资源没有暴露其所属 X screen/root：
仅隐藏 screen 0 与仅隐藏 screen 1 都返回一个资源，资源大小查询的交叉引用
均为零。因此本轮**不把全局计数接入生产光标可见性**，避免隐藏另一屏的光标。

这里的两个 X screen 各有独立 root，不等同于常见的同一 RandR root 多显示器。
单 root 下这个计数可以反映 XFixes 隐藏请求，但它没有涵盖 X server 其他隐藏
来源，也不是完整的可见性 API；本轮没有把它认定为通用解法或增加安装依赖。
后续仍须从实际 compositor / 显示平面取得按输出区分的状态，并验证软件光标。
现有 GNOME PipeWire / DRM 路径的证据不自动覆盖普通 Xorg。

## 验证范围

隔离 Docker 容器，Ubuntu 22.04，Xvfb `2:21.1.4-2ubuntu1.7~22.04.16`，
libXRes `2:1.2.1-1`，XRes 协议 1.2。两个 X screen 分别为
1024×768、800×600；三个独立 X 客户端连接。

- 10 个阶段的隐藏资源数：`0,1,1,1,2,1,0,1,2,0`。
- XRes 资源计数与资源大小查询得到的资源个数一致，交叉引用全部为零。
- 重复隐藏只增加内部计数，第一次 show 后仍隐藏；最后一个客户端释放后消失。
- 一个客户端隐藏两个 screen 后断开，两个资源都释放。
- 原生检查使用 ASan/UBSan、`-Wall -Wextra -Werror`，退出码 0，无诊断输出。
- 编译为单个 g++ 进程；当时其他会话编译为 1 job，总并发低于 10。
- Xvfb 与探针由限时驱动退出并回收，临时容器移除。未访问或修改真实 Linux GUI。

首次探针把 XRes 1.2 返回的 `Success == 0` 当成布尔失败，触发断言；已修正
诊断程序并重新编译验证，失败记录保留在本机 ignored 构建目录。生产程序未改动。
机器可读结果见同名 JSON。

## 复现

源文件：`experiments/remote-desktop/probe_x11_cursor_visibility.cpp`。
需要 X11、Xfixes、XRes 的开发包；这些仅为本探针依赖。

```sh
g++ -std=c++17 -Wall -Wextra -Werror -fsanitize=address,undefined -g \
  experiments/remote-desktop/probe_x11_cursor_visibility.cpp \
  -lX11 -lXfixes -lXRes -o /tmp/probe-xres
# 在一次性的双 screen Xvfb 下执行，禁止指向用户桌面：
DISPLAY=:94 /tmp/probe-xres --isolated-display
```

参考 [X server XFixes 实现](https://raw.githubusercontent.com/mirror/xserver/master/xfixes/cursor.c)
与 [XRes 资源查询实现](https://raw.githubusercontent.com/mirror/xserver/master/Xext/xres.c)。
前者的隐藏资源保留 screen 指针，但后者查询并不直接导出这个指针。

## Apple 解锁回复的后续核对

本轮只读查询返回 console uid 501、on-console 为 true；返回值没有锁屏字段，
不能把缺失字段作为未锁屏的独立证明。用户已明确报告解锁，未再次申请权限。
既有正式单屏输入、光标和协助验收已完成，见
[最新 Apple 验收记录](remote-desktop-macos-activity-input-assistance-2026-10-18.md)。
本轮没有重复启动或替换 Apple 的已授权包。
