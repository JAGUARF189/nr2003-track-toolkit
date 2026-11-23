// Papyrus rts.dll "Archive" serialization, as used by .ptf track files.
//
// Reverse-engineered from rts.dll (Sandbox 2003). One transfer() routine per
// class both reads and writes, exactly like the original:
//
//   transferBytes(p, n)          raw little-endian bytes
//   transferString(s)            int32 length incl. NUL (0 = null), then the bytes
//   transferPointerButNotData    int32 byte size only (0 = null); caller then
//                                transfers the data with transferBytes
//   transferPersistentObject     int32 object id (0 = null). If the id was seen
//                                before, that's all (back-reference). Otherwise:
//                                int32 class id, class name string only the first
//                                time that class id appears, then the object's
//                                own transfer(). Object and class ids are handed
//                                out from ONE counter starting at 1.
//
// See docs/ptf/archive-format.md.
#pragma once

#include <cstddef>
#include <cstdint>
#include <functional>
#include <memory>
#include <optional>
#include <stdexcept>
#include <string>
#include <type_traits>
#include <unordered_map>
#include <vector>

namespace ptf {

struct ArchiveError : std::runtime_error {
    std::size_t offset;
    ArchiveError(const std::string& msg, std::size_t off) : std::runtime_error(msg), offset(off) {}
};

/// A class name the reader has no decoder for yet.
struct UnknownClass : ArchiveError {
    std::string className;
    UnknownClass(const std::string& name, std::size_t off)
        : ArchiveError("class '" + name + "' is not decoded yet", off), className(name) {}
};

class Archive;

class PersistentObject {
public:
    virtual ~PersistentObject() = default;
    virtual const char* className() const = 0;
    virtual void transfer(Archive& ar) = 0;

    // Set by the reader.
    std::size_t fileOffset = 0;  // offset of the object id
    std::uint32_t objectId = 0;
};

using ObjPtr = std::shared_ptr<PersistentObject>;
using Factory = std::function<ObjPtr()>;

void registerClass(const std::string& name, Factory make);
ObjPtr instantiate(const std::string& name);

/// One labelled field the reader consumed (for reports and docs).
struct TraceEntry {
    std::string label;
    std::size_t offset;
    std::size_t size;
};

/// Something structurally odd that doesn't stop parsing.
struct Anomaly {
    std::size_t offset;
    std::string message;
};

class Archive {
public:
    static Archive forReading(std::vector<std::uint8_t> data);
    static Archive forWriting();

    bool isReading() const { return reading_; }
    std::size_t tell() const { return pos_; }
    const std::vector<std::uint8_t>& buffer() const { return data_; }

    void transferBytes(void* p, std::size_t n);

    template <class T>
    void transfer(T& v) {
        static_assert(std::is_trivially_copyable_v<T>);
        transferBytes(&v, sizeof(T));
    }

    void transferString(std::optional<std::string>& s);

    /// Writes `byteSize` (pass 0 for null). When reading, returns the size stored.
    std::uint32_t transferPointerButNotData(std::uint32_t byteSize);

    void transferPersistentObject(ObjPtr& obj);

    template <class T>
    void transferObject(std::shared_ptr<T>& p) {
        ObjPtr o = p;
        transferPersistentObject(o);
        if (reading_) {
            p = std::dynamic_pointer_cast<T>(o);
            if (o && !p) throw ArchiveError(std::string("unexpected class ") + o->className(), o->fileOffset);
        }
    }

    /// Object read with this id (objects are registered before their transfer()
    /// runs, so this works even after a parse stopped part-way).
    ObjPtr objectById(std::uint32_t id) const {
        auto it = objById_.find(id);
        return it == objById_.end() ? nullptr : it->second;
    }

    /// Label the next transfer (reading only; feeds the trace).
    void label(const char* name) { label_ = name; }

    std::vector<TraceEntry> trace;
    std::vector<Anomaly> anomalies;
    void anomaly(std::size_t off, std::string msg) { anomalies.push_back({off, std::move(msg)}); }

    /// Called for every newly read object, before its transfer() runs.
    std::function<void(const PersistentObject&)> onObject;

private:
    Archive() = default;

    bool reading_ = true;
    std::vector<std::uint8_t> data_;
    std::size_t pos_ = 0;
    std::string label_;

    std::uint32_t nextId_ = 1;  // writer: next id; reader: id we expect next
    std::unordered_map<std::uint32_t, ObjPtr> objById_;
    std::unordered_map<const PersistentObject*, std::uint32_t> idByObj_;
    std::unordered_map<std::uint32_t, std::string> classById_;
    std::unordered_map<std::string, std::uint32_t> idByClass_;
};

}  // namespace ptf
