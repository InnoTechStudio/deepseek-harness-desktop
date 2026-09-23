/** Official panel-info shape consumed by `usePanelInfo` in sidebar/workspace. */
export interface DesktopPanelInfo {
  /** Selected global main-slot key; `null` shows the conversation. */
  readonly activePanelId: string | null
}

/** Advanced-shell panel state shared by the root slot and layout-service adapter. */
export interface DesktopLayoutSnapshot {
  /** Preferred sidebar width; zero means the compact rail. */
  sidebar: number
  /** Preferred details width; zero means closed. */
  details: number
  /** Whether the current viewport is below the automatic-collapse breakpoint. */
  narrow: boolean
  /** Manual narrow-screen override that temporarily expands the rail. */
  narrowExpanded: boolean
  /** Official `panelInfo.activePanelId`; `null` renders the conversation key. */
  activePanelId: string | null
  /**
   * Nested frozen object for `usePanelInfo` / `useSyncExternalStore`.
   * Must keep a stable reference while `activePanelId` is unchanged.
   */
  panelInfo: DesktopPanelInfo
}

/** Column geometry after preserving the center surface. */
export interface DesktopColumns {
  /** Rendered sidebar width. */
  sidebar: number
  /** Rendered center width. */
  center: number
  /** Rendered details width. */
  details: number
}

/** Compatibility-mode compact rail used by the upstream Windows sidebar. */
export const SIDEBAR_COLLAPSED = 56
/** Wider compact rail reserved for the desktop-owned macOS sidebar. */
export const MACOS_SIDEBAR_COLLAPSED = 90
export const SIDEBAR_DEFAULT = 280
export const SIDEBAR_MIN = 264
export const SIDEBAR_MAX = 420
export const SIDEBAR_AUTO_COLLAPSE = 1024
export const DETAILS_DEFAULT = 360
export const DETAILS_MIN = 300
export const DETAILS_MAX = 520
/** Official `ui-layout` floor. 640 会让默认 1200 宽窗口永远算不出详情列。 */
export const CENTER_MIN = 400

/**
 * Resolve three desktop columns without allowing details to squeeze the conversation below its floor.
 * @param viewport - available frame width.
 * @param sidebar - sidebar preference, where zero selects the compact rail.
 * @param details - details preference, where zero closes the panel.
 * @returns rendered column widths.
 */
export function computeDesktopColumns(
  viewport: number,
  sidebar: number,
  details: number,
  collapsedWidth: number = SIDEBAR_COLLAPSED,
): DesktopColumns {
  const sidebarWidth = sidebar === 0 ? collapsedWidth : clamp(sidebar, SIDEBAR_MIN, SIDEBAR_MAX)
  const preferredDetails = details === 0 ? 0 : clamp(details, DETAILS_MIN, DETAILS_MAX)
  if (sidebarWidth + preferredDetails + CENTER_MIN <= viewport) {
    return { sidebar: sidebarWidth, center: viewport - sidebarWidth - preferredDetails, details: preferredDetails }
  }
  const reducedDetails = preferredDetails === 0 ? 0 : Math.max(DETAILS_MIN, viewport - sidebarWidth - CENTER_MIN)
  if (sidebarWidth + reducedDetails + CENTER_MIN <= viewport) {
    return { sidebar: sidebarWidth, center: CENTER_MIN, details: reducedDetails }
  }
  return { sidebar: sidebarWidth, center: Math.max(0, viewport - sidebarWidth), details: 0 }
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, Math.round(value)))
}

/**
 * Small observable panel controller used by the advanced root registration.
 *
 * Also implements the official `ILayout` methods (`beginNavigation`, `selectPanel`,
 * `openRightbar`, `closeRightbar`). The desktop shell disables `ui-layout`, so
 * nothing else provides `LayoutController`. ui-workspace's 新建会话 calls
 * `beginNavigation()` then `selectPanel(null)`; missing either is a TypeError
 * swallowed as `console.warn("new session failed:")`, and the app looks idle.
 */
const INITIAL_PANEL_INFO: DesktopPanelInfo = Object.freeze({ activePanelId: null })

export class DesktopLayoutState {
  private snapshot: DesktopLayoutSnapshot = Object.freeze({
    sidebar: SIDEBAR_DEFAULT,
    details: 0,
    narrow: false,
    narrowExpanded: false,
    activePanelId: null,
    panelInfo: INITIAL_PANEL_INFO,
  })
  private readonly listeners = new Set<() => void>()
  private navigation = new AbortController()

