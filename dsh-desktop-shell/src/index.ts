import type { IncomingMessage, ServerResponse } from 'node:http'
import { homedir } from 'node:os'
import { join } from 'node:path'
import { readFileSync } from 'node:fs'
import type { Context } from '@deepseek-ai/cordis'

export const inject = ['webServer']

export function apply(ctx: Context): void {
  ctx.effect(() => ctx.webServer.register({
    kind: 'exact',
    path: '/dsh-desktop-shell/status',
    handler: (_req: IncomingMessage, res: ServerResponse) => {
      const home = process.env.DSH_HOME || join(homedir(), '.dsh')
      const profile = join(home, 'profiles', 'web', 'package.json')
      let manifest: any = {}
      try { manifest = JSON.parse(readFileSync(profile, 'utf8')) } catch { /* unavailable */ }
      res.writeHead(200, { 'content-type': 'application/json; charset=utf-8', 'cache-control': 'no-store' })
      res.end(JSON.stringify({ profile: 'web', dshHome: home, bundles: manifest?.dsh?.profile?.bundles ?? [], dependencies: manifest?.dependencies ?? {} }))
    },
  }), 'dsh-desktop-shell: status route')
}
