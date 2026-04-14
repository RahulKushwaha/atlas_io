#pragma once
#include "rocksdb/env.h"
#include "../include/atlas_client.h"

namespace atlas {

// RandomAccessFile that delegates reads to Atlas I/O service
class AtlasRandomAccessFile : public rocksdb::RandomAccessFile {
public:
    AtlasRandomAccessFile(AtlasClient* client, uint64_t fd)
        : client_(client), fd_(fd) {}
    ~AtlasRandomAccessFile() override {
        atlas_client_close(client_, fd_);
    }
    rocksdb::Status Read(uint64_t offset, size_t n, rocksdb::Slice* result,
                         char* scratch) const override;
private:
    AtlasClient* client_;
    uint64_t fd_;
};

// WritableFile that delegates writes/sync to Atlas I/O service
class AtlasWritableFile : public rocksdb::WritableFile {
public:
    AtlasWritableFile(AtlasClient* client, uint64_t fd)
        : client_(client), fd_(fd), offset_(0) {}
    ~AtlasWritableFile() override {
        if (fd_ != 0) atlas_client_close(client_, fd_);
    }
    rocksdb::Status Append(const rocksdb::Slice& data) override;
    rocksdb::Status Close() override;
    rocksdb::Status Flush() override;
    rocksdb::Status Sync() override;
    rocksdb::Status Fsync() override;
private:
    AtlasClient* client_;
    uint64_t fd_;
    uint64_t offset_;
};

// EnvWrapper that intercepts hot-path file ops, delegates metadata to default Env
class AtlasEnv : public rocksdb::EnvWrapper {
public:
    AtlasEnv(AtlasClient* client)
        : rocksdb::EnvWrapper(rocksdb::Env::Default()), client_(client) {}

    rocksdb::Status NewRandomAccessFile(
        const std::string& fname,
        std::unique_ptr<rocksdb::RandomAccessFile>* result,
        const rocksdb::EnvOptions& options) override;

    rocksdb::Status NewWritableFile(
        const std::string& fname,
        std::unique_ptr<rocksdb::WritableFile>* result,
        const rocksdb::EnvOptions& options) override;

    // All other methods (DeleteFile, RenameFile, GetChildren, etc.)
    // are inherited from EnvWrapper → delegate to default Env.

private:
    AtlasClient* client_;
};

} // namespace atlas

extern "C" {
    // Creates an AtlasEnv and returns it as a rocksdb_env_t* for use with Env::from_raw()
    void* create_atlas_env(uint32_t instance_id);
}
