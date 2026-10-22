# Android 统一界面工具

Android 复用现有 MCP 名称、system-query 协议、操作查询和请求去重。手机侧通过
`AndroidDesktopQueries` 适配同一套无障碍后端与输入调度器；原来的 `pab_mobile_control` 保留。
首次使用仍需在手机正常授权无障碍，远程控制必须开启，手机必须亮屏且已解锁。

| 工具 | Android 范围 |
|---|---|
| `pab_list_windows` | 当前活动无障碍窗口；不枚举任意后台应用窗口 |
| `pab_list_monitors` | 当前活动显示屏，ID 为 0，坐标为屏幕像素 |
| `pab_focus_window` | 校验引用仍指向活动窗口；不切换任意后台窗口 |
| `pab_type_text` | 在唯一聚焦、可编辑且非密码的原生控件当前选区插入文字，保留其他内容 |
| `pab_ui_query` | 查询当前窗口或已有控件子树；支持原生角色、名称和 identifier 筛选 |
| `pab_ui_get` | 读取已有控件；文字值须显式 `include_value=true` |
| `pab_ui_action` | 按控件原生支持执行 invoke、set_value、set_checked、select、expand、collapse、focus |
| `pab_ui_wait` | 有界等待存在、消失、启用、选中、勾选或指定文字值 |
| `pab_capture_screenshot` | 使用共享截图传输和 JPEG 编码，支持当前窗口引用、裁剪、大小与哈希校验 |

`set_value` 替换整个控件值，`type_text` 在选区插入。输入复用暂停状态、串行调度及原生节点校验；
`expected` 在派发前核对原生状态。不支持的原生动作返回拒绝，已经满足的状态不重复派发。
一旦派发后的结果无法确认，调用方应查询原请求及观察页面，不得以新请求自动重放。

窗口和控件引用绑定调用方、连接、窗口身份和有效期。断线重连、页面切换、布局身份变化或过期后应重新查询。
当前无障碍后端每次遍历会更新原生节点引用，因此新的 `ui_query`、窗口或显示屏遍历会使旧控件引用失效；
操作前应使用最新查询结果。`ui_get` 与截图上下文检查不重新遍历。
列表最多返回 128 个控件，最多访问 2000 个节点，深度与时间也有预算；截断结果不能证明某控件不存在。
等待最长 30 秒，等待期间其他查询和输入仍可执行，暂停控制会中断等待。

密码控件不返回名称、值或可执行动作。截图继续受系统安全窗口和现有截图授权约束。
截图源为 JPEG，不能提供无损 Original；共享旧式 PNG 请求不在 Android 支持范围。
沿用共享契约：`window_ref` 与 `monitor_id`／`region` 互斥；裁剪使用当前显示屏及屏幕像素区域。
返回映射为 `android_screen_pixels`，不宣称桌面坐标或鼠标输入。

Android 不提供任意桌面窗口管理、最小化、最大化、原始键鼠批处理、显示屏输入目标或其他应用 PID。
共享窗口契约的 `process_id=0` 表示不可用，响应同时给出范围说明。
`pab_window_control` 与 `pab_desktop_input` 仍未接入 Android；浏览器自动化继续暂缓。

真机验收脚本为 `scripts/test_android_unified_tools.py`。ADB 只准备本项目调试目标和正常首页控制，
功能验收经已有认证的公网强制 Relay 完成，不用 ADB 输入代替远程工具。
