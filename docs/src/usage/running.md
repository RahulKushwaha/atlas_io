# Running the Daemon

## Basic Usage

```bash
# Register specific instance IDs
atlas-service 0 1 2 3 4

# With io_uring on Linux
ATLAS_USE_URING=1 atlas-service 0 1 2 3 4
```

Instance IDs are passed as positional arguments. Each ID corresponds to a set of shared memory segments that a client will create.

## Startup Order

1. **Client starts first** — Creates the shared memory segments
2. **Daemon starts second** — Joins the existing segments

This is important: the daemon will fail to register an instance if the client hasn't created the shared memory yet.

## Stopping

The daemon handles `SIGINT` and `SIGTERM` gracefully. Press Ctrl+C or send a signal:

```bash
kill -TERM $(pidof atlas-service)
```

## Running as a systemd Service

```ini
[Unit]
Description=Atlas I/O Service
After=network.target

[Service]
Type=simple
ExecStart=/usr/local/bin/atlas-service 0 1 2 3 4
Environment=ATLAS_USE_URING=1
Restart=on-failure

[Install]
WantedBy=multi-user.target
```
