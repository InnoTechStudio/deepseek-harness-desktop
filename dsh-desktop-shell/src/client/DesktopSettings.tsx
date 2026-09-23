import { useCallback, useEffect, useState } from 'react'
import {
  IconQuestionOutline14,
  IconWarningOutline16,
  StateDot,
  Tooltip,
} from '@deepseek-ai/dsh-client-ui-primitives'
import { openHostDialog, requestDesktop } from './desktop-bridge.ts'
import css from './DesktopSettings.module.css'

/** 本分区用到的 primitives 导出，注册前逐个确认存在。 */
export const REQUIRED_PRIMITIVES = [
  'Tooltip',
  'IconQuestionOutline14',
  'IconWarningOutline16',
  'StateDot',
] as const

/** 宿主 `get_settings` 的返回结构。 */
interface DesktopSettingsValues {
  autoCheckUpdates: boolean
  autoLaunch: boolean
  preventSleep: boolean
  desktopNotifications: boolean
  confirmExit: boolean
}

/** 宿主 `get_state` 的返回结构中本分区用到的字段。 */
interface DesktopRuntimeState {
  installedVersion: string | null
  running: boolean
  port: number | null
}

interface ClientUpdateInfo {
  current_version: string
  latest_version: string
  update_available: boolean
}

type SettingKey = keyof DesktopSettingsValues

interface SwitchSpec {
  key: SettingKey
  label: string
  hint: string
}

/**
 * 五个开关，按用户关心的优先级排序：
 * 更新最常改，其次是启动方式，再是运行期行为，最后是退出行为。
 */
const SWITCHES: readonly SwitchSpec[] = [
  {
    key: 'autoCheckUpdates',
    label: '自动检查更新',
    hint: '每隔 6 小时检查客户端和内核更新，发现新版本时提醒你。',
  },
  {
    key: 'autoLaunch',
    label: '开机自启',
    hint: '系统启动时自动运行 DeepSeek Harness，适合频繁使用的场景。',
  },
  {
    key: 'desktopNotifications',
    label: '桌面通知',
    hint: '任务完成、任务失败和后台任务状态会以系统通知提醒；不会显示会话正文。',
  },
  {
    key: 'preventSleep',
    label: '防止系统休眠',
    hint: '运行期间阻止系统休眠，长时间任务不会被打断；屏幕仍可正常关闭。',
  },
  {
    key: 'confirmExit',
    label: '关闭窗口时退出程序',
    hint: '开启后点关闭按钮会确认并完全退出；关闭时点关闭仅隐藏窗口，服务继续在后台运行。',
  },
]

function HelpMarker({ text }: { text: string }) {
  return (
    <Tooltip label={text} side="top" delayMs={150} maxWidth={280}>
      <span className={css.help} tabIndex={0} role="img" aria-label={text}>
        <IconQuestionOutline14 size={14} />
      </span>
    </Tooltip>
  )
}

function Switch(props: {
  checked: boolean
  disabled: boolean
  label: string
  onToggle: () => void
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={props.checked}
      aria-label={props.label}
      disabled={props.disabled}
      className={props.checked ? `${css.switch} ${css.switchOn}` : css.switch}
      onClick={props.onToggle}
    >
      <span className={css.switchKnob} />
    </button>
  )
}

/**
 * 客户端设置。
 *
 * 开关的真实状态由宿主持有——开机自启和防休眠对应真实的系统调用，
 * 所以每次写入后都重新读取一遍，避免界面显示的状态与系统实际状态不一致。
 */
