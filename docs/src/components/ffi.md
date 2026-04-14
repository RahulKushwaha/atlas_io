# C FFI

The client library exports a C-compatible API for use from the C++ RocksDB Env shim.

## Functions

```c
#include "atlas_client.h"

AtlasClient* atlas_client_create(uint32_t instance_id);
void          atlas_client_destroy(AtlasClient* client);
int64_t       atlas_client_open(AtlasClient* client, const char* path, uint32_t flags);
int32_t       atlas_client_read(AtlasClient* client, uint64_t fd, uint8_t* buf, uint64_t offset, uint32_t len);
int32_t       atlas_client_write(AtlasClient* client, uint64_t fd, const uint8_t* buf, uint64_t offset, uint32_t len);
int32_t       atlas_client_sync(AtlasClient* client, uint64_t fd);
int32_t       atlas_client_close(AtlasClient* client, uint64_t fd);
```

## Return Values

- `atlas_client_create` — Returns `NULL` on failure
- `atlas_client_open` — Returns virtual fd (positive) or negative errno
- `atlas_client_read` / `atlas_client_write` — Returns bytes transferred or negative errno
- `atlas_client_sync` / `atlas_client_close` — Returns 0 on success or negative errno

## Build Outputs

The `atlas-client` crate builds as:
- `libatlas_client.a` — Static library
- `libatlas_client.dylib` / `libatlas_client.so` — Dynamic library

Link against either when compiling the C++ Env shim.
