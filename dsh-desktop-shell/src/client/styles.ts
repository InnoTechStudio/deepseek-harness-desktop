import {
  MACOS_CAPTION_ROW_HEIGHT,
  MACOS_DRAG_REGION_HEIGHT,
  MACOS_TITLEBAR_HEIGHT,
  MACOS_TRAFFIC_LIGHT_SAFE_WIDTH,
  WINDOWS_CAPTION_CONTROLS_WIDTH,
  WINDOWS_TITLEBAR_HEIGHT,
} from '../window-chrome.ts'
import { SIDEBAR_COLLAPSED } from './layout-state.ts'

/** Advanced-shell stylesheet kept as a plain string so the package client bundle stays self-contained. */
const ADVANCED_STYLES = `
html, body, #root { width: 100%; height: 100%; }
body[data-dsh-desktop-mode="advanced"] { margin: 0; background: transparent !important; }
.dshDesktopFrame { position: relative; display: grid; grid-template-rows: 100%; width: 100%; height: 100%; overflow: hidden; background: var(--dsw-alias-bg-base); transition: grid-template-columns var(--ds-transition-duration-slow) var(--ds-ease-in-out); }
.dshDesktopSidebarSurface { --dsw-specific-sidebar-fill: transparent; position: relative; grid-column: 1; grid-row: 1; min-width: 0; overflow: hidden; background: transparent; border-right: 1px solid var(--dsw-alias-border-l1); }
.dshDesktopUpstreamSidebar { box-sizing: border-box; width: 100%; height: 100%; }
.dshDesktopFrame[data-desktop-platform="darwin"] .dshDesktopUpstreamSidebar { padding-top: ${MACOS_TITLEBAR_HEIGHT}px; -webkit-app-region: no-drag; }
.dshDesktopFrame[data-desktop-platform="darwin"][data-sidebar-collapsed] .dshDesktopUpstreamSidebar { width: ${SIDEBAR_COLLAPSED}px; margin: 0 auto; }
.dshDesktopFrame[data-desktop-platform="darwin"] { grid-template-rows: ${MACOS_CAPTION_ROW_HEIGHT}px minmax(0, 1fr); }
.dshDesktopFrame[data-desktop-platform="darwin"] .dshDesktopSidebarSurface { grid-row: 1 / -1; -webkit-app-region: no-drag; }
.dshDesktopFrame[data-desktop-platform="darwin"] .dshDesktopConversationSurface,
.dshDesktopFrame[data-desktop-platform="darwin"] .dshDesktopDetailsSurface { grid-row: 2; }
.dshDesktopFrame[data-desktop-platform="darwin"] .dshDesktopSidebarSurface::before { content: ""; position: absolute; z-index: 100; top: 0; right: 0; left: ${MACOS_TRAFFIC_LIGHT_SAFE_WIDTH}px; height: ${MACOS_DRAG_REGION_HEIGHT}px; user-select: none; -webkit-app-region: drag; }
.dshDesktopMacCaptionRow { position: relative; z-index: 100; grid-column: 2 / -1; grid-row: 1; min-width: 0; background: var(--dsw-alias-bg-base); }
.dshDesktopMacCaptionRow::before { content: ""; position: absolute; inset: 0; user-select: none; -webkit-app-region: drag; }
/* 按钮要压在拖动层之上，否则点击会被拖动区吃掉。 */
.dshDesktopMacCaptionRow .dshDesktopUpdateButton { z-index: 2; }
.dshDesktopConversationSurface { grid-column: 2; grid-row: 1; min-width: 0; min-height: 0; display: flex; flex-direction: column; overflow: hidden; background: var(--dsw-alias-bg-base); }
/*
	  Official SidebarPanel is position:absolute; top/bottom/right:0 and slides
	  with translate(100%). Official AppFrame therefore gives the right column
	  position:relative; overflow:visible so those offsets are the column, not
	  the window, and so the dock-kit chrome (collapse / fullscreen) is not
	  clipped into the caption row. Frame overflow:hidden still clips the slide.
	*/
	.dshDesktopDetailsSurface { grid-column: 3; grid-row: 1; position: relative; min-width: 0; min-height: 0; overflow: visible; background: var(--dsw-alias-bg-base); border-left: 1px solid var(--dsw-alias-border-l2); }
.dshDesktopFrame[data-details-collapsed] .dshDesktopDetailsSurface { border-left: none; }
/*
  Windows 跟 macOS 同一套网格：会话/详情让出顶部 32px 给最小化/最大化/关闭，
  侧边栏通栏到窗口顶边、不加 padding。宿主的三个按钮画在 webview 物理右上角，
  正好落在这条预留带上，不再压住会话头部的日志下载 / 在本地打开 / 打开侧边栏。

  不要把侧边栏也放到第二行——整块界面会凭空矮一截，看起来不像同一个窗口。
*/
.dshDesktopFrame[data-desktop-platform="win32"] { grid-template-rows: ${WINDOWS_TITLEBAR_HEIGHT}px minmax(0, 1fr); background: var(--dsw-alias-bg-base); }
.dshDesktopFrame[data-desktop-platform="win32"] .dshDesktopSidebarSurface { grid-row: 1 / -1; }
.dshDesktopFrame[data-desktop-platform="win32"] .dshDesktopConversationSurface,
.dshDesktopFrame[data-desktop-platform="win32"] .dshDesktopDetailsSurface { grid-row: 2; }
.dshDesktopWindowsCaptionRow { position: relative; z-index: 100; grid-column: 2 / -1; grid-row: 1; min-width: 0; background: var(--dsw-alias-bg-base); user-select: none; }
.dshDesktopWindowsCaptionRow::before { content: ""; position: absolute; inset: 0 ${WINDOWS_CAPTION_CONTROLS_WIDTH}px 0 0; pointer-events: auto; user-select: none; -webkit-app-region: drag; }
.dshDesktopUpdateButton { position: absolute; z-index: 2; top: 4px; right: ${WINDOWS_CAPTION_CONTROLS_WIDTH + 8}px; display: inline-flex; align-items: center; gap: 5px; height: 24px; padding: 0 12px 0 10px; border: 0; border-radius: 999px; color: #fff; background: var(--dsw-alias-state-success-primary); font: 500 12px/1 inherit; letter-spacing: .2px; white-space: nowrap; cursor: pointer; pointer-events: auto; -webkit-app-region: no-drag !important; box-shadow: 0 1px 3px rgba(0, 0, 0, .16); }
.dshDesktopUpdateButton:hover { background: var(--dsw-alias-state-success-secondary); }
.dshDesktopUpdateButton:active { transform: translateY(.5px); }
.dshDesktopUpdateButton:focus-visible { outline: 2px solid var(--dsw-alias-state-success-primary); outline-offset: 2px; }
/* 到上边和右边的距离相等，按钮才像是贴着角摆放，而不是被挤在角上。 */
.dshDesktopFrame[data-desktop-platform="darwin"] .dshDesktopUpdateButton { top: 6px; right: 6px; }
.dshDesktopFrame[data-dragging] { transition: none; }
.dshDesktopOverlay { position: absolute; z-index: 1000; inset: 0; pointer-events: none; }
.dshDesktopOverlay > * { pointer-events: auto; }
.dshDesktopResizeHandle { position: absolute; z-index: 50; top: 0; bottom: 0; width: 8px; margin-left: -4px; cursor: col-resize; touch-action: none; -webkit-app-region: no-drag; transition: left var(--ds-transition-duration-slow) var(--ds-ease-in-out); }
.dshDesktopFrame[data-dragging] .dshDesktopResizeHandle { transition: none; }
.dshDesktopNoDrag, button, input, textarea, select, a, [contenteditable="true"], [role="button"], [role="checkbox"], [role="dialog"], [role="menu"], [role="menuitem"], [role="option"], [role="switch"], [role="tab"] { -webkit-app-region: no-drag !important; }
[role="dialog"], [aria-modal="true"] { -webkit-app-region: no-drag !important; }
html:has([aria-modal="true"]) .dshDesktopWindowsCaptionRow::before,
html:has([aria-modal="true"]) .dshDesktopMacCaptionRow::before,
html:has([aria-modal="true"]) .dshDesktopSidebarSurface,
html:has([aria-modal="true"]) .dshDesktopSidebarSurface::before { -webkit-app-region: no-drag !important; }
@media (prefers-reduced-motion: reduce) {
  .dshDesktopFrame,
  .dshDesktopResizeHandle { transition: none !important; }
}
`

/** Install and remove the advanced shell's global native-window styles. @returns the style disposer. */
export function installAdvancedStyles(): () => void {
  const style = document.createElement('style')
  style.dataset.plugin = 'dsh-plugin-desktop'
  style.dataset.pluginCss = 'dsh-plugin-desktop/advanced-shell'
  style.textContent = ADVANCED_STYLES
  document.head.appendChild(style)
  return () => { style.remove() }
}
