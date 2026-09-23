/**
 * 与桌面宿主（Tauri）之间的请求/响应通道。
 *
 * 客户端设置的真实数据源在宿主进程：开机自启要写注册表或 LaunchAgent，
 * 防休眠要调用系统电源 API，这些都只有原生侧能做。所以设置界面放在 DSH 里，
 * 读写仍然转交宿主。
 *
 * 现有的 `postMessage` 都是单向通知（拖动窗口、检查更新），拿不到返回值，
 * 这里在其上补一层 requestId 配对。
 */

/** 宿主支持的请求类型。 */
export type DesktopCommand =
  | 'get-settings'
  | 'set-settings'
  | 'get-state'
  | 'send-test-notification'
  | 'open-notification-settings'
  | 'check-client-update'
  | 'open-path'

interface PendingRequest {
  resolve: (value: unknown) => void
  reject: (reason: Error) => void
  timer: ReturnType<typeof setTimeout>
}

const REQUEST_TYPE = 'dsh-desktop:request'
const RESPONSE_TYPE = 'dsh-desktop:response'
/** 宿主没响应时的放弃时限。设置读写都是本地调用，正常在毫秒级完成。 */
const TIMEOUT_MS = 8000

const pending = new Map<string, PendingRequest>()
let listening = false
let counter = 0

/** 宿主的响应只可能来自父窗口，其余来源一律忽略。 */
function onMessage(event: MessageEvent): void {
  if (event.source !== window.parent) return
  const message = event.data
  if (message === null || typeof message !== 'object') return
  if ((message as { type?: unknown }).type !== RESPONSE_TYPE) return

  const { id, ok, data, error } = message as {
    id?: unknown
    ok?: unknown
    data?: unknown
    error?: unknown
  }
  if (typeof id !== 'string') return
  const entry = pending.get(id)
  if (entry === undefined) return
  pending.delete(id)
  clearTimeout(entry.timer)
  if (ok === true) entry.resolve(data)
  else entry.reject(new Error(typeof error === 'string' ? error : '宿主未返回结果'))
}

function ensureListening(): void {
  if (listening) return
  window.addEventListener('message', onMessage)
  listening = true
}

/**
 * 向宿主发一个请求并等待结果。
 *
 * @param command - 宿主侧的命令名。
 * @param payload - 命令参数，会原样转交给对应的 Tauri command。
 * @returns 宿主返回的数据。
 * @throws 宿主报错、超时，或当前不在 iframe 里运行时。
 */
export function requestDesktop<T>(command: DesktopCommand, payload?: unknown): Promise<T> {
  if (window.parent === window) {
    return Promise.reject(new Error('未在桌面客户端中运行'))
  }
  ensureListening()
  counter += 1
  const id = `${Date.now().toString(36)}-${counter}`
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => {
      pending.delete(id)
      reject(new Error('桌面客户端未响应，请稍后重试'))
    }, TIMEOUT_MS)
    pending.set(id, { resolve: resolve as (value: unknown) => void, reject, timer })
    window.parent.postMessage({ type: REQUEST_TYPE, id, command, payload }, '*')
  })
}

/** 宿主已有的对话框，由宿主自己渲染在 iframe 之上。 */
export type HostDialog = 'about' | 'check-updates' | 'feedback'

/**
 * 请求宿主弹出它已有的对话框。
 *
 * 关于、检查更新、反馈这三个窗口宿主早就实现了（含版本对比、下载进度、
 * 日志采集），这里只发通知让它显示，不在 DSH 侧重写一遍。
 */
export function openHostDialog(dialog: HostDialog): void {
  if (window.parent === window) return
  window.parent.postMessage({ type: `dsh-desktop:open-${dialog}` }, '*')
}

/** 当前是否运行在桌面客户端的 iframe 中。 */
export function inDesktopHost(): boolean {
  return window.parent !== window
}
