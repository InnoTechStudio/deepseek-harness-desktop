export type DesktopClientPlatform = 'darwin' | 'win32' | 'linux'
export interface DesktopClientEnvironment {
  mode: 'advanced'
  platform: DesktopClientPlatform
}

export function desktopEnvironment(): DesktopClientEnvironment {
  const platform: DesktopClientPlatform = /Mac/i.test(navigator.platform)
    ? 'darwin'
    : /Win/i.test(navigator.platform) ? 'win32' : 'linux'
  return { mode: 'advanced', platform }
}
