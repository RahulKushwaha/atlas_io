#include "atlas_env.h"
#include <fcntl.h>

namespace atlas {

// --- AtlasRandomAccessFile ---

rocksdb::Status AtlasRandomAccessFile::Read(
    uint64_t offset, size_t n, rocksdb::Slice* result, char* scratch) const {
    int32_t rc = atlas_client_read(
        const_cast<AtlasClient*>(client_), fd_,
        reinterpret_cast<uint8_t*>(scratch), offset, static_cast<uint32_t>(n));
    if (rc < 0) {
        *result = rocksdb::Slice();
        return rocksdb::Status::IOError("Atlas read failed");
    }
    *result = rocksdb::Slice(scratch, static_cast<size_t>(rc));
    return rocksdb::Status::OK();
}

// --- AtlasWritableFile ---

rocksdb::Status AtlasWritableFile::Append(const rocksdb::Slice& data) {
    int32_t rc = atlas_client_write(
        client_, fd_,
        reinterpret_cast<const uint8_t*>(data.data()),
        offset_, static_cast<uint32_t>(data.size()));
    if (rc < 0) {
        return rocksdb::Status::IOError("Atlas write failed");
    }
    offset_ += data.size();
    return rocksdb::Status::OK();
}

rocksdb::Status AtlasWritableFile::Close() {
    int32_t rc = atlas_client_close(client_, fd_);
    fd_ = 0; // prevent double-close in destructor
    return rc == 0 ? rocksdb::Status::OK()
                   : rocksdb::Status::IOError("Atlas close failed");
}

rocksdb::Status AtlasWritableFile::Flush() {
    return rocksdb::Status::OK(); // no-op, data already in shared memory
}

rocksdb::Status AtlasWritableFile::Sync() {
    int32_t rc = atlas_client_sync(client_, fd_);
    return rc == 0 ? rocksdb::Status::OK()
                   : rocksdb::Status::IOError("Atlas sync failed");
}

rocksdb::Status AtlasWritableFile::Fsync() {
    return Sync();
}

// --- AtlasEnv ---

rocksdb::Status AtlasEnv::NewRandomAccessFile(
    const std::string& fname,
    std::unique_ptr<rocksdb::RandomAccessFile>* result,
    const rocksdb::EnvOptions& /*options*/) {
    int64_t fd = atlas_client_open(client_, fname.c_str(), O_RDONLY);
    if (fd < 0) {
        result->reset();
        return rocksdb::Status::IOError("Atlas open failed: " + fname);
    }
    result->reset(new AtlasRandomAccessFile(client_, static_cast<uint64_t>(fd)));
    return rocksdb::Status::OK();
}

rocksdb::Status AtlasEnv::NewWritableFile(
    const std::string& fname,
    std::unique_ptr<rocksdb::WritableFile>* result,
    const rocksdb::EnvOptions& /*options*/) {
    int64_t fd = atlas_client_open(
        client_, fname.c_str(), O_WRONLY | O_CREAT | O_TRUNC);
    if (fd < 0) {
        result->reset();
        return rocksdb::Status::IOError("Atlas open failed: " + fname);
    }
    result->reset(new AtlasWritableFile(client_, static_cast<uint64_t>(fd)));
    return rocksdb::Status::OK();
}

} // namespace atlas

// --- C FFI entry point ---

extern "C" void* create_atlas_env(uint32_t instance_id) {
    AtlasClient* client = atlas_client_create(instance_id);
    if (!client) return nullptr;
    // Create the AtlasEnv on the heap. The caller (Rust) takes ownership.
    auto* env = new atlas::AtlasEnv(client);
    // rocksdb_env_t is just a wrapper around Env*. We return the Env* directly
    // and let the Rust side wrap it via Env::from_raw().
    // Note: rocksdb_env_t in the C API is { Env* rep; bool is_default; }
    // We need to allocate that struct.
    struct rocksdb_env_t {
        rocksdb::Env* rep;
        bool is_default;
    };
    auto* c_env = new rocksdb_env_t{env, false};
    return c_env;
}
