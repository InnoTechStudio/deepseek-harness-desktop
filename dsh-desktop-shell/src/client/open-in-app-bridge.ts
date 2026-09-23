import { requestDesktop } from './desktop-bridge.ts'
import type { DesktopClientPlatform } from './environment.ts'

const OPEN_IN_APP_OPEN_PATH = '/open-in-app/open'

interface OpenInAppBody {
  app?: unknown
  path?: unknown
}

/**
 * Windows 上官方「在本地打开」会 POST `/open-in-app/open`，宿主再用隐藏的
 * Node 进程跑 `powershell Invoke-Item`。那个进程带着 CREATE_NO_WINDOW，
 * Explorer 经常不出现。这里只拦截资源管理器这一条，改走桌面宿主的 opener。
 *
 * 不能在 host 侧再注册同一条 exact 路由——webServer 遇到重复 (kind, path) 会抛错。
 * GET 应用列表和其它应用的 argv 启动仍走官方。
 */
export function installWindowsOpenInAppBridge(platform: DesktopClientPlatform): () => void {
  if (platform !== 'win32') return () => {}
  const original = window.fetch.bind(window)
  window.fetch = async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const intercepted = tryInterceptExplorerOpen(input, init)
    if (intercepted !== null) return intercepted
    return original(input, init)
  }
  return () => {
    window.fetch = original
  }
}

function tryInterceptExplorerOpen(input: RequestInfo | URL, init?: RequestInit): Promise<Response> | null {
  if (!isOpenInAppPost(input, init)) return null
  const body = readJsonBody(init?.body)
  if (body === null) return null
  if (body.app !== 'explorer' || typeof body.path !== 'string') return null
  return openExplorer(body.path)
}

function isOpenInAppPost(input: RequestInfo | URL, init?: RequestInit): boolean {
  const method = (init?.method ?? (input instanceof Request ? input.method : 'GET')).toUpperCase()
  if (method !== 'POST') return false
  return pathnameOf(input) === OPEN_IN_APP_OPEN_PATH
}

function pathnameOf(input: RequestInfo | URL): string {
  try {
    if (typeof input === 'string') return new URL(input, window.location.origin).pathname
    if (input instanceof URL) return input.pathname
    return new URL(input.url, window.location.origin).pathname
  } catch {
    return ''
  }
}

function readJsonBody(body: BodyInit | null | undefined): OpenInAppBody | null {
  if (typeof body !== 'string') return null
  try {
    const parsed: unknown = JSON.parse(body)
    if (parsed === null || typeof parsed !== 'object') return null
    return parsed as OpenInAppBody
  } catch {
    return null
  }
}

async function openExplorer(path: string): Promise<Response> {
  try {
    await requestDesktop('open-path', { path })
    return jsonResponse({ ok: true }, 200)
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error)
    return jsonResponse({ code: 'launch-failed', error: message }, 502)
  }
}

function jsonResponse(body: unknown, status: number): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  })
}
