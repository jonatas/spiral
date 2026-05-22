# Spiral Vortex Server

Sidecar API for the Spiral storage engine.

## Prerequisites
- Rust 1.75+
- PostgreSQL with the `spiral` extension installed.

## Running
```bash
# Update .env with your DATABASE_URL
cargo run
```

## API Endpoints
- `GET /api/metadata`: List all spiral-managed relations.
- `GET /api/metadata/:name`: Get metadata for a specific relation.
- `GET /api/changelog`: Get the latest 100 entries from the changelog.
- `WS /ws`: WebSocket endpoint for real-time events (`ChangelogUpdate`, `StorageStats`).
