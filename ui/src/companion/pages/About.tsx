import { useEffect, useState } from 'react'
import { appVersion } from '../bridge'
import { Button, Group, Panel, Row } from '../room/parts'
import { banner } from '../room/room'
import { GLOW, useScreenAt } from '../room/screen'
import { SYSTEM } from '../system'
import { lookForUpdate } from '../update'
import { appLogPlace, type HostStatus } from '../useHostStatus'
import { type PageProps, screenState } from './page'

/** How long the screen says the report was copied. */
const COPIED_MS = 3200

/** Where the plugin writes its log: WestPlay's folder in Documents, on the Mac in Application Support. */
const PLUGIN_LOG =
  SYSTEM === 'mac' ? '~/Library/Application Support/WestPlay/logs/plugin.log' : 'Documents\\WestPlay\\logs\\plugin.log'

/**
 * WestPlay itself: its version and whether there's an update (looked for as the page opens; one found opens
 * its sheet), and the live details of WestPlay and the game for when something isn't
 * right, with a report of them to copy for whoever helps, and where the logs are (app.log in WestPlay's own
 * folder where Documents refused it; on the Mac, both in Application Support). On the room's screen, the Dashboard; a banner says when the report's
 * copied.
 */
export function About({ status, setup, cw, update, showUpdate }: PageProps) {
  const w = cw.about
  const [version, setVersion] = useState('')
  const [copied, setCopied] = useState(false)
  // Whether there's an update: the one found, or none after looking again.
  const [looked, setLooked] = useState(false)
  // Where this run's app.log is; null if it couldn't be made anywhere.
  const [appLog, setAppLog] = useState<string | null>(null)
  useEffect(() => {
    appVersion().then(setVersion, () => setVersion(''))
    appLogPlace().then(setAppLog, () => setAppLog(null))
    lookForUpdate().then(
      () => setLooked(true),
      () => setLooked(true),
    )
  }, [])
  useEffect(() => {
    if (!copied) return
    const timer = window.setTimeout(() => setCopied(false), COPIED_MS)
    return () => window.clearTimeout(timer)
  }, [copied])
  useScreenAt('dashboard', GLOW.dashboard, copied ? banner(w.copied) : null)

  const copy = () => {
    const text = report(version, status, setup.game?.version ?? null, setup.game?.dir ?? null, appLog)
    navigator.clipboard.writeText(text).then(
      () => setCopied(true),
      (error: unknown) => console.error('The report could not be copied', error),
    )
  }

  return (
    <Panel
      place="page"
      title={w.title}
      sub={w.sub}
      action={
        <Button glass onClick={copy}>
          {w.copy}
        </Button>
      }
    >
      <Group place={4}>
        <Row title={w.version} value={version} />
        {update ? (
          <Row
            title={cw.update.row}
            value={cw.update.available.replace('{version}', update.version)}
            link
            onClick={showUpdate}
          />
        ) : (
          <Row title={cw.update.row} value={looked ? cw.update.upToDate : undefined} />
        )}
      </Group>
      <Group title={w.diagnostics} place={5}>
        <Row
          title={w.frames}
          value={status?.screenDrawing ? w.perSecond.replace('{n}', String(status.framesPerSecond)) : '—'}
        />
        <Row title={w.dropped} value={status ? String(status.droppedTotal) : '—'} />
        <Row title={w.cabGpu} detail={status?.cabAdapter ?? '—'} />
        <Row title={w.gameGpu} detail={status?.gameAdapter ?? w.waitingForGame} />
        <Row title={w.webview} value={webview(status?.webviewVersion)} />
        <Row title={w.log} detail={PLUGIN_LOG} />
        <Row title={w.appLog} detail={appLog ?? '—'} />
      </Group>
    </Panel>
  )
}

/** The WebView's version: WebView2's number; on the Mac, "WebKit (Safari 26.0)" under WebKit's title is Safari's. */
function webview(version: string | undefined): string {
  if (!version) return '—'
  return version.replace(/^WebKit \((.*)\)$/, '$1')
}

/** The report to copy: plain facts, in English (for whoever helps), nothing of the player's accounts. */
function report(
  version: string,
  status: HostStatus | null,
  game: string | null,
  dir: string | null,
  appLog: string | null,
): string {
  return [
    `WestPlay ${version}`,
    `Game: ${game ?? 'unknown'}${dir ? ` (${dir})` : ''}`,
    `State: ${screenState(status)}`,
    `Engine: ${status?.error ? `stopped: ${status.error}` : status ? 'running' : 'starting'}`,
    `Game connected: ${status?.gameConnected ? 'yes' : 'no'}`,
    `Screen drawing: ${status?.screenDrawing ? `yes, ${status.framesPerSecond} fps` : 'no'}`,
    `Dropped frames: ${status?.droppedTotal ?? '—'}`,
    `GPU: ${status?.cabAdapter ?? '—'} (game: ${status?.gameAdapter ?? '—'}; all: ${
      status?.adapters.map((adapter) => adapter.name).join(', ') || '—'
    })`,
    `${SYSTEM === 'mac' ? 'WebView' : 'WebView2'}: ${status?.webviewVersion || '—'}`,
    `Map: ${status?.map.state ?? 'unknown'}`,
    `App log: ${appLog ?? 'not written'}`,
    `${SYSTEM === 'mac' ? 'macOS' : 'Windows'}: ${navigator.userAgent}`,
  ].join('\n')
}
