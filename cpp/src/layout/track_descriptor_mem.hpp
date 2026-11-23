// In-memory layout of sandbox.exe's TrackDescriptor (32-bit process, MSVC 6).
// Reference only: the readers in descriptors.hpp don't depend on it, but the
// static_asserts keep the documented offsets honest. Pointers are stored as
// uint32_t because the original is a 32-bit image.
//
// Evidence: factory 0x0048B230 calls _memAlloc(&p, 0x51); transfer 0x0048BAA0
// touches every field below at these exact offsets; PersistentObject::addRef
// increments +0x04; Descriptor's base transfer (0x00486BB0) passes this+0x08 to
// transferString.
#pragma once

#include <cstddef>
#include <cstdint>

namespace ptf::mem {

#pragma pack(push, 1)
struct TrackDescriptor {
    std::uint32_t vtable;        // 0x00  -> 0x004AD0E0 (slot 0 = transfer, slot 1 = getClassName)
    std::int32_t refCount;       // 0x04  PersistentObject
    std::uint32_t name;          // 0x08  char*  Descriptor name (null in stock tracks)
    std::uint32_t segments;      // 0x0C  PersistentObject*[segmentCount]  (SegmentDescriptor)
    std::int32_t segmentCount;   // 0x10  stored right after the version; must be > 0
    std::int32_t listBCount;     // 0x14
    std::uint32_t listB;         // 0x18  PersistentObject*[listBCount]
    std::int32_t listACount;     // 0x1C
    std::uint32_t listA;         // 0x20  PersistentObject*[listACount]  (first: TrackDetailDescriptor)
    std::uint32_t extra;         // 0x24  PersistentObject*  (null in hillside)
    std::uint32_t unknown28;     // 0x28  not serialized
    std::uint32_t unknown2C;     // 0x2C  serialized as 4 raw bytes, v>=8 (hillside 2.618f)
    std::uint8_t night;          // 0x30  CenterLine "night"
    double lighting[3];          // 0x31  CenterLine unk_1..unk_3 (unaligned: struct is packed)
    std::int32_t recordCount;    // 0x49
    std::uint32_t records;       // 0x4D  48-byte records[recordCount]
};
#pragma pack(pop)

static_assert(sizeof(TrackDescriptor) == 0x51, "factory allocates 0x51 bytes");
static_assert(offsetof(TrackDescriptor, refCount) == 0x04);
static_assert(offsetof(TrackDescriptor, name) == 0x08);
static_assert(offsetof(TrackDescriptor, segments) == 0x0C);
static_assert(offsetof(TrackDescriptor, segmentCount) == 0x10);
static_assert(offsetof(TrackDescriptor, listBCount) == 0x14);
static_assert(offsetof(TrackDescriptor, listB) == 0x18);
static_assert(offsetof(TrackDescriptor, listACount) == 0x1C);
static_assert(offsetof(TrackDescriptor, listA) == 0x20);
static_assert(offsetof(TrackDescriptor, extra) == 0x24);
static_assert(offsetof(TrackDescriptor, unknown2C) == 0x2C);
static_assert(offsetof(TrackDescriptor, night) == 0x30);
static_assert(offsetof(TrackDescriptor, lighting) == 0x31);
static_assert(offsetof(TrackDescriptor, recordCount) == 0x49);
static_assert(offsetof(TrackDescriptor, records) == 0x4D);

}  // namespace ptf::mem
