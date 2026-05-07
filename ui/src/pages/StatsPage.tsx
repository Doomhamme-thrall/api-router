import { useState, useEffect, useCallback } from 'react'
import { BarChart3, RefreshCw, Filter, RotateCcw } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Badge } from '@/components/ui/badge'
import { useStats } from '@/hooks/use-api'
import type { StatsItem } from '@/types'

interface Props {
  token: string
}

const PRESETS = [
  { label: 'Last 30 min', value: 1800 },
  { label: 'Last 1 hour', value: 3600 },
  { label: 'Last 3 hours', value: 10800 },
  { label: 'Last 12 hours', value: 43200 },
  { label: 'Last 1 day', value: 86400 },
  { label: 'Last 7 days', value: 604800 },
  { label: 'Custom', value: -1 },
] as const

function toLocalDatetimeInputValue(date: Date) {
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`
}

function formatNumber(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`
  return String(n)
}

const TOKENS_PER_BAR = 1_000_000

function TokenBar({ total, maxTotal }: { total: number; maxTotal: number }) {
  const pct = maxTotal > 0 ? (total / maxTotal) * 100 : 0
  const fullBars = Math.floor(total / TOKENS_PER_BAR)
  const remainder = total % TOKENS_PER_BAR
  const remainderPct = maxTotal > 0 ? (remainder / maxTotal) * 100 : 0

  return (
    <div className="space-y-1">
      {Array.from({ length: fullBars }).map((_, i) => (
        <div key={i} className="h-2 bg-muted rounded-full overflow-hidden">
          <div className="h-full bg-primary rounded-full" style={{ width: '100%' }} />
        </div>
      ))}
      {(remainder > 0 || total === 0) && (
        <div className="h-2 bg-muted rounded-full overflow-hidden">
          <div className="h-full bg-primary rounded-full" style={{ width: `${remainderPct}%` }} />
        </div>
      )}
    </div>
  )
}

