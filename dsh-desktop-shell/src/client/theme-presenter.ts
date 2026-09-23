import type { ThemeSnapshot } from '@deepseek-ai/dsh-client-ui-theme/client'

const DARK_ATTRIBUTE = 'data-ds-dark-theme'

/** Projects the resolved theme service snapshot onto the desktop document. */
export class DesktopThemePresenter {
  private appliedTokens: string[] = []
  private readonly themeColorMeta = document.createElement('meta')
  private reportedDark: boolean | undefined

  constructor() {
    this.themeColorMeta.name = 'theme-color'
  }

  /** @param snapshot - current resolved palette and token overrides. */
  apply(snapshot: ThemeSnapshot): void {
    const scheme = snapshot.active.colorScheme
    document.documentElement.style.colorScheme = scheme
    document.body.style.colorScheme = scheme
    if (scheme === 'dark') document.body.setAttribute(DARK_ATTRIBUTE, '')
    else document.body.removeAttribute(DARK_ATTRIBUTE)
    for (const name of this.appliedTokens) document.body.style.removeProperty(name)
    this.appliedTokens = []
    for (const [name, value] of Object.entries(snapshot.active.tokens)) {
      document.body.style.setProperty(name, value)
      this.appliedTokens.push(name)
    }
    this.themeColorMeta.content = getComputedStyle(document.body).backgroundColor
    if (!this.themeColorMeta.isConnected) document.head.appendChild(this.themeColorMeta)
    this.reportToHost(scheme === 'dark')
  }

  /**
   * Tell the launcher which scheme is active.
   *
   * The launcher paints the Windows caption controls itself, outside this
   * document — without this signal it can only follow the OS preference, so
   * switching the theme inside DSH would leave dark glyphs on a dark caption.
   */
  private reportToHost(dark: boolean): void {
    if (this.reportedDark === dark) return
    this.reportedDark = dark
    if (window.parent === window) return
    window.parent.postMessage({ type: 'theme-change', dark }, '*')
  }

  /** Remove only DOM state owned by this presenter. */
  dispose(): void {
    document.documentElement.style.removeProperty('color-scheme')
    document.body.style.removeProperty('color-scheme')
    document.body.removeAttribute(DARK_ATTRIBUTE)
    for (const name of this.appliedTokens) document.body.style.removeProperty(name)
    this.appliedTokens = []
    this.themeColorMeta.remove()
    this.reportedDark = undefined
  }
}
