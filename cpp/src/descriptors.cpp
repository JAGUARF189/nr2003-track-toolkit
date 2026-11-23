#include "descriptors.hpp"

#include "layout/track_descriptor_mem.hpp"  // compile-time check of the documented offsets

#include <format>

namespace ptf {

void Descriptor::transferBase(Archive& ar) {
    std::int32_t v = 1;
    const std::size_t at = ar.tell();
    ar.label("Descriptor.baseVersion");
    ar.transfer(v);
    if (ar.isReading() && v != 1)
        throw ArchiveError(std::format("Descriptor base version is {} (Sandbox only accepts 1)", v), at);
    ar.label("Descriptor.name");
    ar.transferString(name);
}

namespace {

/// int32 count, int32 byte size (count*4), then `count` objects.
/// Mirrors sandbox.exe 0x0048BE89 / 0x0048BF19.
void transferObjectList(Archive& ar, std::vector<ObjPtr>& list, std::int32_t& declared, const char* what) {
    std::int32_t count = ar.isReading() ? 0 : static_cast<std::int32_t>(list.size());
    const std::size_t at = ar.tell();
    ar.label(what);
    ar.transfer(count);
    if (ar.isReading()) {
        declared = count;
        if (count < 0) throw ArchiveError(std::format("{} has a negative count ({})", what, count), at);
        if (count > 1'000'000) throw ArchiveError(std::format("{} count {} is implausible", what, count), at);
    }
    if (count <= 0) return;
    const std::size_t sizeAt = ar.tell();
    const auto bytes = ar.transferPointerButNotData(static_cast<std::uint32_t>(count) * 4u);
    if (ar.isReading() && bytes != static_cast<std::uint32_t>(count) * 4u)
        ar.anomaly(sizeAt, std::format("{}: stored byte size {} but count {} needs {}", what, bytes, count, count * 4));
    if (ar.isReading()) list.assign(static_cast<std::size_t>(count), nullptr);
    for (auto& o : list) ar.transferPersistentObject(o);
}

}  // namespace

void TrackDescriptor::transferHeader(Archive& ar) {
    transferBase(ar);

    std::int32_t v = ar.isReading() ? 0 : kVersion;
    const std::size_t vAt = ar.tell();
    ar.label("TrackDescriptor.version");
    ar.transfer(v);
    if (ar.isReading()) {
        if (v > kVersion) throw ArchiveError(std::format("TrackDescriptor version {} is newer than Sandbox's {}", v, kVersion), vAt);
        if (v < 1) throw ArchiveError(std::format("TrackDescriptor version {} is invalid", v), vAt);
        version = v;
        // Defaults Sandbox applies before reading (0x0048BB10).
        night = 0;
        lighting = {0.2, 0.2, 0.2};
        unknown2C = 0;
        records.clear();
    }

    std::int32_t segCount = ar.isReading() ? 0 : static_cast<std::int32_t>(segments.size());
    ar.label("TrackDescriptor.segmentCount (+0x10)");
    ar.transfer(segCount);
    if (ar.isReading()) declaredSegments = segCount;

    if (v >= 3) {
        ar.label("TrackDescriptor.night (+0x30)");
        ar.transfer(night);
    }
    if (v >= 8) {
        ar.label("TrackDescriptor.lighting[0] (+0x31)");
        ar.transfer(lighting[0]);
        ar.label("TrackDescriptor.lighting[1] (+0x39)");
        ar.transfer(lighting[1]);
        ar.label("TrackDescriptor.lighting[2] (+0x41)");
        ar.transfer(lighting[2]);
        ar.label("TrackDescriptor.unknown2C (+0x2C)");
        ar.transfer(unknown2C);
    } else if (v >= 3) {
        // Older layouts: doubles Sandbox reads and throws away (0x0048BB70..0x0048BD36).
        const int legacy = 5 + (v >= 5 ? 8 : 0) + (v >= 6 ? 8 : 0);
        for (int i = 0; i < legacy; ++i) {
            double discard = 0;
            ar.label("TrackDescriptor.legacyDouble");
            ar.transfer(discard);
        }
    }

    if (!ar.isReading() || v >= 7) {
        std::int32_t count = ar.isReading() ? 0 : static_cast<std::int32_t>(records.size());
        const std::size_t at = ar.tell();
        ar.label("TrackDescriptor.recordCount (+0x49)");
        ar.transfer(count);
        if (ar.isReading() && (count < 0 || count > 100'000))
            throw ArchiveError(std::format("record count {} is implausible", count), at);
        if (count > 0) {
            const std::uint32_t need = static_cast<std::uint32_t>(count) * 48u;
            const std::size_t sizeAt = ar.tell();
            const auto bytes = ar.transferPointerButNotData(need);
            if (ar.isReading() && bytes != need)
                ar.anomaly(sizeAt, std::format("records: stored byte size {} but {} records need {}", bytes, count, need));
            if (ar.isReading()) records.resize(static_cast<std::size_t>(count));
            ar.label("TrackDescriptor.records (+0x4D)");
            ar.transferBytes(records.data(), need);
        }
    }
    if (ar.isReading()) headerEnd = ar.tell();
}

void TrackDescriptor::transferChildren(Archive& ar) {
    const std::int32_t v = ar.isReading() ? version : kVersion;
    if (v == 2 || v >= 4) {
        ar.transferPersistentObject(extra);
        transferObjectList(ar, listA, declaredListA, "TrackDescriptor.listA (+0x1C)");
    }
    transferObjectList(ar, listB, declaredListB, "TrackDescriptor.listB (+0x14)");

    // Segments: the count was stored at the top; here only the byte size and the objects.
    const std::int32_t count = ar.isReading() ? declaredSegments : static_cast<std::int32_t>(segments.size());
    const std::size_t at = ar.tell();
    if (count <= 0) throw ArchiveError("track has no segments (Sandbox refuses to load this)", at);
    const auto bytes = ar.transferPointerButNotData(static_cast<std::uint32_t>(count) * 4u);
    if (ar.isReading() && bytes != static_cast<std::uint32_t>(count) * 4u)
        ar.anomaly(at, std::format("segments: stored byte size {} but count {} needs {}", bytes, count, count * 4));
    if (ar.isReading()) segments.assign(static_cast<std::size_t>(count), nullptr);
    for (auto& s : segments) ar.transferPersistentObject(s);
}

void TrackDescriptor::transfer(Archive& ar) {
    transferHeader(ar);
    transferChildren(ar);
}

void registerDescriptors() {
    registerClass(TrackDescriptor::kClassName, [] { return std::make_shared<TrackDescriptor>(); });
}

}  // namespace ptf
