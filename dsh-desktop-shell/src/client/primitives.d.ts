declare module '@deepseek-ai/dsh-client-ui-primitives' {
  import type { ReactElement, ReactNode } from 'react'

  export interface IconProps {
    size?: number
    className?: string
  }

  /**
   * 帮助气泡。`label` 传函数时只在气泡可见期间求值。
   * `children` 必须是单个元素——组件要克隆它以挂 ref。
   */
  export function Tooltip(props: {
    label: string | (() => string)
    side?: 'right' | 'bottom' | 'top'
    delayMs?: number
    disabled?: boolean
    maxWidth?: number
    children: ReactElement
  }): ReactElement

  export function IconQuestionOutline14(props: IconProps): ReactElement
  export function IconWarningOutline16(props: IconProps): ReactElement

  export type StateDotState = 'done' | 'warning' | 'ongoing' | 'error'
  export function StateDot(props: {
    state: StateDotState
    size?: number | undefined
    className?: string | undefined
  }): ReactElement
}
