/**
 * macOS 侧边栏的顶部留白：让侧边栏内容避开左上角的红绿灯。
 *
 * 这个值只管侧边栏。按钮那一行的高度是 MACOS_CAPTION_ROW_HEIGHT，
 * 两者分开是因为调整按钮位置不该连带把侧边栏的 logo 往下推。
 */
export const MACOS_TITLEBAR_HEIGHT = 28
/**
 * macOS 顶部按钮行的高度。
 *
 * 36px = 24px 按钮 + 上下各 6px。取这个值是为了让按钮到窗口上边和右边的距离
 * 相等（都是 6px）——只留 2px 时按钮几乎顶在边框上，而右边却空着 12px，
 * 看起来是被挤到角上而不是贴角摆放。
 */
export const MACOS_CAPTION_ROW_HEIGHT = 36
export const MACOS_DRAG_REGION_HEIGHT = 32
export const MACOS_TRAFFIC_LIGHT_SAFE_WIDTH = 80
export const WINDOWS_TITLEBAR_HEIGHT = 32
export const WINDOWS_CAPTION_CONTROLS_WIDTH = 138
