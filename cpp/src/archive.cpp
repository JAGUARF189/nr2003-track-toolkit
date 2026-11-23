#include "archive.hpp"

#include <cstring>
#include <format>

namespace ptf {

namespace {
std::unordered_map<std::string, Factory>& registry() {
    static std::unordered_map<std::string, Factory> r;
    return r;
}
}  // namespace

void registerClass(const std::string& name, Factory make) { registry()[name] = std::move(make); }

ObjPtr instantiate(const std::string& name) {
    auto it = registry().find(name);
    return it == registry().end() ? nullptr : it->second();
}

Archive Archive::forReading(std::vector<std::uint8_t> data) {
    Archive a;
    a.reading_ = true;
    a.data_ = std::move(data);
    return a;
}

Archive Archive::forWriting() {
    Archive a;
    a.reading_ = false;
    return a;
}

void Archive::transferBytes(void* p, std::size_t n) {
    if (reading_) {
        if (n > data_.size() - pos_) {
            label_.clear();
            throw ArchiveError(std::format("read of {} bytes runs past the end of the file ({} bytes)", n, data_.size()),
                               pos_);
        }
        if (!label_.empty()) trace.push_back({label_, pos_, n});
        std::memcpy(p, data_.data() + pos_, n);
    } else {
        const auto* b = static_cast<const std::uint8_t*>(p);
        data_.insert(data_.end(), b, b + n);
    }
    pos_ += n;
    label_.clear();
}

void Archive::transferString(std::optional<std::string>& s) {
    const std::size_t at = pos_;
    const std::string lbl = label_;
    std::uint32_t len = (!reading_ && s) ? static_cast<std::uint32_t>(s->size() + 1) : 0u;
    transfer(len);
    if (reading_) {
        if (len == 0) {
            s.reset();
            return;
        }
        if (len > data_.size() - pos_)
            throw ArchiveError(std::format("string length {} runs past the end of the file", len), at);
        std::string v(reinterpret_cast<const char*>(data_.data() + pos_), len);
        if (!lbl.empty()) trace.push_back({lbl + " (text)", pos_, len});
        pos_ += len;
        if (v.back() != '\0') throw ArchiveError("string is not NUL-terminated", at);
        v.pop_back();
        s = std::move(v);
    } else if (len) {
        transferBytes(s->data(), len);  // includes the terminating NUL
    }
}

std::uint32_t Archive::transferPointerButNotData(std::uint32_t byteSize) {
    std::uint32_t v = reading_ ? 0u : byteSize;
    transfer(v);
    return v;
}

void Archive::transferPersistentObject(ObjPtr& obj) {
    if (reading_) {
        const std::size_t at = pos_;
        std::uint32_t id = 0;
        label("object id");
        transfer(id);
        if (id == 0) {
            obj = nullptr;
            return;
        }
        if (auto it = objById_.find(id); it != objById_.end()) {
            obj = it->second;  // back-reference
            return;
        }
        if (id != nextId_) anomaly(at, std::format("object id {} out of sequence (expected {})", id, nextId_));
        nextId_ = id + 1;

        std::uint32_t cid = 0;
        const std::size_t cidAt = pos_;
        label("class id");
        transfer(cid);
        std::string name;
        if (auto it = classById_.find(cid); it != classById_.end()) {
            name = it->second;
        } else {
            if (cid != nextId_) anomaly(cidAt, std::format("class id {} out of sequence (expected {})", cid, nextId_));
            nextId_ = cid + 1;
            std::optional<std::string> n;
            label("class name");
            transferString(n);
            if (!n || n->empty()) throw ArchiveError("object has an empty class name", at);
            name = *n;
            classById_[cid] = name;
        }

        obj = instantiate(name);
        if (!obj) throw UnknownClass(name, at);
        obj->fileOffset = at;
        obj->objectId = id;
        objById_[id] = obj;  // registered before transfer(), like rts.dll, so cycles resolve
        if (onObject) onObject(*obj);
        obj->transfer(*this);
    } else {
        if (!obj) {
            std::uint32_t z = 0;
            transfer(z);
            return;
        }
        if (auto it = idByObj_.find(obj.get()); it != idByObj_.end()) {
            std::uint32_t id = it->second;
            transfer(id);
            return;
        }
        std::uint32_t id = nextId_++;
        idByObj_[obj.get()] = id;
        transfer(id);
        const std::string name = obj->className();
        if (auto it = idByClass_.find(name); it != idByClass_.end()) {
            std::uint32_t cid = it->second;
            transfer(cid);
        } else {
            std::uint32_t cid = nextId_++;
            idByClass_[name] = cid;
            transfer(cid);
            std::optional<std::string> n = name;
            transferString(n);
        }
        obj->transfer(*this);
    }
}

}  // namespace ptf
