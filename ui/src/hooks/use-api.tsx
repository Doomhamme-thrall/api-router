import { createContext, useContext, useState, useCallback, useEffect } from 'react'
import type { ReactNode } from 'react'
import type { Target, ModelGroup, StatsItem, LoginResponse } from '@/types'

interface AuthContextType {
  token: string
  login: (username: string, password: string) => Promise<void>
  logout: () => void
}

const AuthContext = createContext<AuthContextType | null>(null)

export function AuthProvider({ children }: { children: ReactNode }) {
  const [token, setToken] = useState(() => localStorage.getItem('admin_token') || '')

  const login = useCallback(async (username: string, password: string) => {
    const res = await fetch('/admin/login', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ username, password }),
    })
    const data: LoginResponse = await res.json()
    if (!res.ok) throw new Error((data as any).error?.message || 'Login failed')
    localStorage.setItem('admin_token', data.token)
    setToken(data.token)
  }, [])

  const logout = useCallback(() => {
    localStorage.removeItem('admin_token')
    setToken('')
  }, [])

  return (
    <AuthContext.Provider value={{ token, login, logout }}>
      {children}
    </AuthContext.Provider>
  )
}

export function useAuth(): AuthContextType {
  const ctx = useContext(AuthContext)
  if (!ctx) throw new Error('useAuth must be inside AuthProvider')
  return ctx
}

function authHeaders(token: string): Record<string, string> {
  return {
    'Content-Type': 'application/json',
    Authorization: `Bearer ${token}`,
  }
}

async function handleResponse(res: Response): Promise<any> {
  const data = await res.json()
  if (!res.ok) {
    const msg = data?.error?.message || data?.message || `HTTP ${res.status}`
    throw new Error(msg)
  }
  return data
}

export function useTargets(token: string) {
  const [targets, setTargets] = useState<Target[]>([])
  const [loading, setLoading] = useState(false)

  const refresh = useCallback(async () => {
    if (!token) return
    setLoading(true)
    try {
      const data = await fetch('/admin/targets', { headers: authHeaders(token) }).then(handleResponse)
      setTargets(data.items || [])
    } finally {
      setLoading(false)
    }
  }, [token])

  const create = useCallback(async (payload: Partial<Target>): Promise<any> => {
    const data = await fetch('/admin/targets', {
      method: 'POST',
      headers: authHeaders(token),
      body: JSON.stringify(payload),
    }).then(handleResponse)
    await refresh()
    return data
  }, [token, refresh])

  const update = useCallback(async (id: string, payload: Partial<Target>): Promise<any> => {
    const data = await fetch(`/admin/targets/${id}`, {
      method: 'PUT',
      headers: authHeaders(token),
      body: JSON.stringify(payload),
    }).then(handleResponse)
    await refresh()
    return data
  }, [token, refresh])

  const remove = useCallback(async (id: string): Promise<any> => {
    const data = await fetch(`/admin/targets/${id}`, {
      method: 'DELETE',
      headers: authHeaders(token),
    }).then(handleResponse)
    await refresh()
    return data
  }, [token, refresh])

  const testTarget = useCallback(async (id: string): Promise<any> => {
    return fetch(`/admin/test-target/${encodeURIComponent(id)}`, {
      headers: authHeaders(token),
    }).then(handleResponse)
  }, [token])

  useEffect(() => { if (token) refresh() }, [token, refresh])

  return { targets, loading, refresh, create, update, remove, testTarget }
}

export function useModelGroups(token: string) {
  const [groups, setGroups] = useState<ModelGroup[]>([])
  const [loading, setLoading] = useState(false)

  const refresh = useCallback(async () => {
    if (!token) return
    setLoading(true)
    try {
      const data = await fetch('/admin/model-groups', { headers: authHeaders(token) }).then(handleResponse)
      setGroups(data.items || [])
    } finally {
      setLoading(false)
    }
  }, [token])

  const create = useCallback(async (payload: Partial<ModelGroup>): Promise<any> => {
    const data = await fetch('/admin/model-groups', {
      method: 'POST',
      headers: authHeaders(token),
      body: JSON.stringify(payload),
    }).then(handleResponse)
    await refresh()
    return data
  }, [token, refresh])

  const update = useCallback(async (id: string, payload: Partial<ModelGroup>): Promise<any> => {
    const data = await fetch(`/admin/model-groups/${id}`, {
      method: 'PUT',
      headers: authHeaders(token),
      body: JSON.stringify(payload),
    }).then(handleResponse)
    await refresh()
    return data
  }, [token, refresh])

  const remove = useCallback(async (id: string): Promise<any> => {
    const data = await fetch(`/admin/model-groups/${id}`, {
      method: 'DELETE',
      headers: authHeaders(token),
    }).then(handleResponse)
    await refresh()
    return data
  }, [token, refresh])

  useEffect(() => { if (token) refresh() }, [token, refresh])

  return { groups, loading, refresh, create, update, remove }
}

export function useStats(token: string) {
  const [stats, setStats] = useState<StatsItem[]>([])
  const [loading, setLoading] = useState(false)

  const load = useCallback(async (from?: number, to?: number) => {
    if (!token) return
    setLoading(true)
    try {
      const params = new URLSearchParams()
      if (from !== undefined) params.set('from', String(from))
      if (to !== undefined) params.set('to', String(to))
      const qs = params.toString()
      const data = await fetch(`/admin/stats${qs ? '?' + qs : ''}`, { headers: authHeaders(token) }).then(handleResponse)
      setStats(data.items || [])
    } finally {
      setLoading(false)
    }
  }, [token])

  return { stats, loading, load }
}
