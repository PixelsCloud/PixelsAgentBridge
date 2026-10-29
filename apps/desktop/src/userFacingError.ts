import { initialLanguage, messages, type Language } from "./i18n";
import { macosPermissionText } from "./macosPermissionText";
import { desktopTerminalText } from "./desktopTerminalText";

// Presentation only: retain the original error in state/protocol/logs. Never
// use translated text to decide retries, permissions, or takeover ownership.
export const errorMessages = {
  unknown: ["操作未完成。请检查当前状态；如需排查，可查看诊断日志。", "The operation did not complete. Check its current state; diagnostic logs can help investigate."],
  fullscreen: ["无法切换全屏，请稍后重试。", "Could not switch fullscreen mode. Please try again."],
  confirmation: ["无法显示确认窗口，请稍后重试。", "Could not open the confirmation dialog. Please try again."],
  assistance: ["无法更新协助请求，请检查请求状态后再试。", "Could not update the assistance request. Check its status before trying again."],
  window: ["无法打开操作窗口，请关闭多余窗口后重试。", "Could not open the window. Close any unused windows and try again."],
  configChanged: ["配置已被其他操作修改，请刷新后重新操作。", "Settings changed during this operation. Refresh before making changes again."],
  signOut: ["退出账号失败，当前登录状态已保留，请稍后重试。", "Could not sign out. Your current sign-in was preserved; please try again."],
  accountChanged: ["账号已切换，请在当前账号下重新打开文件传输。", "Account changed. Reopen file transfer under the current account."],
  nativePath: ["此原生路径无法表示为剪贴板文本，请直接选择条目进行传输或管理。", "This native path cannot be represented as clipboard text. Use its entry to transfer or manage the file."],
  terminalOutput: ["终端输出不完整，会话已停止接收输入。请检查远端进程后重新打开终端。", "Terminal output is incomplete and input has been stopped. Check the remote process before reopening the terminal."],
  shellExited: ["Shell 已退出", "Shell exited"],
  clearTransfers: ["无法清理已完成的传输记录，请稍后重试。", "Could not clear completed transfers. Please try again."],
  resolveConflict: ["同名文件处理失败，请检查目标文件状态后重新选择。", "Could not resolve the file conflict. Check the destination file and choose again."],
  macosSettings: ["无法打开系统权限设置，请在 Mac 的“系统设置 → 隐私与安全性”中手动打开。", "Could not open permission settings. Open System Settings → Privacy & Security on the Mac."],
  permissionStatus: ["无法确认权限状态，请检查设备服务后刷新。", "Could not confirm permission status. Check the device service and refresh."],
  updateVerification: ["更新包校验失败，已停止安装。请重新下载。", "The update package failed verification. Installation was stopped. Download it again."],
  installer: ["无法启动安装程序，请检查系统权限或手动运行已下载的安装包。", "Could not start the installer. Check system permissions or run the downloaded installer manually."],
  updateFailed: ["更新未完成，请检查网络和当前版本后重试。", "The update did not complete. Check the network and current version before trying again."],
  fileCopy: ["无法复制此文件，请检查源文件和目标目录是否可访问。", "Could not copy this file. Check access to the source file and destination folder."],
  occupied: ["这台设备正在被其他会话控制。你可以关闭原会话，或确认后强制接管。", "Another session is controlling this device. Close that session, or confirm a takeover."],
  takenOver: ["当前会话已被其他控制端接管，键盘和鼠标控制已释放。", "Another controller took over this session. Keyboard and mouse control has been released."],
  takeoverChanged: ["远端会话已变化或暂时无法释放，请重新连接后再次确认。", "The remote session changed or could not be released. Reconnect and confirm again."],
  takeoverUnavailable: ["当前被控端暂不支持接管，请关闭原会话或更新被控端后重试。", "This remote app does not support takeover. Close the existing session or update the remote app."],
  unsupported: ["此设备暂无此功能。", "This feature is not available on this device."],
  androidUnsupported: ["此 Android 设备暂无此功能。", "This feature is not available on this Android device."],
  permission: ["没有执行此操作的权限。请检查远端授权及所选用户。", "Permission was denied. Check remote permissions and the selected user."],
  login: ["登录状态已失效，请重新登录账号。", "Your sign-in session is unavailable or expired. Sign in again."],
  credentials: ["账号或密码不正确，请检查后重试。", "The account or password is incorrect. Check your credentials."],
  devicePassword: ["设备访问密码不正确，请确认远端显示的密码。", "The device password is incorrect. Check the password shown on the remote device."],
  offline: ["设备暂时无法连接。请确认远端 Pixels 在线并检查网络。", "The device is unreachable. Check that remote Pixels is online and the network is available."],
  network: ["连接已中断。请检查两端网络后重新连接。", "The connection was interrupted. Check the network on both devices, then reconnect."],
  timeout: ["等待响应超时。请检查连接及原操作状态，避免重复执行。", "The response timed out. Check the connection and the original operation before trying again."],
  unconfirmed: ["操作结果尚未确认。请先检查原操作状态，避免重复执行。", "The operation result is not confirmed. Check the original operation before repeating it."],
  media: ["远端画面暂时不可用。请确认远端桌面已打开；若未自动恢复，请重新连接。", "The remote display is temporarily unavailable. Check the remote desktop; reconnect if it does not recover automatically."],
  decoder: ["画面显示失败。请重新连接；若仍失败，请查看诊断日志。", "The remote picture could not be displayed. Reconnect; check diagnostic logs if the problem continues."],
  viewOnly: ["当前为仅查看模式，请先启用键盘和鼠标控制。", "This session is view-only. Enable keyboard and mouse control first."],
  inputNotReady: ["键盘和鼠标控制尚未就绪，请等待远端画面加载完成。", "Keyboard and mouse control is not ready. Wait for the remote picture to load."],
  session: ["会话已结束或发生变化，请重新连接。", "The session ended or changed. Reconnect to continue."],
  console: ["远端没有可用的已登录桌面用户。请登录并解锁远端桌面。", "No signed-in desktop user is available. Sign in and unlock the remote desktop."],
  userChanged: ["所选用户会话已失效，请刷新并重新选择用户。", "The selected user session is no longer available. Refresh and select the user again."],
  busy: ["设备正在处理其他操作，请稍候再试。", "The device is handling another operation. Please wait and try again."],
  notFound: ["目标已不存在或无法访问，请刷新后重新选择。", "The requested item no longer exists or is unavailable. Refresh and select it again."],
  conflict: ["目标位置已有同名文件，请选择替换、跳过或保留两者。", "An item with this name already exists. Choose Replace, Skip, or Keep both."],
  changed: ["文件或目录已发生变化，请刷新并重新选择。", "The file or folder changed. Refresh and select it again."],
  space: ["目标磁盘空间不足，请释放空间后再试。", "The destination disk is full. Free up space and try again."],
  readOnly: ["目标位置为只读，请选择可写入的目录。", "The destination is read-only. Choose a writable folder."],
  integrity: ["文件完整性校验失败，传输未完成。请重新传输该文件。", "File integrity verification failed. Transfer this file again."],
  limit: ["此内容超过设备支持的大小或数量限制，请减少后再试。", "This content exceeds the device's size or item limit. Reduce it and try again."],
  clipboard: ["剪贴板同步失败。请确认两端允许访问剪贴板后重新复制。", "Clipboard synchronization failed. Check clipboard access on both devices, then copy again."],
  cancelled: ["操作已取消。", "The operation was cancelled."],
  invalid: ["请求的内容或设置无效，请检查后再试。", "The request or settings are invalid. Check them and try again."],
  certificate: ["无法验证服务器身份。请检查服务地址、系统时间及证书配置。", "The server identity could not be verified. Check its address, the system clock, and certificate settings."],
  server: ["服务暂时不可用，请稍后重试。", "The service is temporarily unavailable. Please try again later."],
  configuration: ["无法读取或保存配置。请检查文件权限和配置格式。", "Settings could not be read or saved. Check file permissions and the configuration format."],
} as const satisfies Record<string, readonly [string, string]>;
type ErrorKey = keyof typeof errorMessages;

