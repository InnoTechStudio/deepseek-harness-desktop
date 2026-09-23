import type { Context } from '@deepseek-ai/cordis'
import type {} from '@deepseek-ai/dsh-client-ui-slots'
import type {} from '@deepseek-ai/dsh-client-ui-theme/client'
import { applyAdvancedShell } from './advanced-shell.ts'
import { desktopEnvironment } from './environment.ts'
import { installWindowsOpenInAppBridge } from './open-in-app-bridge.ts'

export const inject = ['slots', 'sessions', 'theme', 'locale']

export function apply(ctx: Context): void {
  const environment = desktopEnvironment()
  ctx.effect(
    () => installWindowsOpenInAppBridge(environment.platform),
    'desktop: windows open-in-app bridge',
  )
  try {
    applyAdvancedShell(ctx as never, environment)
  } catch (error) {
    console.error('[dsh-desktop-shell] advanced layout unavailable; keeping official layout', error)
  }
}
