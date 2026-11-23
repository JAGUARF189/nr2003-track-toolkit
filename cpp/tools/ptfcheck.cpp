// ptfcheck: read-only structural check of a Papyrus .ptf track file.
//
//   ptfcheck <file.ptf> [--trace]
//
// Exit codes: 0 PASS, 1 FAIL, 2 PARTIAL (stopped at a class not decoded yet).
// The input file is never modified.

#include "archive.hpp"
#include "descriptors.hpp"

#include <cmath>
#include <cstdio>
#include <cstring>
#include <format>
#include <fstream>
#include <iostream>
#include <iterator>
#include <string>
#include <vector>

using namespace ptf;

namespace {

int g_fail = 0;

void pass(std::size_t off, const std::string& msg) { std::cout << std::format("[PASS] 0x{:06X}  {}\n", off, msg); }
void fail(std::size_t off, const std::string& msg) {
    ++g_fail;
    std::cout << std::format("[FAIL] 0x{:06X}  {}\n", off, msg);
}
void warn(std::size_t off, const std::string& msg) { std::cout << std::format("[WARN] 0x{:06X}  {}\n", off, msg); }

std::string hex(const std::uint8_t* p, std::size_t n) {
    std::string s;
    for (std::size_t i = 0; i < n; ++i) s += std::format("{:02X}{}", p[i], i + 1 < n ? " " : "");
    return s;
}

float asFloat(std::uint32_t u) {
    float f;
    std::memcpy(&f, &u, 4);
    return f;
}

void checkTrack(const TrackDescriptor& td, const std::vector<std::uint8_t>& file) {
    std::cout << "\nTrackDescriptor fields\n";
    std::cout << std::format("  version            {}\n", td.version);
    std::cout << std::format("  name               {}\n", td.name ? "\"" + *td.name + "\"" : "(null)");
    std::cout << std::format("  segment count      {}\n", td.declaredSegments);
    std::cout << std::format("  night              {}\n", td.night);
    std::cout << std::format("  lighting           {:.6g} {:.6g} {:.6g}\n", td.lighting[0], td.lighting[1], td.lighting[2]);
    std::cout << std::format("  unknown2C          0x{:08X} (float {:.6g}, int {})\n", td.unknown2C, asFloat(td.unknown2C),
                             static_cast<std::int32_t>(td.unknown2C));
    std::cout << std::format("  records            {}\n", td.records.size());
    for (std::size_t i = 0; i < td.records.size(); ++i) {
        const auto& r = td.records[i];
        std::cout << std::format("    [{}] {}\n", i, hex(r.data(), 16));
        std::cout << "        floats:";
        for (std::size_t k = 16; k < 48; k += 4) {
            std::uint32_t u;
            std::memcpy(&u, r.data() + k, 4);
            std::cout << std::format(" {:.6g}", asFloat(u));
        }
        std::cout << "\n";
    }
    if (td.declaredListA >= 0) std::cout << std::format("  list A (+0x1C)     {} objects declared\n", td.declaredListA);
    if (td.declaredListB >= 0) std::cout << std::format("  list B (+0x14)     {} objects declared\n", td.declaredListB);
    std::cout << "\nChecks\n";

    if (td.declaredSegments > 0 && td.declaredSegments < 100'000)
        pass(0, std::format("segment count {} is plausible", td.declaredSegments));
    else
        fail(0, std::format("segment count {} (Sandbox requires > 0)", td.declaredSegments));

    if (td.night <= 1) pass(0, std::format("night flag is {}", td.night));
    else warn(0, std::format("night flag is {} (expected 0 or 1)", td.night));

    bool lightOk = true;
    for (double d : td.lighting)
        if (!std::isfinite(d)) lightOk = false;
    if (lightOk) pass(0, "lighting values are finite");
    else fail(0, "lighting value is NaN/Inf");
    for (double d : td.lighting)
        if (std::isfinite(d) && (d < 0.0 || d > 1.0)) warn(0, std::format("lighting value {:.6g} outside 0..1", d));

    // Byte-exact re-encode of everything decoded so far.
    auto w = Archive::forWriting();
    std::uint32_t id = 1, cid = 2;
    w.transfer(id);
    w.transfer(cid);
    std::optional<std::string> cname = std::string(TrackDescriptor::kClassName);
    w.transferString(cname);
    TrackDescriptor copy = td;
    // The header stores the segment count; segment objects may not be decoded yet,
    // so give the copy placeholders matching the declared count.
    if (copy.declaredSegments > 0) copy.segments.resize(static_cast<std::size_t>(copy.declaredSegments));
    copy.transferHeader(w);
    const auto& out = w.buffer();
    if (td.version != TrackDescriptor::kVersion) {
        warn(0, std::format("version {} file: re-encode would upgrade to {}, byte compare skipped", td.version,
                            TrackDescriptor::kVersion));
    } else if (out.size() == td.headerEnd && std::equal(out.begin(), out.end(), file.begin())) {
        pass(0, std::format("re-encoded header is byte-identical (0x000000..0x{:06X}, {} bytes)", td.headerEnd, out.size()));
    } else {
        std::size_t i = 0;
        while (i < out.size() && i < file.size() && out[i] == file[i]) ++i;
        fail(i, std::format("re-encoded header differs from the file at this offset ({} vs {} bytes)", out.size(),
                            td.headerEnd));
    }
}

}  // namespace

int main(int argc, char** argv) {
    if (argc < 2) {
        std::cerr << "usage: ptfcheck <file.ptf> [--trace]\n";
        return 64;
    }
    const std::string path = argv[1];
    const bool showTrace = argc > 2 && std::string(argv[2]) == "--trace";

    std::ifstream in(path, std::ios::binary);
    if (!in) {
        std::cerr << "cannot open " << path << "\n";
        return 66;
    }
    std::vector<std::uint8_t> file((std::istreambuf_iterator<char>(in)), std::istreambuf_iterator<char>());
    std::cout << std::format("ptfcheck 0.1  {}  ({} bytes)\n\n", path, file.size());

    registerDescriptors();
    auto ar = Archive::forReading(file);
    std::vector<std::pair<std::size_t, std::string>> seen;
    ar.onObject = [&](const PersistentObject& o) { seen.emplace_back(o.fileOffset, o.className()); };

    bool partial = false;
    std::size_t stopAt = 0;
    std::string stopMsg;
    ObjPtr top;
    try {
        ar.transferPersistentObject(top);
        if (ar.tell() == file.size()) pass(ar.tell(), "parsed to the exact end of the file");
        else fail(ar.tell(), std::format("{} trailing bytes after the root object", file.size() - ar.tell()));
    } catch (const UnknownClass& e) {
        partial = true;
        stopAt = e.offset;
        stopMsg = e.what();
    } catch (const ArchiveError& e) {
        fail(e.offset, e.what());
    }

    if (!seen.empty() && seen.front().second == TrackDescriptor::kClassName && seen.front().first == 0)
        pass(0, "root object is TrackDescriptor (object id 1)");
    else
        fail(0, "root object is not a TrackDescriptor at offset 0");

    // Object 1 is the root. It stays in the archive's id table even if the parse
    // stopped inside one of its children, with every field read up to that point.
    if (auto td = std::dynamic_pointer_cast<TrackDescriptor>(ar.objectById(1))) checkTrack(*td, file);

    for (const auto& a : ar.anomalies) fail(a.offset, a.message);

    if (showTrace) {
        std::cout << "\nField trace\n";
        for (const auto& t : ar.trace) {
            const std::size_t n = t.size < 16 ? t.size : 16;
            std::cout << std::format("  0x{:06X} {:4}  {:42} {}{}\n", t.offset, t.size, t.label, hex(file.data() + t.offset, n),
                                     t.size > 16 ? " ..." : "");
        }
    }

    std::cout << "\nObjects read\n";
    for (const auto& [off, cls] : seen) std::cout << std::format("  0x{:06X}  {}\n", off, cls);

    std::cout << "\n";
    if (g_fail) {
        std::cout << std::format("RESULT: FAIL ({} problem{})\n", g_fail, g_fail == 1 ? "" : "s");
        return 1;
    }
    if (partial) {
        const double pct = 100.0 * static_cast<double>(stopAt) / static_cast<double>(file.size());
        std::cout << std::format("[PART] 0x{:06X}  {}\n", stopAt, stopMsg);
        std::cout << std::format("RESULT: PARTIAL — decoded 0x000000..0x{:06X} ({:.2f}% of the file), no failures\n", stopAt, pct);
        return 2;
    }
    std::cout << "RESULT: PASS\n";
    return 0;
}
