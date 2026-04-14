# Testing

## Run All Tests

```bash
cargo test -- --test-threads=1
```

> **Note:** `--test-threads=1` is required because the end-to-end tests use shared memory segments with fixed names that would collide under parallel execution.

## Test Suites

### Protocol Tests (`atlas-protocol`)

```bash
cargo test -p atlas-protocol
```

- `AnyBitPattern` compliance for `IoRequest` and `IoResponse`
- Struct size stability (296 and 32 bytes)
- Path string round-trip
- Op/priority enum round-trip

### Executor Tests (`atlas-service`)

```bash
cargo test -p atlas-service executor
```

- Open/create/close lifecycle
- Open nonexistent file returns error
- Write + read round-trip
- Positional write at offset
- Fsync on valid and invalid fds
- Pread/close on bad fds
- Fsync persistence verification via `std::fs`

### Batch & Scheduler Tests (`atlas-service`)

```bash
cargo test -p atlas-service batch
cargo test -p atlas-service scheduler
```

- Adjacent read merging
- No merge across different fds
- Priority ordering (Critical > High > Low)

### End-to-End Tests (`atlas-client`)

```bash
cargo test -p atlas-client --test e2e -- --test-threads=1
```

- Single instance: open → write → sync → close → reopen → read → verify
- Multi-instance: 4 concurrent instances each writing/reading their own file
