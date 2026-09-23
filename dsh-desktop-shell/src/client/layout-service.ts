// 内核 0.1.2 起移除了 dsh-client-runtime，这个类型改由 cordis 导出。
// 官方插件（dsh-client-ui-jobs 等）都是这个写法。
import type { Context as ClientContext } from '@deepseek-ai/cordis'
import type {} from './contracts.ts'
import type { DesktopLayoutState } from './layout-state.ts'

/**
 * Root-hook installer used by official `ui-layout`. The slots types package
 * does not declare `provideRoot`; the runtime SlotsService still exposes it.
 */
interface SlotsRootProvider {
  provideRoot(value: {
    hooks: {
      panelInfo: {
        getSnapshot: () => { activePanelId: string | null }
        subscribe: (listener: () => void) => () => void
      }
    }
  }): () => void
}

/**
 * Provide the advanced layout service for one plugin-fiber lifetime.
 *
 * Also installs the official `panelInfo` root hook. WorkspaceBrowser /
 * SessionTree / FlatList / SearchResults all call `usePanelInfo(...)`
 * unconditionally; missing it throws while rendering the session tree, which
 * looks like an empty history list with the New Session chrome still visible.
 *
 * @param ctx - active browser Cordis context.
 * @param layout - desktop-owned layout implementation.
 * @returns disposer for the service registration.
 */
export function provideDesktopLayout(ctx: ClientContext, layout: DesktopLayoutState): () => void {
  const disposeService = ctx.reflect.provide('layout', layout)
  const disposePanelInfo = (ctx.slots as unknown as SlotsRootProvider).provideRoot({
    hooks: {
      panelInfo: {
        getSnapshot: () => layout.getPanelInfo(),
        subscribe: (listener) => layout.subscribe(listener),
      },
    },
  })
  return () => {
    layout.dispose()
    disposePanelInfo()
    void disposeService()
  }
}
