import { useCallback, useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore } from 'react'
import type {} from './contracts.ts'
import type { DesktopClientPlatform } from './environment.ts'
import {
  computeDesktopColumns, DesktopLayoutState, DETAILS_DEFAULT,
  MACOS_SIDEBAR_COLLAPSED, SIDEBAR_AUTO_COLLAPSE, SIDEBAR_COLLAPSED, SIDEBAR_DEFAULT,
} from './layout-state.ts'
import { MACOS_TRAFFIC_LIGHT_SAFE_WIDTH, WINDOWS_TITLEBAR_HEIGHT } from '../window-chrome.ts'

/** Private values assembled by the advanced-shell registration. */
export interface AdvancedFrameInjected {
  /** Desktop-owned panel state exposed through the standard layout service. */
  layout: DesktopLayoutState
  /** Host platform controlling native title-bar spacing. */
  platform: DesktopClientPlatform
}

/** Full advanced root slot props. */
export type AdvancedFrameProps = AdvancedFrameInjected & {
  renderSlot: (
    name: 'sidebar' | 'main' | 'rightbar' | 'shell.overlay',
    owner: Record<string, unknown>,
    opts?: { entryKey?: string | null },
  ) => JSX.Element
  useSessions: (selector: (state: any) => any) => any
}

function postDesktopMessage(type: 'dsh-desktop:check-updates' | 'dsh-desktop:start-dragging' | 'dsh-desktop:request-update-state') {
  window.parent.postMessage({ type }, '*')
}

/**
 * 宿主是否报告有可用更新。
 *
 * 标题栏只在有更新时才显示按钮，所以这个状态必须来自宿主的真实检查结果，
 * 不能由前端自己猜。挂载时主动问一次：后台检查任务启动 20 秒后才跑第一轮，
 * 而这个组件通常更早就绪。
 */
function useUpdateAvailable(): boolean {
  const [available, setAvailable] = useState(false)
  useEffect(() => {
    const onMessage = (event: MessageEvent) => {
      if (event.source !== window.parent) return
      const message = event.data
      if (message === null || typeof message !== 'object') return
      if ((message as { type?: unknown }).type !== 'dsh-desktop:update-availability') return
      setAvailable((message as { available?: unknown }).available === true)
    }
    window.addEventListener('message', onMessage)
    postDesktopMessage('dsh-desktop:request-update-state')
    return () => { window.removeEventListener('message', onMessage) }
  }, [])
  return available
}

/**
 * 向上的箭头，从一条横线上抬起——比裸箭头更像"升级"而不是"滚动到顶部"。
 *
 * 底线在下、箭头在上，整体重心与 12px 的文字基线对齐，
 * 两端按钮共用同一尺寸，视觉上才一致。
 */
function UpdateArrowIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden="true">
      <path
        d="M8 10V3.6M8 3.6L5.2 6.4M8 3.6L10.8 6.4"
        stroke="currentColor"
        strokeWidth="1.7"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <path d="M4.4 12.6h7.2" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" />
    </svg>
  )
}

/** 标题栏的"新版本"按钮。点击后由宿主打开它已有的更新窗口。 */
function UpdateButton() {
  return (
    <button
      type="button"
      className="dshDesktopUpdateButton"
      title="有新版本可用，点击查看并更新"
      onPointerDown={(event) => {
        // 标题栏整条是原生拖动区，不拦住 pointerdown 的话按钮点不动。
        event.preventDefault()
        event.stopPropagation()
      }}
      onClick={() => postDesktopMessage('dsh-desktop:check-updates')}
    >
      <UpdateArrowIcon />
      新版本
    </button>
  )
}

