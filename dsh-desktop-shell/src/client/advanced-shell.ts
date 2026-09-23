// 内核 0.1.2 起移除了 dsh-client-runtime，这个类型改由 cordis 导出。
// 官方插件（dsh-client-ui-jobs 等）都是这个写法。
import type { Context as ClientContext } from '@deepseek-ai/cordis'
import type { ThemeSnapshot } from '@deepseek-ai/dsh-client-ui-theme/client'
import * as primitives from '@deepseek-ai/dsh-client-ui-primitives'
import type {} from './contracts.ts'
import type { DesktopClientEnvironment } from './environment.ts'
import { AdvancedFrame } from './AdvancedFrame.tsx'
import { DesktopSettings, REQUIRED_PRIMITIVES } from './DesktopSettings.tsx'
import { DesktopLayoutState } from './layout-state.ts'
import { provideDesktopLayout } from './layout-service.ts'
import { installAdvancedStyles } from './styles.ts'
import { DesktopThemePresenter } from './theme-presenter.ts'

/**
 * Provide the advanced layout service and own the desktop root slot.
 * @param ctx - active browser Cordis context.
 * @param environment - validated mode and platform marker.
 */
export function applyAdvancedShell(ctx: ClientContext, environment: DesktopClientEnvironment): void {
  if (environment.mode !== 'advanced') {
    throw new Error(`dsh-plugin-desktop: advanced shell received mode ${JSON.stringify(environment.mode)}`)
  }

  const desktopLayout = new DesktopLayoutState()
  ctx.effect(
    () => provideDesktopLayout(ctx, desktopLayout),
    'desktop: layout service',
  )

  ctx.effect(() => {
    document.body.dataset.dshDesktopMode = 'advanced'
    document.body.dataset.dshDesktopPlatform = environment.platform
    const removeStyles = installAdvancedStyles()
    return () => {
      removeStyles()
      delete document.body.dataset.dshDesktopMode
      delete document.body.dataset.dshDesktopPlatform
    }
  }, 'desktop: advanced shell styles')

  ctx.effect(() => {
    const presenter = new DesktopThemePresenter()
    presenter.apply(ctx.theme.getTheme())
    const off = ctx.events.on('theme/change', (snapshot: ThemeSnapshot) => {
      presenter.apply(snapshot)
    })
    return () => {
      off()
      presenter.dispose()
    }
  }, 'desktop: theme presenter')

  ctx.effect(() => {
    const disposeRoot = ctx.slots.register({
      name: 'root',
      children: {
        'sidebar': { kind: 'single', scope: 'root' },
        'main': { kind: 'keyed', scope: 'root' },
        'rightbar': { kind: 'single', scope: 'root' },
        'shell.overlay': { kind: 'list', scope: 'root' },
      },
      inject: () => ({ layout: desktopLayout, platform: environment.platform }),
    }, AdvancedFrame)
    const retainMainPanels = () => {
      desktopLayout.retainMainPanels(
        ctx.slots.entries('main').flatMap((entry) => {
          const key = entry.options.key
          return typeof key === 'string' ? [key] : []
        }),
      )
    }
    const disposePanels = ctx.slots.subscribe('main', retainMainPanels)
    retainMainPanels()
    return () => {
      disposePanels()
      disposeRoot()
    }
  }, 'desktop: advanced root slot')

  registerDesktopSettings(ctx)
}

/**
 * 把桌面客户端设置注册成 DSH 原生设置里的一个分区。
 *
 * `settings.section` 只在设置外壳挂载期间存在，所以必须用 `slots.inject`
 * 包一层，不能直接 register。order 取 50 排在内置分区之后
 * （通用 0、模型 10、插件 15、智能体预设 20、插件市场 40）。
 *
 * primitives 由宿主的固定模块表提供，不在依赖图里。旧版宿主会把新导出解析成
 * `undefined`，渲染时抛错会让整个设置对话框白屏——所以先确认用到的导出都在，
 * 缺失时放弃注册，只丢掉本分区。
 */
function registerDesktopSettings(ctx: ClientContext): void {
  const missing = REQUIRED_PRIMITIVES.filter(
    (name) => (primitives as unknown as Record<string, unknown>)[name] === undefined,
  )
  if (missing.length > 0) {
    console.warn(
      `[dsh-desktop-shell] 宿主 ui-primitives 缺少 ${missing.join('、')}，桌面设置分区已跳过`,
    )
    return
  }
  ctx.effect(() => ctx.slots.inject('settings.section', () => ctx.slots.register({
    name: 'settings.section',
    id: 'desktop',
    order: 50,
    label: () => '客户端设置',
  }, DesktopSettings)), 'desktop: settings section')
}
