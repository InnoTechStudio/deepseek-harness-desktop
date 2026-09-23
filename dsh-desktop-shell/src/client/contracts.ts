/** Sidebar geometry passed by the desktop root slot. */
export interface DesktopSidebarOwnerProps {
  /** Whether the sidebar is showing its compact rail. */
  collapsed: boolean
  /** Current rendered sidebar width. */
  width: number
}

/**
 * Public panel transitions consumed by conversation and sidebar plugins.
 *
 * Must stay a superset of official `ILayout`. The desktop shell replaces
 * `ui-layout`'s LayoutController; ui-workspace / ui-sidebar call the official
 * methods directly on `ctx.layout`.
 */
export interface DesktopLayoutService {
  /** Toggle the sidebar between wide and compact presentation. */
  toggleSidebar(): void
  /** Open the current session's details panel. */
  openDetails(): void
  /** Close the details panel. */
  closeDetails(): void
  /** Start an async navigation; abort any earlier pending one. */
  beginNavigation(): AbortSignal
  /** Select a global main panel, or `null` to show the conversation. */
  selectPanel(panelId: string | null): void
  /** Official rightbar show; desktop maps this onto the details column. */
  openRightbar(track: boolean, fullscreen: boolean): void
  /** Official rightbar hide; desktop maps this onto the details column. */
  closeRightbar(): void
  /** Abort pending navigations when the layout owner is unloaded. */
  dispose(): void
}

declare module '@deepseek-ai/cordis' {
  interface Context {
    /** Desktop-owned layout service in advanced mode. */
    layout: DesktopLayoutService
  }
}

declare module '@deepseek-ai/dsh-client-ui-slots' {
  interface SlotMap {
    /** Upstream sidebar hosted by the desktop advanced frame. */
    'sidebar': { kind: 'single'; scope: 'root'; owner: DesktopSidebarOwnerProps }
    /**
     * Official 0.1.5 center surface. ui-conversation injects
     * `{ name: "main", key: "conversation" }`; other panels register other keys.
     */
    'main': { kind: 'keyed'; scope: 'root'; owner: Record<never, never> }
    /**
     * Official 0.1.5 right panel. ui-sidebar-right injects this as a
     * root-scoped single; children such as `rightbar.session` stay session-scoped.
     *
     * Owner matches official AppFrame: a *hypothetical* width used by
     * RightbarSeat to decide whether ExpandButton is allowed to stay open.
     * Passing `{}` leaves `canShow` falsy and the button immediately collapses.
     */
    'rightbar': {
      kind: 'single';
      scope: 'root';
      owner: { width: number; viewportWidth: number; canShow: boolean };
    }
    /** Frame-wide additive overlays. */
    'shell.overlay': { kind: 'list'; scope: 'root' }
  }
}