export function AdvancedFrame({ layout, platform, renderSlot, useSessions }: AdvancedFrameProps) {
  const subscribeLayout = useCallback((listener: () => void) => layout.subscribe(listener), [layout])
  const readLayout = useCallback(() => layout.getSnapshot(), [layout])
  const panels = useSyncExternalStore(subscribeLayout, readLayout)
  const frameRef = useRef<HTMLDivElement>(null)
  const [viewport, setViewport] = useState(() => window.innerWidth)
  const updateAvailable = useUpdateAvailable()
  const detailsSession = useSessions((state) => {
    const current = state.current
    return current !== undefined && state.byId[current]?.blank === false ? current : undefined
  })

  // WKWebView 默认把未 preventDefault 的文件拖放当成导航：图片会全屏打开，
  // 把整个 iframe 换成 file URL。官方 ComposerAttachments 只在会话输入栏
  // 挂载后才监听；没有会话、或社区 dsh-file-upload 被摘掉之后，这里必须自己拦。
  // 捕获阶段只 preventDefault，不 stopPropagation，这样输入栏自己的 drop
  // 处理（读 dataTransfer.files）仍然能跑。
  useEffect(() => {
    const blockNavigation = (event: DragEvent) => {
      const types = event.dataTransfer?.types
      if (types === undefined || !types.includes('Files')) return
      event.preventDefault()
    }
    document.addEventListener('dragover', blockNavigation, true)
    document.addEventListener('drop', blockNavigation, true)
    return () => {
      document.removeEventListener('dragover', blockNavigation, true)
      document.removeEventListener('drop', blockNavigation, true)
    }
  }, [])

  useEffect(() => {
    const element = frameRef.current
    if (element === null) return
    let raf: number | null = null
    const observer = new ResizeObserver(() => {
      raf ??= requestAnimationFrame(() => {
        raf = null
        const width = element.getBoundingClientRect().width
        if (width > 0) setViewport(width)
      })
    })
    observer.observe(element)
    return () => {
      observer.disconnect()
      if (raf !== null) cancelAnimationFrame(raf)
    }
  }, [])

  const narrow = viewport < SIDEBAR_AUTO_COLLAPSE
  useEffect(() => { layout.setNarrow(narrow) }, [layout, narrow])

  const previousSession = useRef(detailsSession)
  useLayoutEffect(() => {
    if (detailsSession === undefined) return
    if (previousSession.current !== undefined && previousSession.current !== detailsSession) {
      layout.closeDetails()
    }
    previousSession.current = detailsSession
  }, [detailsSession, layout])

  const collapsed = narrow ? !panels.narrowExpanded : panels.sidebar === 0
  const sidebarPreference = collapsed ? 0 : panels.sidebar === 0 ? SIDEBAR_DEFAULT : panels.sidebar
  const collapsedWidth = platform === 'darwin' ? MACOS_SIDEBAR_COLLAPSED : SIDEBAR_COLLAPSED
  const columns = computeDesktopColumns(
    viewport,
    sidebarPreference,
    detailsSession === undefined ? 0 : panels.details,
    collapsedWidth,
  )
  // Official AppFrame passes a hypothetical rightbar width (preference, not the
  // visual 0 when closed). RightbarSeat collapses immediately when canShow is
  // falsy, which is why 「打开侧边栏」 previously did nothing on both platforms.
  const rightbarPreference = panels.details === 0 ? DETAILS_DEFAULT : panels.details
  const normalRightbar = computeDesktopColumns(viewport, sidebarPreference, rightbarPreference, collapsedWidth)
  // macOS keeps a wider native rail around the centered upstream sidebar,
  // while the public owner contract still reports the rendered 56px rail.
  const sidebarOwnerWidth = collapsed ? SIDEBAR_COLLAPSED : columns.sidebar
  const columnsRef = useRef(columns)
  columnsRef.current = columns
  const requestNativeDrag = useCallback((event: React.PointerEvent<HTMLElement>) => {
    if (event.button !== 0) return
    postDesktopMessage('dsh-desktop:start-dragging')
  }, [])

  const sidebarBase = useRef(0)
  const detailsBase = useRef(0)
  const [dragging, setDragging] = useState(false)
  const onDragEnd = useCallback(() => { setDragging(false) }, [])
  const onSidebarStart = useCallback(() => {
    sidebarBase.current = columnsRef.current.sidebar
    setDragging(true)
  }, [])
  const onDetailsStart = useCallback(() => {
    detailsBase.current = columnsRef.current.details
    setDragging(true)
  }, [])
  const onSidebarDrag = useCallback((dx: number) => {
    layout.setSidebar(sidebarBase.current + dx)
  }, [layout])
  const onDetailsDrag = useCallback((dx: number) => {
    layout.setDetails(detailsBase.current - dx)
  }, [layout])

  return (
    <div
      ref={frameRef}
      className="dshDesktopFrame"
      data-desktop-platform={platform}
      data-sidebar-collapsed={collapsed || undefined}
      data-details-collapsed={columns.details === 0 || undefined}
      data-dragging={dragging || undefined}
      style={{ gridTemplateColumns: `${columns.sidebar}px minmax(0, 1fr) ${columns.details}px` }}
    >
      {platform === 'darwin' && (
        <div className="dshDesktopMacCaptionRow" onPointerDown={requestNativeDrag}>
          {updateAvailable && <UpdateButton />}
        </div>
      )}
      {platform === 'win32' && (
        // 标题栏只在有新版本时显示一个按钮：客户端设置已迁进 DSH 原生设置，
        // 从侧边栏齿轮进入；没有更新时这里保持空白，不占视觉。
        <div className="dshDesktopWindowsCaptionRow" onPointerDown={requestNativeDrag}>
          {updateAvailable && <UpdateButton />}
        </div>
      )}
      <aside className="dshDesktopSidebarSurface" onPointerDown={(event) => {
        // 侧边栏在两个平台上都通栏到窗口顶部，那条顶边要能拖动窗口。
        // macOS 还要额外避开左上角的交通灯，否则会挡住关闭按钮。
        if (event.clientY > WINDOWS_TITLEBAR_HEIGHT) return
        if (platform === 'darwin') {
          if (event.clientY <= 32 && event.clientX >= MACOS_TRAFFIC_LIGHT_SAFE_WIDTH) requestNativeDrag(event)
          return
        }
        if (platform === 'win32') requestNativeDrag(event)
      }}>
        <div className="dshDesktopUpstreamSidebar">
          {renderSlot('sidebar', { collapsed, width: sidebarOwnerWidth })}
        </div>
      </aside>
      <main className="dshDesktopConversationSurface">
        {renderSlot('main', {}, { entryKey: panels.activePanelId ?? 'conversation' })}
      </main>
      {/*
        0.1.5 的 rightbar 是 root-scoped，始终挂载。官方 RightbarRoot 自己处理
        会话级子插槽。没有真实会话时只把列宽收成 0，不要卸载插槽——卸载会让
        官方 ExpandButton / 会话详情永远挂不上去。
      */}
      <aside className="dshDesktopDetailsSurface">
        {renderSlot('rightbar', {
          width: normalRightbar.details,
          viewportWidth: viewport,
          canShow: normalRightbar.details > 0,
        })}
      </aside>
      <div className="dshDesktopOverlay" data-shell-overlay>
        {renderSlot('shell.overlay', {})}
      </div>
      {!collapsed && (
        <ResizeHandle
          side="sidebar"
          left={columns.sidebar}
          onStart={onSidebarStart}
          onDrag={onSidebarDrag}
          onEnd={onDragEnd}
        />
      )}
      {columns.details > 0 && (
        <ResizeHandle
          side="details"
          left={viewport - columns.details}
          onStart={onDetailsStart}
          onDrag={onDetailsDrag}
          onEnd={onDragEnd}
        />
      )}
    </div>
  )
}

