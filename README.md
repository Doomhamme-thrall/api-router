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

## Quick Start

1. Install Rust toolchain.
2. Edit `config/router.json`:
   - set `jwt_secret`
   - update admin password hash if needed
   - add valid upstream API keys and enable targets
3. Run:

```bash
cargo run
```

Server starts at `0.0.0.0:8080` by default.

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
  - `id`, `name`, `provider`, `base_url`, `api_key`
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

## Ubuntu Deployment

1. Build release binary:

```bash
cargo build --release
```

2. Copy files to server:

- binary -> `/opt/llm-router/llm-router`
- `config/router.json` -> `/opt/llm-router/config/router.json`
- `ui/` -> `/opt/llm-router/ui/`

3. Install systemd service:

```bash
sudo cp deploy/llm-router.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now llm-router
```

4. Install nginx config:

```bash
sudo cp deploy/nginx.conf /etc/nginx/sites-available/llm-router
sudo ln -s /etc/nginx/sites-available/llm-router /etc/nginx/sites-enabled/llm-router
sudo nginx -t && sudo systemctl reload nginx
```

## Ubuntu Run Script

Project includes a helper script:

- [deploy/start-router-ubuntu.sh](deploy/start-router-ubuntu.sh)

Make it executable:

```bash
chmod +x deploy/start-router-ubuntu.sh
```

Run with defaults:

```bash
./deploy/start-router-ubuntu.sh
```

Common options via environment variables:

```bash
# Bind to all interfaces
BIND_ADDR=0.0.0.0:8080 ./deploy/start-router-ubuntu.sh

# Use binary mode (no cargo required)
MODE=binary BINARY_PATH=./llm-router ./deploy/start-router-ubuntu.sh

# Skip cargo check
SKIP_BUILD=1 ./deploy/start-router-ubuntu.sh
```

If your server cargo is old and shows lockfile error like "lock file version 4 requires -Znext-lockfile-bump":

```bash
rm -f Cargo.lock
cargo generate-lockfile
cargo build --release
```

The Ubuntu run script already handles this automatically when using cargo mode.

## Security Notes

- Change default admin password hash before production.
- Use a strong random `jwt_secret`.
- Keep upstream API keys secure and with minimum privileges.
- Consider restricting `/admin/*` with IP allowlist on Nginx.