const rules: readonly [RegExp, ErrorKey][] = [
  [/could not clear completed transfers/i, "clearTransfers"],
  [/could not resolve conflict/i, "resolveConflict"],
  [/open_macos_permission_settings/i, "macosSettings"],
  [/macos_permissions/i, "permissionStatus"],
  [/could not start the installer|installer.*(?:launch|start).*failed/i, "installer"],
  [/fullscreen.*(?:unavailable|failed)|full.?screen.*error/i, "fullscreen"],
  [/could not show.*confirmation/i, "confirmation"],
  [/could not complete current request|assistance.*(?:failed|expired|unavailable)/i, "assistance"],
  [/could not create.*window|window.*creation.*failed/i, "window"],
  [/config(?:uration)? changed/i, "configChanged"],
  [/sign out failed|logout.*failed/i, "signOut"],
  [/desktop_taken_over|remote desktop TakenOver\b/i, "takenOver"],
  [/this device already has a desktop viewer|remote desktop Busy\b|desktop_occupied/i, "occupied"],
  [/remote session changed or could not be released|远端会话已变化或暂时无法释放/i, "takeoverChanged"],
  [/takeover requires an updated remote app/i, "takeoverUnavailable"],
  [/android_(?:system_action_unavailable|pty_start_failed|terminal_requires_app_uid)|Android does not provide a System\/root terminal|Android.*(?:not available|not supported)/i, "androidUnsupported"],
  [/console (?:session discovery unavailable|user is unavailable)|no (?:active |interactive )?(?:local )?(?:console|desktop) (?:session|user|login)/i, "console"],
  [/execution_context_(?:expired|unavailable|changed)|EnvironmentChanged|context.*(?:expired|stale)|user session.*(?:expired|changed)/i, "userChanged"],
  [/unconfirmed|unknown outcome|not confirmed|reply.*lost/i, "unconfirmed"],
  [/^shell exited$/i, "shellExited"],
  [/terminal output.*(?:gap|duplicate|invalid)|PTY output ended unexpectedly/i, "terminalOutput"],
  [/view only|view.only|input.*disabled/i, "viewOnly"],
  [/input.*not ready|first frame.*not ready/i, "inputNotReady"],
  [/MediaUnavailable|presentation backlog expired|capture.*(?:failed|unavailable)|screen.*unavailable/i, "media"],
  [/decoder|decode.*failed|native viewer|(?:renderer|presenter).*(?:failed|unavailable|error)/i, "decoder"],
  [/StaleGeneration|session.*(?:ended|closed|expired|changed)|viewer is closing/i, "session"],
  [/device.*(?:offline|not found|unavailable)|NotFound.*device/i, "offline"],
  [/device.*password|access password.*(?:invalid|incorrect)/i, "devicePassword"],
  [/invalid_credentials|invalid credentials|incorrect (?:username|password)|wrong password/i, "credentials"],
  [/login required|account unavailable|unauthenticated|unauthorized|HTTP 401|session_expired/i, "login"],
  [/not supported|unsupported|unavailable capability|feature.*unavailable|not implemented|version.*mismatch/i, "unsupported"],
  [/permission[ _-]?denied|permission.*(?:required|missing|not granted)|access.denied|destination.denied|forbidden|NotAllowed|HTTP 403|os error 5\b/i, "permission"],
  [/no space|disk.*full|insufficient.*space|os error 112\b/i, "space"],
  [/cannot copy|could not copy/i, "fileCopy"],
  [/read.only/i, "readOnly"],
  [/hash.*mismatch|checksum|integrity.*fail|signature.*(?:invalid|fail)|invalid.*signature/i, "integrity"],
  [/already exists|name.conflict|destination_exists/i, "conflict"],
  [/source.changed|file.changed|directory.changed|stale.*(?:file|directory)/i, "changed"],
  [/not.found|does not exist|no such file|os error [23]\b/i, "notFound"],
  [/too.large|too.many|limit.exceeded|size.limit|oversized/i, "limit"],
  [/certificate|\bTLS\b|unknownissuer/i, "certificate"],
  [/timed? ?out|timeout|deadline/i, "timeout"],
  [/clipboard|paste.*fail/i, "clipboard"],
  [/cancelled|canceled/i, "cancelled"],
  [/busy|rate.limit|HTTP 429/i, "busy"],
  [/HTTP 5\d\d|service.unavailable|server.error/i, "server"],
  [/connection|network|transport|\bQUIC\b|\bDNS\b|broken pipe|unexpected eof|peer.*closed/i, "network"],
  [/configuration|settings unavailable|config.*(?:invalid|parse|read|write)|invalid.*(?:json|toml)|malformed.*(?:json|toml)/i, "configuration"],
  [/invalid|bad.request|HTTP 400/i, "invalid"],
];