function ResizeHandle(props: {
  side: 'sidebar' | 'details'
  left: number
  onStart: () => void
  onDrag: (dx: number) => void
  onEnd: () => void
}) {
  const [dragging, setDragging] = useState(false)
  const origin = useRef(0)
  const latest = useRef(0)
  const frame = useRef<number | null>(null)
  const callbacks = useRef({ onStart: props.onStart, onDrag: props.onDrag, onEnd: props.onEnd })
  callbacks.current = { onStart: props.onStart, onDrag: props.onDrag, onEnd: props.onEnd }

  const onPointerDown = useCallback((event: React.PointerEvent<HTMLDivElement>) => {
    event.preventDefault()
    event.currentTarget.setPointerCapture(event.pointerId)
    origin.current = event.clientX
    latest.current = event.clientX
    callbacks.current.onStart()
    setDragging(true)
  }, [])
  const onPointerMove = useCallback((event: React.PointerEvent<HTMLDivElement>) => {
    if (!event.currentTarget.hasPointerCapture(event.pointerId)) return
    latest.current = event.clientX
    frame.current ??= requestAnimationFrame(() => {
      frame.current = null
      callbacks.current.onDrag(latest.current - origin.current)
    })
  }, [])
  const onPointerUp = useCallback((event: React.PointerEvent<HTMLDivElement>) => {
    if (!event.currentTarget.hasPointerCapture(event.pointerId)) return
    event.currentTarget.releasePointerCapture(event.pointerId)
    if (frame.current !== null) {
      cancelAnimationFrame(frame.current)
      frame.current = null
    }
    callbacks.current.onDrag(latest.current - origin.current)
    setDragging(false)
    callbacks.current.onEnd()
  }, [])
  return (
    <div
      className="dshDesktopResizeHandle"
      data-side={props.side}
      data-dragging={dragging || undefined}
      style={{ left: props.left }}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
    />
  )
}