export default function StatsPage({ token }: Props) {
  const { stats, loading, load } = useStats(token)
  const [preset, setPreset] = useState('86400')
  const [fromDate, setFromDate] = useState('')
  const [toDate, setToDate] = useState('')
  const [isCustom, setIsCustom] = useState(false)

  useEffect(() => {
    // Load stats automatically on mount
    const toTs = Math.floor(Date.now() / 1000)
    const fromTs = toTs - 86400
    load(fromTs, toTs)
  }, [])

  const handleLoad = useCallback(() => {
    if (isCustom) {
      if (!fromDate || !toDate) return
      const fromTs = Math.floor(new Date(fromDate).getTime() / 1000)
      const toTs = Math.floor(new Date(toDate).getTime() / 1000)
      if (Number.isNaN(fromTs) || Number.isNaN(toTs)) return
      if (fromTs > toTs) return
      load(fromTs, toTs)
    } else {
      const seconds = Number(preset || 86400)
      const toTs = Math.floor(Date.now() / 1000)
      const fromTs = toTs - seconds
      load(fromTs, toTs)
    }
  }, [preset, isCustom, fromDate, toDate, load])

  const handlePresetChange = (value: string) => {
    setPreset(value)
    if (value === '-1') {
      setIsCustom(true)
      const end = new Date()
      const start = new Date(end.getTime() - 24 * 60 * 60 * 1000)
      setFromDate(toLocalDatetimeInputValue(start))
      setToDate(toLocalDatetimeInputValue(end))
    } else {
      setIsCustom(false)
    }
  }

  const handleReset = () => {
    setPreset('86400')
    setIsCustom(false)
    const toTs = Math.floor(Date.now() / 1000)
    const fromTs = toTs - 86400
    load(fromTs, toTs)
  }

  const sorted = [...stats].sort((a, b) => (b.total_tokens || 0) - (a.total_tokens || 0))
  const maxTotal = sorted.length > 0 ? sorted[0].total_tokens : 0

  const totalCalls = sorted.reduce((sum, s) => sum + s.total_calls, 0)
  const totalSuccess = sorted.reduce((sum, s) => sum + s.success_count, 0)
  const totalErrors = sorted.reduce((sum, s) => sum + s.error_count, 0)
  const totalTokens = sorted.reduce((sum, s) => sum + s.total_tokens, 0)

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <div>
          <h3 className="text-lg font-semibold">Usage Stats</h3>
          <p className="text-sm text-muted-foreground">
            {totalCalls} total calls · {formatNumber(totalTokens)} tokens
          </p>
        </div>
      </div>

      <div className="flex flex-wrap items-end gap-3">
        <div className="space-y-1.5 min-w-[160px]">
          <label className="text-xs text-muted-foreground font-medium">Time Range</label>
          <select
            value={preset}
            onChange={(e) => handlePresetChange(e.target.value)}
            className="flex h-9 w-full rounded-md border border-input bg-background px-3 py-1.5 text-sm ring-offset-background focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
          >
            {PRESETS.map((p) => (
              <option key={p.value} value={String(p.value)}>
                {p.label}
              </option>
            ))}
          </select>
        </div>

        {isCustom && (
          <>
            <div className="space-y-1.5">
              <label className="text-xs text-muted-foreground font-medium">From</label>
              <input
                type="datetime-local"
                value={fromDate}
                onChange={(e) => setFromDate(e.target.value)}
                className="flex h-9 rounded-md border border-input bg-background px-3 py-1.5 text-sm ring-offset-background focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
              />
            </div>
            <div className="space-y-1.5">
              <label className="text-xs text-muted-foreground font-medium">To</label>
              <input
                type="datetime-local"
                value={toDate}
                onChange={(e) => setToDate(e.target.value)}
                className="flex h-9 rounded-md border border-input bg-background px-3 py-1.5 text-sm ring-offset-background focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
              />
            </div>
          </>
        )}

        <Button size="sm" variant="default" onClick={handleLoad} disabled={loading}>
          {loading ? (
            <RefreshCw className="h-3.5 w-3.5 mr-1.5 animate-spin" />
          ) : (
            <Filter className="h-3.5 w-3.5 mr-1.5" />
          )}
          Load
        </Button>
        <Button size="sm" variant="outline" onClick={handleReset}>
          <RotateCcw className="h-3.5 w-3.5 mr-1.5" />
          Reset 24h
        </Button>
      </div>

      {sorted.length === 0 && !loading ? (
        <Card>
          <CardContent className="flex flex-col items-center justify-center py-12">
            <BarChart3 className="h-10 w-10 text-muted-foreground/40 mb-3" />
            <p className="text-sm text-muted-foreground">No data for this period.</p>
          </CardContent>
        </Card>
      ) : (
        <div className="space-y-3">
          {loading && sorted.length === 0 ? (
            <Card>
              <CardContent className="flex items-center justify-center py-12">
                <RefreshCw className="h-6 w-6 text-muted-foreground animate-spin" />
              </CardContent>
            </Card>
          ) : (
            sorted.map((item, idx) => {
              const successRate = item.total_calls > 0
                ? ((item.success_count / item.total_calls) * 100).toFixed(1)
                : '0.0'

              return (
                <Card key={item.target_name} className="hover:border-muted-foreground/20 transition-colors">
                  <CardContent className="p-4">
                    <div className="flex items-center gap-4 flex-wrap">
                      <span className="text-xs font-bold text-muted-foreground w-6 shrink-0">
                        #{idx + 1}
                      </span>
                      <span className="font-medium text-sm min-w-[120px]">{item.target_name}</span>
                      <span className="text-xs text-muted-foreground font-mono">
                        {item.total_calls} calls
                      </span>
                      <Badge variant="success" className="text-[10px]">
                        {item.success_count} OK
                      </Badge>
                      <Badge variant="destructive" className="text-[10px]">
                        {item.error_count} Err
                      </Badge>
                      <span className="text-xs text-muted-foreground font-mono">
                        {formatNumber(item.prompt_tokens)}p / {formatNumber(item.completion_tokens)}c
                      </span>
                      <span className="text-xs font-semibold text-primary font-mono">
                        {formatNumber(item.total_tokens)} tokens
                      </span>
                      <span className="text-xs text-muted-foreground font-mono">
                        {successRate}% success
                      </span>
                    </div>
                    <div className="mt-3">
                      <TokenBar total={item.total_tokens} maxTotal={maxTotal} />
                    </div>
                  </CardContent>
                </Card>
              )
            })
          )}
        </div>
      )}
    </div>
  )
}
