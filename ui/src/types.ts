export interface Target {
  id: string
  name: string
  provider: string
  api_format: string
  base_url: string
  api_key: string
  router_model: string
  upstream_model: string
  enabled: boolean
}

export interface ModelGroup {
  id: string
  name: string
  target_ids: string[]
  enabled: boolean
}

export interface StatsItem {
  target_name: string
  total_calls: number
  success_count: number
  error_count: number
  prompt_tokens: number
  completion_tokens: number
  total_tokens: number
}

export interface ApiError {
  error?: {
    message?: string
  }
  message?: string
}

export interface LoginResponse {
  token: string
  expires_at: number
}