// Existing curated translations are safe to render, including contextual
// prefixes such as "Registration failed: ...". Never echo arbitrary suffixes.
const curated = new Map<string, Record<Language, string>>();
function collect(zh: unknown, en: unknown) {
  if (typeof zh === "string" && typeof en === "string") {
    const pair = { "zh-CN": zh, en };
    curated.set(zh, pair); curated.set(en, pair);
  } else if (zh && en && typeof zh === "object" && typeof en === "object") {
    for (const key of Object.keys(zh)) collect((zh as Record<string, unknown>)[key], (en as Record<string, unknown>)[key]);
  }
}
collect(messages["zh-CN"], messages.en);
collect(macosPermissionText["zh-CN"], macosPermissionText.en);
collect(desktopTerminalText["zh-CN"], desktopTerminalText.en);
for (const [zh, en] of Object.values(errorMessages)) collect(zh, en);

function rawError(value: unknown): string {
  if (typeof value === "string") return value.slice(0, 8192);
  if (value && typeof value === "object") {
    const error = value as { code?: unknown; message?: unknown; error?: unknown };
    // Prefer structured codes; support Error, Tauri and protocol error objects.
    return [error.code, error.message, typeof error.error === "string" ? error.error : undefined]
      .filter((part): part is string => typeof part === "string").join(" ").slice(0, 8192);
  }
  return "";
}

export function userFacingError(cause: unknown, language: Language = initialLanguage(), context?: "update"): string {
  return translateError(cause, language, 0, context);
}

function translateError(cause: unknown, language: Language, depth: number, context?: "update"): string {
  const raw = rawError(cause).trim().replace(/^(?:(?:Error|TypeError|RuntimeError):\s*){1,4}/, "");
  const known = curated.get(raw);
  if (known) return known[language];
  const separator = raw.indexOf(": ");
  if (separator > 0 && depth < 4) {
    const prefix = curated.get(raw.slice(0, separator));
    if (prefix) return `${prefix[language]}: ${translateError(raw.slice(separator + 2), language, depth + 1, context)}`;
  }
  let key = rules.find(([pattern]) => pattern.test(raw))?.[1] ?? "unknown";
  if (context === "update") {
    if (key === "integrity") key = "updateVerification";
    if (key === "unknown") key = "updateFailed";
  }
  return errorMessages[key][language === "en" ? 1 : 0];
}
