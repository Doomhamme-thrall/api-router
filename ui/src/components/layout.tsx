import { useState } from 'react'
import { Routes, Route, useNavigate, useLocation, Navigate } from 'react-router-dom'
import {
  Box,
  LayoutDashboard,
  Network,
  BarChart3,
  LogOut,
  RefreshCw,
  Plus,
  ChevronDown,
} from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Separator } from '@/components/ui/separator'
import { useAuth, useTargets, useModelGroups } from '@/hooks/use-api'
import TargetsPage from '@/pages/TargetsPage'
import GroupsPage from '@/pages/GroupsPage'
import StatsPage from '@/pages/StatsPage'

const navItems = [
  { path: '/targets', label: 'Targets', icon: Box },
  { path: '/groups', label: 'Model Groups', icon: Network },
  { path: '/stats', label: 'Usage Stats', icon: BarChart3 },
] as const

export default function Dashboard() {
  const { token, logout } = useAuth()
  const navigate = useNavigate()
  const location = useLocation()
  const { targets, refresh: refreshTargets } = useTargets(token)
  const { groups, refresh: refreshGroups } = useModelGroups(token)
  const [mobileMenuOpen, setMobileMenuOpen] = useState(false)

  const currentPath = location.pathname === '/' ? '/targets' : location.pathname
  const currentTab = navItems.find((item) => item.path === currentPath)
  const pageTitle = currentTab?.label || 'Dashboard'

  const handleRefresh = () => {
    if (currentPath.includes('targets')) refreshTargets()
    else if (currentPath.includes('groups')) refreshGroups()
  }

  const handleLogout = () => {
    logout()
    navigate('/')
  }

  return (
    <div className="min-h-screen bg-background">
      {/* Mobile header */}
      <div className="lg:hidden fixed top-0 left-0 right-0 z-30 flex items-center justify-between border-b bg-background/95 backdrop-blur px-4 h-14">
        <button
          onClick={() => setMobileMenuOpen(!mobileMenuOpen)}
          className="p-2 -ml-2 hover:bg-accent rounded-md"
        >
          <ChevronDown className={`h-5 w-5 transition-transform ${mobileMenuOpen ? 'rotate-180' : ''}`} />
        </button>
        <span className="font-semibold text-sm">{pageTitle}</span>
        <div className="flex gap-1">
          <Button variant="ghost" size="icon" onClick={handleRefresh}>
            <RefreshCw className="h-4 w-4" />
          </Button>
          <Button variant="ghost" size="icon" onClick={handleLogout}>
            <LogOut className="h-4 w-4" />
          </Button>
        </div>
      </div>

      {/* Mobile menu */}
      {mobileMenuOpen && (
        <div className="lg:hidden fixed inset-0 z-20 bg-background/80 backdrop-blur-sm" onClick={() => setMobileMenuOpen(false)}>
          <div className="fixed top-14 left-0 right-0 bg-background border-b shadow-lg p-4" onClick={(e) => e.stopPropagation()}>
            <nav className="flex flex-col gap-1">
              {navItems.map((item) => (
                <button
                  key={item.path}
                  onClick={() => { navigate(item.path); setMobileMenuOpen(false) }}
                  className={`flex items-center gap-3 px-3 py-2.5 rounded-md text-sm font-medium transition-colors ${
                    currentPath === item.path
                      ? 'bg-accent text-accent-foreground'
                      : 'text-muted-foreground hover:bg-accent/50'
                  }`}
                >
                  <item.icon className="h-4 w-4" />
                  {item.label}
                </button>
              ))}
              <Separator className="my-2" />
              <button
                onClick={handleLogout}
                className="flex items-center gap-3 px-3 py-2.5 rounded-md text-sm font-medium text-muted-foreground hover:bg-accent/50"
              >
                <LogOut className="h-4 w-4" />
                Sign Out
              </button>
            </nav>
          </div>
        </div>
      )}

      {/* Sidebar */}
      <aside className="hidden lg:flex fixed inset-y-0 left-0 z-30 w-56 flex-col border-r bg-card">
        <div className="flex items-center gap-2 px-5 py-4 border-b">
          <LayoutDashboard className="h-5 w-5 text-primary" />
          <div>
            <h1 className="text-sm font-semibold leading-tight">LLM Router</h1>
            <p className="text-[11px] text-muted-foreground">Round-Robin Proxy</p>
          </div>
        </div>

        <nav className="flex-1 p-3 space-y-1">
          {navItems.map((item) => (
            <button
              key={item.path}
              onClick={() => navigate(item.path)}
              className={`flex items-center gap-3 w-full px-3 py-2.5 rounded-md text-sm font-medium transition-colors ${
                currentPath === item.path
                  ? 'bg-accent text-accent-foreground'
                  : 'text-muted-foreground hover:bg-accent/50'
              }`}
            >
              <item.icon className="h-4 w-4" />
              {item.label}
            </button>
          ))}
        </nav>

        <div className="p-3 border-t">
          <button
            onClick={handleLogout}
            className="flex items-center gap-3 w-full px-3 py-2.5 rounded-md text-sm font-medium text-muted-foreground hover:bg-accent/50 transition-colors"
          >
            <LogOut className="h-4 w-4" />
            Sign Out
          </button>
        </div>
      </aside>

      {/* Main content */}
      <div className="lg:pl-56 min-h-screen">
        {/* Desktop topbar */}
        <header className="hidden lg:flex sticky top-0 z-20 items-center justify-between h-14 border-b bg-background/95 backdrop-blur px-6">
          <h2 className="text-sm font-semibold">{pageTitle}</h2>
          <div className="flex items-center gap-2">
            {currentPath !== '/stats' && (
              <Button size="sm" onClick={handleRefresh}>
                <RefreshCw className="h-3.5 w-3.5 mr-1.5" />
                Refresh
              </Button>
            )}
          </div>
        </header>

        <main className="p-4 lg:p-6 pt-16 lg:pt-6">
          <Routes>
            <Route path="/" element={<Navigate to="/targets" replace />} />
            <Route path="/targets" element={
              <TargetsPage token={token} targets={targets} onRefresh={refreshTargets} />
            } />
            <Route path="/groups" element={
              <GroupsPage token={token} groups={groups} targets={targets} onRefresh={refreshGroups} />
            } />
            <Route path="/stats" element={<StatsPage token={token} />} />
          </Routes>
        </main>
      </div>
    </div>
  )
}
