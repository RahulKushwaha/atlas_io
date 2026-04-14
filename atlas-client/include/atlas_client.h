#ifndef ATLAS_CLIENT_H
#define ATLAS_CLIENT_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct AtlasClient AtlasClient;

AtlasClient* atlas_client_create(uint32_t instance_id);
void atlas_client_destroy(AtlasClient* client);
int64_t atlas_client_open(AtlasClient* client, const char* path, uint32_t flags);
int32_t atlas_client_read(AtlasClient* client, uint64_t fd, uint8_t* buf, uint64_t offset, uint32_t len);
int32_t atlas_client_write(AtlasClient* client, uint64_t fd, const uint8_t* buf, uint64_t offset, uint32_t len);
int32_t atlas_client_sync(AtlasClient* client, uint64_t fd);
int32_t atlas_client_close(AtlasClient* client, uint64_t fd);

#ifdef __cplusplus
}
#endif

#endif // ATLAS_CLIENT_H
