// .ptf descriptor classes decoded so far. Layouts and transfer order come from
// sandbox.exe; see docs/ptf/TrackDescriptor.md for the disassembly notes.
#pragma once

#include "archive.hpp"

#include <array>
#include <cstddef>
#include <cstdint>
#include <optional>
#include <string>
#include <vector>

namespace ptf {

/// Base of every *Descriptor (sandbox.exe 0x00486BB0):
/// int32 base version (must be 1), then an optional name string.
class Descriptor : public PersistentObject {
public:
    std::optional<std::string> name;

protected:
    void transferBase(Archive& ar);
};

/// Root object of a .ptf (sandbox.exe: transfer 0x0048BAA0, vtable 0x004AD0E0,
/// factory 0x0048B230 allocating 0x51 bytes, ctor 0x0048B6B0).
class TrackDescriptor : public Descriptor {
public:
    static constexpr const char* kClassName = "TrackDescriptor";
    static constexpr std::int32_t kVersion = 8;  // highest version Sandbox reads; always written

    const char* className() const override { return kClassName; }
    void transfer(Archive& ar) override;

    /// Base + scalar fields + records, i.e. everything before the child objects.
    void transferHeader(Archive& ar);
    void transferChildren(Archive& ar);

    std::int32_t version = kVersion;   // file version that was read
    std::uint8_t night = 0;            // +0x30  CenterLine "night" (0 day, 1 night), v>=3
    std::array<double, 3> lighting{0.2, 0.2, 0.2};  // +0x31/+0x39/+0x41  CenterLine unk_1..unk_3, v>=8
    std::uint32_t unknown2C = 0;       // +0x2C  4 bytes, meaning unknown (hillside: 0x40278D36 = 2.618f), v>=8
    std::vector<std::array<std::uint8_t, 48>> records;  // +0x49 count / +0x4D data, v>=7

    ObjPtr extra;                      // +0x24  single object, v==2 or v>=4 (null in hillside)
    std::vector<ObjPtr> listA;         // +0x1C count / +0x20 objects, v==2 or v>=4
    std::vector<ObjPtr> listB;         // +0x14 count / +0x18 objects
    std::vector<ObjPtr> segments;      // +0x10 count / +0x0C objects; count is stored early, must be > 0

    // Reader bookkeeping
    std::int32_t declaredSegments = 0;  // the +0x10 count as stored at the top of the object
    std::int32_t declaredListA = -1;
    std::int32_t declaredListB = -1;
    std::size_t headerEnd = 0;          // file offset just after transferHeader()
};

/// Register every decoded class with the archive factory.
void registerDescriptors();

}  // namespace ptf
