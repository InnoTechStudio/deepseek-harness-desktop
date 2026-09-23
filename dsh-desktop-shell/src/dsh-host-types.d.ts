import type { IncomingMessage, ServerResponse } from 'node:http'

declare module '@deepseek-ai/cordis' {
  interface Context {
    effect<T>(effect: () => T, name?: string): T
    slots: {
      inject(name: string, callback: () => unknown): unknown
      register(meta: Record<string, unknown>, component: unknown): () => void
      entries(key: string): ReadonlyArray<{ options: { key?: string } }>
      subscribe(key: string, fn: () => void): () => void
    }
    workspaceRegistry: {
      list(): Array<{ path: string }>
    }
    webServer: {
      register(route: {
        kind: 'prefix' | 'exact'
        path: string
        handler: (req: IncomingMessage, res: ServerResponse) => void | Promise<void>
      }): () => void
    }
  }
}
