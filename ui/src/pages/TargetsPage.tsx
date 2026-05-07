import { useState, useEffect } from 'react'
import { toast } from 'sonner'
import {
  Plus,
  Save,
  Trash2,
  Play,
  ChevronRight,
  Globe,
  Key,
  Cpu,
  Zap,
  CheckCircle2,
  XCircle,
  Terminal,
  RefreshCw,
} from 'lucide-react'
import { Button } from '@/components/ui/button'
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Badge } from '@/components/ui/badge'
import { Separator } from '@/components/ui/separator'
import type { Target } from '@/types'

interface Props {
  token: string
  targets: Target[]
  onRefresh: () => void
}

export default function TargetsPage({ token, targets, onRefresh }: Props) {
  const [createOpen, setCreateOpen] = useState(false)
  const [expandedId, setExpandedId] = useState<string | null>(null)
  const [testResults, setTestResults] = useState<Record<string, { ok: boolean; text: string }>>({})
  const [testing, setTesting] = useState<Record<string, boolean>>({})

  // Create form state
  const [form, setForm] = useState({
    name: '',
    provider: '',
    api_format: 'openai',
    base_url: '',
    api_key: '',
    router_model: '',
    upstream_model: '',
  })

  const resetForm = () => {
    setForm({ name: '', provider: '', api_format: 'openai', base_url: '', api_key: '', router_model: '', upstream_model: '' })
  }

  const handleCreate = async () => {
    try {
      const res = await fetch('/admin/targets', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${token}` },
        body: JSON.stringify({ ...form, enabled: true }),
      })
      if (!res.ok) throw new Error((await res.json()).error?.message || 'Create failed')
      toast.success('Target created')
      setCreateOpen(false)
      resetForm()
      onRefresh()
    } catch (err: any) {
      toast.error(err.message)
    }
  }

  const handleUpdate = async (id: string, payload: Partial<Target>) => {
    try {
      const res = await fetch(`/admin/targets/${id}`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${token}` },
        body: JSON.stringify(payload),
      })
      if (!res.ok) throw new Error((await res.json()).error?.message || 'Update failed')
      toast.success('Target updated')
      onRefresh()
    } catch (err: any) {
      toast.error(err.message)
    }
  }

  const handleDelete = async (id: string, name: string) => {
    try {
      const res = await fetch(`/admin/targets/${id}`, {
        method: 'DELETE',
        headers: { Authorization: `Bearer ${token}` },
      })
      if (!res.ok) throw new Error((await res.json()).error?.message || 'Delete failed')
      toast.success(`Deleted ${name}`)
      onRefresh()
    } catch (err: any) {
      toast.error(err.message)
    }
  }

  const handleTest = async (id: string) => {
    setTesting((prev) => ({ ...prev, [id]: true }))
    setTestResults((prev) => ({ ...prev, [id]: { ok: false, text: 'Testing...' } }))
    try {
      const res = await fetch(`/admin/test-target/${encodeURIComponent(id)}`, {
        headers: { Authorization: `Bearer ${token}` },
      })
      const data = await res.json()
      if (data.ok) {
        const content = data.response?.choices?.[0]?.message?.content || JSON.stringify(data.response)
        setTestResults((prev) => ({ ...prev, [id]: { ok: true, text: content } }))
        toast.success('Test passed')
      } else {
        const errMsg = typeof data.error === 'string'
          ? data.error
          : data.error?.error?.message || JSON.stringify(data.error)
        setTestResults((prev) => ({ ...prev, [id]: { ok: false, text: errMsg } }))
        toast.error('Test failed')
      }
    } catch (err: any) {
      setTestResults((prev) => ({ ...prev, [id]: { ok: false, text: `Network error: ${err.message}` } }))
      toast.error('Test network error')
    } finally {
      setTesting((prev) => ({ ...prev, [id]: false }))
    }
  }

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <div>
          <h3 className="text-lg font-semibold">Targets</h3>
          <p className="text-sm text-muted-foreground">
            {targets.length} upstream{targets.length !== 1 ? 's' : ''} configured
          </p>
        </div>
        <Dialog open={createOpen} onOpenChange={setCreateOpen}>
          <DialogTrigger asChild>
            <Button size="sm">
              <Plus className="h-4 w-4 mr-1.5" />
              Add Target
            </Button>
          </DialogTrigger>
          <DialogContent className="sm:max-w-lg">
            <DialogHeader>
              <DialogTitle>New Target</DialogTitle>
              <DialogDescription>Add a new upstream LLM target</DialogDescription>
            </DialogHeader>
            <div className="grid grid-cols-2 gap-4 py-4">
              <div className="space-y-2">
                <Label htmlFor="f_name">Name</Label>
                <Input id="f_name" value={form.name} onChange={(e) => setForm((f) => ({ ...f, name: e.target.value }))} placeholder="OpenAI Primary" />
              </div>
              <div className="space-y-2">
                <Label htmlFor="f_provider">Provider</Label>
                <Input id="f_provider" value={form.provider} onChange={(e) => setForm((f) => ({ ...f, provider: e.target.value }))} placeholder="openai" />
              </div>
              <div className="space-y-2">
                <Label htmlFor="f_api_format">API Format</Label>
                <select
                  id="f_api_format"
                  value={form.api_format}
                  onChange={(e) => setForm((f) => ({ ...f, api_format: e.target.value }))}
                  className="flex h-10 w-full rounded-md border border-input bg-background px-3 py-2 text-sm ring-offset-background focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
                >
                  <option value="openai">openai</option>
                  <option value="gemini">gemini</option>
                </select>
              </div>
              <div className="space-y-2">
                <Label htmlFor="f_base_url">Base URL</Label>
                <Input id="f_base_url" value={form.base_url} onChange={(e) => setForm((f) => ({ ...f, base_url: e.target.value }))} placeholder="https://api.openai.com" />
              </div>
              <div className="space-y-2 col-span-2">
                <Label htmlFor="f_api_key">API Key</Label>
                <Input id="f_api_key" value={form.api_key} onChange={(e) => setForm((f) => ({ ...f, api_key: e.target.value }))} placeholder="sk-..." />
              </div>
              <div className="space-y-2">
                <Label htmlFor="f_router_model">Router Model</Label>
                <Input id="f_router_model" value={form.router_model} onChange={(e) => setForm((f) => ({ ...f, router_model: e.target.value }))} placeholder="router-default" />
              </div>
              <div className="space-y-2">
                <Label htmlFor="f_upstream_model">Upstream Model</Label>
                <Input id="f_upstream_model" value={form.upstream_model} onChange={(e) => setForm((f) => ({ ...f, upstream_model: e.target.value }))} placeholder="gpt-4o-mini" />
              </div>
            </div>
            <DialogFooter>
              <Button variant="outline" onClick={() => { setCreateOpen(false); resetForm() }}>Cancel</Button>
              <Button onClick={handleCreate}>Create</Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      </div>

      {targets.length === 0 ? (
        <Card>
          <CardContent className="flex flex-col items-center justify-center py-12">
            <Cpu className="h-10 w-10 text-muted-foreground/40 mb-3" />
            <p className="text-sm text-muted-foreground">No targets yet. Click Add to create one.</p>
          </CardContent>
        </Card>
      ) : (
        <div className="space-y-3">
          {targets.map((target) => {
            const isExpanded = expandedId === target.id
            return (
              <Card
                key={target.id}
                className={`transition-all duration-200 ${isExpanded ? 'ring-1 ring-ring' : 'hover:border-muted-foreground/20'}`}
              >
                <button
                  onClick={() => setExpandedId(isExpanded ? null : target.id)}
                  className="w-full text-left"
                >
                  <CardHeader className="flex flex-row items-center gap-3 py-3 px-4">
                    <ChevronRight
                      className={`h-4 w-4 text-muted-foreground transition-transform duration-200 ${
                        isExpanded ? 'rotate-90' : ''
                      }`}
                    />
                    <div className="flex-1 min-w-0">
                      <div className="flex items-center gap-2">
                        <span className="font-medium text-sm truncate">{target.name}</span>
                        <Badge variant={target.enabled ? 'success' : 'warning'} className="shrink-0">
                          {target.enabled ? 'ON' : 'OFF'}
                        </Badge>
                      </div>
                      <p className="text-xs text-muted-foreground mt-0.5 truncate">
                        {target.provider} · {target.router_model}
                      </p>
                    </div>
                    <span className="text-[11px] text-muted-foreground font-mono">{target.id.slice(0, 8)}</span>
                  </CardHeader>
                </button>

                {isExpanded && (
                  <>
                    <Separator />
                    <CardContent className="p-4 space-y-4">
                      <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
                        <div className="space-y-1.5">
                          <Label className="text-[11px]">Name</Label>
                          <Input
                            defaultValue={target.name}
                            id={`t_name_${target.id}`}
                            className="h-8 text-xs"
                          />
                        </div>
                        <div className="space-y-1.5">
                          <Label className="text-[11px]">Provider</Label>
                          <Input
                            defaultValue={target.provider}
                            id={`t_provider_${target.id}`}
                            className="h-8 text-xs"
                          />
                        </div>
                        <div className="space-y-1.5">
                          <Label className="text-[11px]">API Format</Label>
                          <select
                            defaultValue={target.api_format || 'openai'}
                            id={`t_api_format_${target.id}`}
                            className="flex h-8 w-full rounded-md border border-input bg-background px-2 py-1 text-xs ring-offset-background focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
                          >
                            <option value="openai">openai</option>
                            <option value="gemini">gemini</option>
                          </select>
                        </div>
                        <div className="space-y-1.5 sm:col-span-2">
                          <Label className="text-[11px]">Base URL</Label>
                          <Input
                            defaultValue={target.base_url}
                            id={`t_base_url_${target.id}`}
                            className="h-8 text-xs font-mono"
                          />
                        </div>
                        <div className="space-y-1.5 sm:col-span-2">
                          <Label className="text-[11px]">API Key</Label>
                          <Input
                            defaultValue={target.api_key}
                            id={`t_api_key_${target.id}`}
                            className="h-8 text-xs font-mono"
                            type="password"
                          />
                        </div>
                        <div className="space-y-1.5">
                          <Label className="text-[11px]">Router Model</Label>
                          <Input
                            defaultValue={target.router_model}
                            id={`t_router_model_${target.id}`}
                            className="h-8 text-xs font-mono"
                          />
                        </div>
                        <div className="space-y-1.5">
                          <Label className="text-[11px]">Upstream Model</Label>
                          <Input
                            defaultValue={target.upstream_model}
                            id={`t_upstream_model_${target.id}`}
                            className="h-8 text-xs font-mono"
                          />
                        </div>
                        <div className="space-y-1.5">
                          <Label className="text-[11px]">Enabled</Label>
                          <select
                            defaultValue={String(target.enabled)}
                            id={`t_enabled_${target.id}`}
                            className="flex h-8 w-full rounded-md border border-input bg-background px-2 py-1 text-xs ring-offset-background focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
                          >
                            <option value="true">true</option>
                            <option value="false">false</option>
                          </select>
                        </div>
                      </div>

                      {/* Test result */}
                      {testResults[target.id] && (
                        <div
                          className={`p-3 rounded-md text-xs font-mono whitespace-pre-wrap break-all ${
                            testResults[target.id].ok
                              ? 'bg-emerald-500/10 text-emerald-500'
                              : 'bg-red-500/10 text-red-500'
                          }`}
                        >
                          {testResults[target.id].ok ? '✓ ' : '✗ '}
                          {testResults[target.id].text}
                        </div>
                      )}

                      <div className="flex items-center gap-2 pt-2">
                        <Button
                          size="sm"
                          variant="default"
                          onClick={() => {
                            const payload: Partial<Target> = {
                              name: (document.getElementById(`t_name_${target.id}`) as HTMLInputElement)?.value,
                              provider: (document.getElementById(`t_provider_${target.id}`) as HTMLInputElement)?.value,
                              api_format: (document.getElementById(`t_api_format_${target.id}`) as HTMLSelectElement)?.value,
                              base_url: (document.getElementById(`t_base_url_${target.id}`) as HTMLInputElement)?.value,
                              api_key: (document.getElementById(`t_api_key_${target.id}`) as HTMLInputElement)?.value,
                              router_model: (document.getElementById(`t_router_model_${target.id}`) as HTMLInputElement)?.value,
                              upstream_model: (document.getElementById(`t_upstream_model_${target.id}`) as HTMLInputElement)?.value,
                              enabled: (document.getElementById(`t_enabled_${target.id}`) as HTMLSelectElement)?.value === 'true',
                            }
                            handleUpdate(target.id, payload)
                          }}
                        >
                          <Save className="h-3.5 w-3.5 mr-1" />
                          Save
                        </Button>
                        <Button
                          size="sm"
                          variant="secondary"
                          onClick={() => handleTest(target.id)}
                          disabled={testing[target.id]}
                        >
                          {testing[target.id] ? (
                            <RefreshCw className="h-3.5 w-3.5 mr-1 animate-spin" />
                          ) : (
                            <Play className="h-3.5 w-3.5 mr-1" />
                          )}
                          Test
                        </Button>
                        <Button
                          size="sm"
                          variant="destructive"
                          onClick={() => handleDelete(target.id, target.name)}
                        >
                          <Trash2 className="h-3.5 w-3.5 mr-1" />
                          Delete
                        </Button>
                      </div>
                    </CardContent>
                  </>
                )}
              </Card>
            )
          })}
        </div>
      )}
    </div>
  )
}
