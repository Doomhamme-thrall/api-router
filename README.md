# llm-router

A lightweight OpenAI-compatible LLM router with global round-robin, model-group scoped round-robin, and a built-in admin Web UI.

## Features

- OpenAI-compatible endpoints:
  - `POST /v1/chat/completions`
  - `POST /v1/embeddings`
  - `GET /v1/models`
- Global round-robin across all enabled upstream targets.
- Model group routing: when request `model` equals a model-group name, dispatch only within that group (no global fallback).
- Admin API + Web UI (`/ui`) for provider/model target management.
- File-based config (`config/router.json`) for easy single-host deployment.
- JWT-protected admin operations.
- Persistent usage statistics log (JSONL, human-readable) for `/admin/stats`.

## Quick Start

无需安装 Rust 或 Node.js，直接使用 CI 自动编译的二进制即可运行。

```bash
# 1. 克隆仓库
git clone https://github.com/Doomhamme-thrall/api-router
cd llm-router

# 2. 编辑配置文件（填上你的 API keys）
vim config/router.json

# 3. 直接运行（使用预编译二进制，无需编译）
./release/llm-router
```

CI 每次推送到 `main` 分支时自动编译并将二进制提交到 `release/` 目录，

### 从源码编译（开发者）

```bash
cargo build --release
./target/release/llm-router
```

Server starts at `0.0.0.0:8080` by default.

## One-Click Deployment (Ubuntu/Debian)

```bash
bash deploy/deploy-ubuntu.sh
```

## One-Click Deployment (Windows)

```powershell
.\deploy\deploy-windows.ps1
```

## OpenAI-Compatible Call Example

```bash
curl http://127.0.0.1:8080/v1/chat/completions \
  -H "Authorization: Bearer client-demo-key" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "any-model-name",
    "messages": [{"role":"user","content":"hello"}],
    "stream": true
  }'
```

Routing behavior:

- If `model` matches an enabled model-group name, router only round-robins inside that group's targets.
- If `model` does not match any model-group name, router uses global round-robin across all enabled targets.
- Router rewrites request model to selected target `upstream_model` before forwarding.

## Admin API

### Login

`POST /admin/login`

```json
{
  "username": "admin",
  "password": "admin123"
}
```

### Manage targets

- `GET /admin/targets`
- `POST /admin/targets`
- `PUT /admin/targets/:id`
- `DELETE /admin/targets/:id`

### Manage model groups

- `GET /admin/model-groups`
- `POST /admin/model-groups`
- `PUT /admin/model-groups/:id`
- `DELETE /admin/model-groups/:id`

`target_ids` in a model group must reference existing target IDs.

Requires `Authorization: Bearer <admin_jwt>`.

## Config Format

`config/router.json` fields:

- `admin.username`
- `admin.password_sha256`
- `jwt_secret`
- `client_api_keys[]`
- `targets[]`:
  - `id`, `name`, `api_format`, `base_url`, `api_key`
  - `router_model`, `upstream_model`, `enabled`
- `model_groups[]`:
  - `id`, `name`, `target_ids[]`, `enabled`

## Model Group Example

Use model-group name as request model:

```bash
curl http://127.0.0.1:8080/v1/chat/completions \
  -H "Authorization: Bearer client-demo-key" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "team-default",
    "messages": [{"role":"user","content":"hello"}]
  }'
```

If `team-default` exists and is enabled, request will only route within that group's members.

## Gemini Target Configuration

`targets[]` now supports `api_format`:

- `openai` (default): use OpenAI-compatible upstream format
- `gemini`: convert OpenAI chat-completions requests to Gemini `generateContent`

Gemini target example:

```json
{
  "id": "f0f7e4be-4c77-4ae3-9058-1d9a14cbf7a1",
  "name": "gemini-flash",
  "api_format": "gemini",
  "base_url": "https://generativelanguage.googleapis.com",
  "api_key": "AIza...",
  "router_model": "chat-fast",
  "upstream_model": "gemini-2.0-flash",
  "enabled": true
}
```

Notes:

- For Gemini targets, `api_key` is sent as URL query parameter `key`.
- `upstream_model` is used to build `/v1beta/models/{model}:generateContent`.
- Current Gemini compatibility is for `POST /v1/chat/completions` routing.

## CI Auto Build

Repository includes workflow: `.github/workflows/build-linux.yml`

| 触发方式 | 行为 |
|---|---|
| 推送到 `main` 分支 | 自动编译 Linux x86_64 binary + 前端 UI，**提交到仓库 `release/` 目录** |
| 推送标签 `v*` | 在上述基础上，额外发布 GitHub Release 完整包 |
| 手动触发 (`workflow_dispatch`) | 同上 |

所以你 clone 这个仓库时，`release/` 目录里就已经有了最新编译好的二进制和前端页面。

## Security Notes

- Change default admin password hash before production.
- Use a strong random `jwt_secret`.
- Keep upstream API keys secure and with minimum privileges.
- Consider restricting `/admin/*` with IP allowlist on Nginx.

## Usage Statistics Persistence

- Usage records are persisted to daily JSONL shards under `data/usage/` by default.
- File naming pattern: `usage-YYYY-MM-DD.jsonl`.
- Format is JSON Lines (one JSON object per line), human-readable and easy to inspect/grep.
- On startup, router loads recent records into memory and continuously appends new records to disk.
- `/admin/stats` aggregates from persisted shard files, so stats survive process restarts.

Optional env vars:

- `ROUTER_USAGE_LOG`: customize usage log directory (default `data/usage`).
- `ROUTER_MAX_CALL_RECORDS`: max in-memory cache size (default `100000`); does not limit persisted log file.