  /** @returns the immutable current panel snapshot. */
  getSnapshot(): DesktopLayoutSnapshot {
    return this.snapshot
  }

  /** @param listener - callback notified after a snapshot replacement. @returns its disposer. */
  subscribe(listener: () => void): () => void {
    this.listeners.add(listener)
    return () => { this.listeners.delete(listener) }
  }

  /** Toggle the wide sidebar and the platform-selected compact rail. */
  toggleSidebar(): void {
    if (this.snapshot.narrow) {
      this.publish({ ...this.snapshot, narrowExpanded: !this.snapshot.narrowExpanded })
      return
    }
    this.publish({ ...this.snapshot, sidebar: this.snapshot.sidebar === 0 ? SIDEBAR_DEFAULT : 0 })
  }

  /** @param narrow - whether the frame is below the automatic-collapse breakpoint. */
  setNarrow(narrow: boolean): void {
    if (this.snapshot.narrow === narrow) return
    this.publish({ ...this.snapshot, narrow, narrowExpanded: false })
  }

  /** Open details at its default width. */
  openDetails(): void {
    if (this.snapshot.details === 0) this.publish({ ...this.snapshot, details: DETAILS_DEFAULT })
  }

  /** Close details while keeping its slot mounted. */
  closeDetails(): void {
    if (this.snapshot.details !== 0) this.publish({ ...this.snapshot, details: 0 })
  }

  /** @param width - requested sidebar width from a resize gesture. */
  setSidebar(width: number): void {
    this.publish({ ...this.snapshot, sidebar: clamp(width, SIDEBAR_MIN, SIDEBAR_MAX) })
  }

  /** @param width - requested details width from a resize gesture. */
  setDetails(width: number): void {
    this.publish({ ...this.snapshot, details: clamp(width, DETAILS_MIN, DETAILS_MAX) })
  }

  /**
   * Start an asynchronous navigation, aborting any earlier pending one.
   * ui-workspace wraps this with `AbortSignal.any` and checks it before
   * committing `sessions.open`.
   */
  beginNavigation(): AbortSignal {
    this.navigation.abort()
    this.navigation = new AbortController()
    return this.navigation.signal
  }

  /**
   * Official main-slot selection. `null` shows the conversation; a non-null id
   * is stored so `renderSlot("main", {}, { entryKey })` can switch panels.
   *
   * Official `LayoutController` throws if the id is not yet registered. Desktop
   * does not: sidebar PanelRow may select a panel before its `main` occupant
   * has injected. Unknown ids still become `activePanelId` so the keyed slot
   * can resolve them once they appear.
   */
  selectPanel(panelId: string | null): void {
    this.navigation.abort()
    const next = panelId === null ? null : panelId
    if (this.snapshot.activePanelId === next) return
    this.publish({ ...this.snapshot, activePanelId: next })
  }

  /**
   * Reset `activePanelId` when the selected main key is no longer registered.
   * Mirrors official `retainMainPanels`.
   */
  retainMainPanels(panelIds: readonly string[]): void {
    const current = this.snapshot.activePanelId
    if (current === null || panelIds.includes(current)) return
    this.publish({ ...this.snapshot, activePanelId: null })
  }

  /** Snapshot consumed by the official `usePanelInfo` root hook. */
  getPanelInfo(): DesktopPanelInfo {
    return this.snapshot.panelInfo
  }

  /** Map the official rightbar show onto the desktop details column. */
  openRightbar(_track: boolean, _fullscreen: boolean): void {
    this.openDetails()
  }

  /** Map the official rightbar hide onto the desktop details column. */
  closeRightbar(): void {
    this.closeDetails()
  }

  /** Abort pending navigations when the layout owner is unloaded. */
  dispose(): void {
    this.navigation.abort()
  }

  private publish(next: Omit<DesktopLayoutSnapshot, 'panelInfo'>): void {
    const panelInfo = next.activePanelId === this.snapshot.panelInfo.activePanelId
      ? this.snapshot.panelInfo
      : Object.freeze({ activePanelId: next.activePanelId })
    this.snapshot = Object.freeze({ ...next, panelInfo })
    for (const listener of this.listeners) listener()
  }
}
