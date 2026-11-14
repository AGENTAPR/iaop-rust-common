# Axum Microservice Template

Canonical scaffold generator for IAOP Rust services.

## Usage

```powershell
python scaffold.py <repo_dir> <service_name> <service_title> <db_name>
# example:
python scaffold.py ../iaop-agent-service iaop-agent-service "Agent Lifecycle" agent_db
```

Generates a Cargo workspace with four crates honoring hexagonal dependency
rules:

- `crates/domain` — pure domain model (`#![forbid(unsafe_code)]`, no infra deps)
- `crates/application` — use cases + ports (`EventPublisher`, `IntegrationEvent`)
- `crates/infrastructure` — config, PostgreSQL pool/migrations, outbox/inbox, security helpers
- `crates/api` — Axum router: `/health/live`, `/health/ready`, `/api/v1/system/info`, `/api/docs/openapi.json`, request-id/timeout middleware, RFC 9457 errors
- `src/main.rs` — composition root with graceful shutdown
- `migrations/` — outbox + inbox SQL templates (SQLx timestamp naming)
- `.github/workflows/main-ci.yml` — CI triggered ONLY by pushes to `main`
- `tools/verify-local.ps1` — local verification (fmt/clippy/test/build)
- `Dockerfile` — multi-stage, non-root, distroless-style slim runtime

After generating: `cargo fmt --all`, `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`, `cargo test --workspace --locked`, `cargo build --release --locked`.

Note: the generated README/docs should be reconciled with the service's own
README — the generator does not overwrite existing files.
