import { useState, useCallback } from 'react'
import { toast } from 'sonner'
import { Plus, Save, Trash2, ChevronRight, Network } from 'lucide-react'
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
import { Checkbox } from '@/components/ui/checkbox'
import { Separator } from '@/components/ui/separator'
import type { ModelGroup, Target } from '@/types'

interface Props {
  token: string
  groups: ModelGroup[]
  targets: Target[]
  onRefresh: () => void
}

export default function GroupsPage({ token, groups, targets, onRefresh }: Props) {
  const [createOpen, setCreateOpen] = useState(false)
  const [expandedId, setExpandedId] = useState<string | null>(null)
  const [formName, setFormName] = useState('')
  const [formEnabled, setFormEnabled] = useState(true)
  const [formTargetIds, setFormTargetIds] = useState<string[]>([])

  // Edit state per expanded group (keyed by group id)
  const [editStates, setEditStates] = useState<Record<string, { name: string; enabled: boolean; target_ids: string[] }>>({})

  const initEditState = useCallback((group: ModelGroup) => {
    setEditStates((prev) => ({
      ...prev,
      [group.id]: { name: group.name, enabled: group.enabled, target_ids: [...(group.target_ids || [])] },
    }))
  }, [])

  const getEditState = (group: ModelGroup) =>
    editStates[group.id] ?? { name: group.name, enabled: group.enabled, target_ids: [...(group.target_ids || [])] }

  const resetForm = () => {
    setFormName('')
    setFormEnabled(true)
    setFormTargetIds([])
  }

  const handleCreate = async () => {
    if (!formName.trim()) {
      toast.error('Group name is required')
      return
    }
    try {
      const res = await fetch('/admin/model-groups', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${token}` },
        body: JSON.stringify({ name: formName.trim(), target_ids: formTargetIds, enabled: formEnabled }),
      })
      if (!res.ok) throw new Error((await res.json()).error?.message || 'Create failed')
      toast.success('Group created')
      setCreateOpen(false)
      resetForm()
      onRefresh()
    } catch (err: any) {
      toast.error(err.message)
    }
  }

  const handleUpdate = async (id: string, payload: Partial<ModelGroup>) => {
    try {
      const res = await fetch(`/admin/model-groups/${id}`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${token}` },
        body: JSON.stringify(payload),
      })
      if (!res.ok) throw new Error((await res.json()).error?.message || 'Update failed')
      toast.success('Group updated')
      onRefresh()
    } catch (err: any) {
      toast.error(err.message)
    }
  }

  const handleDelete = async (id: string, name: string) => {
    try {
      const res = await fetch(`/admin/model-groups/${id}`, {
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

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <div>
          <h3 className="text-lg font-semibold">Model Groups</h3>
          <p className="text-sm text-muted-foreground">
            {groups.length} group{groups.length !== 1 ? 's' : ''} configured
          </p>
        </div>
        <Dialog open={createOpen} onOpenChange={setCreateOpen}>
          <DialogTrigger asChild>
            <Button size="sm">
              <Plus className="h-4 w-4 mr-1.5" />
              Add Group
            </Button>
          </DialogTrigger>
          <DialogContent className="sm:max-w-md">
            <DialogHeader>
              <DialogTitle>New Model Group</DialogTitle>
              <DialogDescription>Group targets under a single model name</DialogDescription>
            </DialogHeader>
            <div className="space-y-4 py-4">
              <div className="grid grid-cols-2 gap-4">
                <div className="space-y-2">
                  <Label htmlFor="g_name">Group Name</Label>
                  <Input
                    id="g_name"
                    value={formName}
                    onChange={(e) => setFormName(e.target.value)}
                    placeholder="my-group"
                  />
                </div>
                <div className="space-y-2">
                  <Label htmlFor="g_enabled">Enabled</Label>
                  <select
                    id="g_enabled"
                    value={String(formEnabled)}
                    onChange={(e) => setFormEnabled(e.target.value === 'true')}
                    className="flex h-10 w-full rounded-md border border-input bg-background px-3 py-2 text-sm ring-offset-background focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
                  >
                    <option value="true">true</option>
                    <option value="false">false</option>
                  </select>
                </div>
              </div>
              <div className="space-y-2">
                <Label>Select Targets</Label>
                {targets.length === 0 ? (
                  <p className="text-sm text-muted-foreground">Create targets first.</p>
                ) : (
                  <div className="grid grid-cols-2 gap-2">
                    {targets.map((t) => (
                      <label
                        key={t.id}
                        className="flex items-center gap-2 p-2 rounded-md border hover:bg-accent/50 cursor-pointer transition-colors"
                      >
                        <Checkbox
                          checked={formTargetIds.includes(t.id)}
                          onCheckedChange={(checked) => {
                            setFormTargetIds((prev) =>
                              checked ? [...prev, t.id] : prev.filter((id) => id !== t.id)
                            )
                          }}
                        />
                        <span className="text-xs">{t.name}</span>
                      </label>
                    ))}
                  </div>
                )}
              </div>
            </div>
            <DialogFooter>
              <Button variant="outline" onClick={() => { setCreateOpen(false); resetForm() }}>Cancel</Button>
              <Button onClick={handleCreate}>Create</Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      </div>

      {groups.length === 0 ? (
        <Card>
          <CardContent className="flex flex-col items-center justify-center py-12">
            <Network className="h-10 w-10 text-muted-foreground/40 mb-3" />
            <p className="text-sm text-muted-foreground">No model groups yet. Click Add to create one.</p>
          </CardContent>
        </Card>
      ) : (
        <div className="space-y-3">
          {groups.map((group) => {
            const isExpanded = expandedId === group.id
            const groupTargets = (group.target_ids || [])
              .map((tid) => targets.find((t) => t.id === tid))
              .filter(Boolean) as Target[]

            return (
              <Card
                key={group.id}
                className={`transition-all duration-200 ${isExpanded ? 'ring-1 ring-ring' : 'hover:border-muted-foreground/20'}`}
              >
                <button
                  onClick={() => {
                    if (isExpanded) {
                      setExpandedId(null)
                    } else {
                      initEditState(group)
                      setExpandedId(group.id)
                    }
                  }}
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
                        <span className="font-medium text-sm truncate">{group.name}</span>
                        <Badge variant={group.enabled ? 'success' : 'warning'} className="shrink-0">
                          {group.enabled ? 'ON' : 'OFF'}
                        </Badge>
                      </div>
                      <p className="text-xs text-muted-foreground mt-0.5">
                        {groupTargets.length} target{groupTargets.length !== 1 ? 's' : ''}
                        {groupTargets.length > 0 &&
                          ` · ${groupTargets.map((t) => t.name).join(', ')}`}
                      </p>
                    </div>
                    <span className="text-[11px] text-muted-foreground font-mono">{group.id.slice(0, 8)}</span>
                  </CardHeader>
                </button>

                {isExpanded && (() => {
                  const editState = getEditState(group)
                  return (
                    <>
                      <Separator />
                      <CardContent className="p-4 space-y-4">
                        <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                          <div className="space-y-1.5">
                            <Label className="text-[11px]">Group Name</Label>
                            <Input
                              value={editState.name}
                              onChange={(e) =>
                                setEditStates((prev) => ({
                                  ...prev,
                                  [group.id]: { ...prev[group.id], name: e.target.value },
                                }))
                              }
                              className="h-8 text-xs"
                            />
                          </div>
                          <div className="space-y-1.5">
                            <Label className="text-[11px]">Enabled</Label>
                            <select
                              value={String(editState.enabled)}
                              onChange={(e) =>
                                setEditStates((prev) => ({
                                  ...prev,
                                  [group.id]: { ...prev[group.id], enabled: e.target.value === 'true' },
                                }))
                              }
                              className="flex h-8 w-full rounded-md border border-input bg-background px-2 py-1 text-xs ring-offset-background focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
                            >
                              <option value="true">true</option>
                              <option value="false">false</option>
                            </select>
                          </div>
                        </div>

                        <div className="space-y-2">
                          <Label className="text-[11px]">Members</Label>
                          {targets.length === 0 ? (
                            <p className="text-xs text-muted-foreground">No targets available.</p>
                          ) : (
                            <div className="grid grid-cols-2 sm:grid-cols-3 gap-2">
                              {targets.map((t) => {
                                const checked = editState.target_ids.includes(t.id)
                                return (
                                  <label
                                    key={t.id}
                                    className="flex items-center gap-2 p-2 rounded-md border hover:bg-accent/50 cursor-pointer transition-colors"
                                  >
                                    <Checkbox
                                      checked={checked}
                                      onCheckedChange={(chk) => {
                                        setEditStates((prev) => {
                                          const curr = prev[group.id]
                                          const tid = curr.target_ids.includes(t.id)
                                            ? curr.target_ids.filter((id) => id !== t.id)
                                            : [...curr.target_ids, t.id]
                                          return { ...prev, [group.id]: { ...curr, target_ids: tid } }
                                        })
                                      }}
                                      className="data-[state=checked]:bg-primary"
                                    />
                                    <span className="text-xs">{t.name}</span>
                                  </label>
                                )
                              })}
                            </div>
                          )}
                        </div>

                        <div className="flex items-center gap-2 pt-2">
                          <Button
                            size="sm"
                            variant="default"
                            onClick={() => {
                              const payload: Partial<ModelGroup> = {
                                name: editState.name,
                                enabled: editState.enabled,
                                target_ids: editState.target_ids,
                              }
                              handleUpdate(group.id, payload)
                            }}
                          >
                            <Save className="h-3.5 w-3.5 mr-1" />
                            Save
                          </Button>
                          <Button
                            size="sm"
                            variant="destructive"
                            onClick={() => handleDelete(group.id, group.name)}
                          >
                            <Trash2 className="h-3.5 w-3.5 mr-1" />
                            Delete
                          </Button>
                        </div>
                      </CardContent>
                    </>
                  )
                })()}
              </Card>
            )
          })}
        </div>
      )}
    </div>
  )
}
