import { Routes, Route, Navigate } from 'react-router-dom'
import { AuthProvider, useAuth } from '@/hooks/use-api'
import LoginPage from '@/pages/LoginPage'
import Dashboard from '@/components/layout'

function AppContent() {
  const { token } = useAuth()

  if (!token) {
    return (
      <Routes>
        <Route path="/*" element={<LoginPage />} />
      </Routes>
    )
  }

  return (
    <Routes>
      <Route path="/*" element={<Dashboard />} />
    </Routes>
  )
}

export default function App() {
  return (
    <AuthProvider>
      <AppContent />
    </AuthProvider>
  )
}