export function DesktopSettings(): JSX.Element {
  const [values, setValues] = useState<DesktopSettingsValues | null>(null)
  const [state, setState] = useState<DesktopRuntimeState | null>(null)
  const [error, setError] = useState<string | null>(null)
  // 通知被系统拒绝时，光给一句话没用——得让用户能一键跳到那一页。
  const [showNotificationFix, setShowNotificationFix] = useState(false)
  const [busy, setBusy] = useState<SettingKey | null>(null)
  const [clientVersion, setClientVersion] = useState<string | null>(null)

  const load = useCallback(async () => {
    try {
      const [settings, runtime] = await Promise.all([
        requestDesktop<DesktopSettingsValues>('get-settings'),
        requestDesktop<DesktopRuntimeState>('get-state'),
      ])
      setValues(settings)
      setState(runtime)
      setError(null)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    }
  }, [])

  useEffect(() => { void load() }, [load])

  // 客户端版本没有单独的查询命令，用更新检查的返回值顺带拿到。
  useEffect(() => {
    void requestDesktop<ClientUpdateInfo>('check-client-update')
      .then((info) => { setClientVersion(info.current_version) })
      .catch(() => { /* 离线时留空即可，不打扰用户 */ })
  }, [])

  const toggle = useCallback(async (key: SettingKey) => {
    if (values === null || busy !== null) return
    const next = !values[key]
    setBusy(key)
    setError(null)
    setShowNotificationFix(false)
    // 先乐观更新，写入失败时由重新读取纠正回来。
    setValues({ ...values, [key]: next })
    try {
      await requestDesktop('set-settings', { [key]: next })
      // 宿主会把开机自启/防休眠回报为系统真实状态。
      const fresh = await requestDesktop<DesktopSettingsValues>('get-settings')
      setValues(fresh)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
      void load()
      setBusy(null)
      return
    }
    // 开关本身已经存下了，测试通知只是验证系统是否真的会显示。
    // 它失败不该回滚开关，只把系统给出的原因报给用户。
    if (key === 'desktopNotifications' && next) {
      try {
        const shown = await requestDesktop<boolean>('send-test-notification')
        if (!shown) {
          setError('设置已保存，但系统没有显示测试通知。')
          setShowNotificationFix(true)
        }
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : String(cause))
        setShowNotificationFix(true)
      }
    }
    setBusy(null)
  }, [busy, load, values])

  return (
    <div className={css.root}>
      <h2 className={css.title}>客户端设置</h2>
      <p className={css.intro}>DeepSeek Harness 桌面客户端的偏好与运行状态。</p>

      {error !== null && (
        <div className={css.error} role="alert">
          <IconWarningOutline16 size={16} />
          <span>{error}</span>
          {showNotificationFix && (
            <button
              type="button"
              className={css.errorAction}
              onClick={() => { void requestDesktop('open-notification-settings').catch(() => {}) }}
            >
              打开系统通知设置
            </button>
          )}
        </div>
      )}

      {values === null
        ? <p className={css.loading}>加载中…</p>
        : (
          <div className={css.group}>
            {SWITCHES.map((spec) => (
              <div className={css.row} key={spec.key}>
                <div className={css.rowText}>
                  <div className={css.label}>
                    {spec.label}
                    <HelpMarker text={spec.hint} />
                  </div>
                </div>
                <Switch
                  checked={values[spec.key]}
                  disabled={busy !== null}
                  label={spec.label}
                  onToggle={() => { void toggle(spec.key) }}
                />
              </div>
            ))}
          </div>
        )}

      {state !== null && (
        <div className={css.status}>
          <div className={css.statusItem}>
            <span className={css.statusLabel}>客户端版本</span>
            <span className={css.statusValue}>
              {clientVersion === null ? '—' : `v${clientVersion}`}
            </span>
          </div>
          <div className={css.statusItem}>
            <span className={css.statusLabel}>内核版本</span>
            <span className={css.statusValue}>
              {state.installedVersion === null ? '未安装' : `v${state.installedVersion}`}
            </span>
          </div>
          <div className={css.statusItem}>
            <span className={css.statusLabel}>服务状态</span>
            <span className={css.statusValue}>
              <StateDot state={state.running ? 'done' : 'error'} size={8} />
              {state.running
                ? (state.port === null ? '运行中' : `运行中 · 端口 ${state.port}`)
                : '未运行'}
            </span>
          </div>
        </div>
      )}

      <div className={css.actions}>
        <button type="button" className={css.linkButton} onClick={() => { openHostDialog('about') }}>
          关于
        </button>
        <button type="button" className={css.linkButton} onClick={() => { openHostDialog('check-updates') }}>
          检查更新
        </button>
        <button type="button" className={css.linkButton} onClick={() => { openHostDialog('feedback') }}>
          反馈问题
        </button>
      </div>

      <p className={css.about}>
        DeepSeek Harness 桌面客户端 · Copyright © 2026 INNOTECH
      </p>
    </div>
  )
}
