// ptfjson: convert .ptf track files to JSON and back, and prove round-trips.
//
//   ptfjson read  <in.ptf> <out.json>     decode (use - for stdout)
//   ptfjson write <in.json> <out.ptf>     encode
//   ptfjson check <in.ptf>...             .ptf -> graph -> .ptf and .ptf -> JSON -> .ptf, byte-compare
//
// Exit code 0 = success. Errors go to stderr as one line starting with "error:".
#include "graph.hpp"

#include <chrono>
#include <cstdio>
#include <fstream>
#include <iostream>
#include <iterator>
#include <map>
#include <sstream>

namespace {

std::vector<std::uint8_t> loadBytes(const std::string& path) {
    std::ifstream f(path, std::ios::binary);
    if (!f) throw std::runtime_error("cannot open " + path);
    return {std::istreambuf_iterator<char>(f), std::istreambuf_iterator<char>()};
}

std::string loadText(const std::string& path) {
    std::ifstream f(path, std::ios::binary);
    if (!f) throw std::runtime_error("cannot open " + path);
    std::ostringstream s;
    s << f.rdbuf();
    return s.str();
}

void save(const std::string& path, const void* p, std::size_t n) {
    if (path == "-") { std::fwrite(p, 1, n, stdout); return; }
    const std::string tmp = path + ".tmp";
    {
        std::ofstream f(tmp, std::ios::binary | std::ios::trunc);
        if (!f) throw std::runtime_error("cannot write " + tmp);
        f.write(static_cast<const char*>(p), static_cast<std::streamsize>(n));
        if (!f) throw std::runtime_error("write failed: " + tmp);
    }
    std::remove(path.c_str());
    if (std::rename(tmp.c_str(), path.c_str()) != 0) throw std::runtime_error("cannot replace " + path);
}

std::size_t firstDiff(const std::vector<std::uint8_t>& a, const std::vector<std::uint8_t>& b) {
    const std::size_t n = std::min(a.size(), b.size());
    for (std::size_t k = 0; k < n; ++k)
        if (a[k] != b[k]) return k;
    return n;
}

int check(const std::string& path) {
    const auto t0 = std::chrono::steady_clock::now();
    const auto data = loadBytes(path);
    const ptf::Graph g = ptf::readPtf(data);
    const auto direct = ptf::writePtf(g);
    const std::string json = ptf::toJson(g);
    const auto viaJson = ptf::writePtf(ptf::fromJson(json));
    const double ms = std::chrono::duration<double, std::milli>(std::chrono::steady_clock::now() - t0).count();
    std::map<std::string, int> counts;
    for (const auto& [id, n] : g.nodes) ++counts[n.cls];
    const bool ok1 = direct == data, ok2 = viaJson == data;
    std::printf("%s  %zu bytes, %zu objects, json %zu KB, %.0f ms  %s\n",
                ok1 && ok2 ? "OK  " : "DIFF", data.size(), g.nodes.size(), json.size() / 1024, ms, path.c_str());
    if (!ok1) std::printf("      re-encode differs at byte %zu (sizes %zu vs %zu)\n", firstDiff(data, direct), data.size(), direct.size());
    if (!ok2) std::printf("      JSON round-trip differs at byte %zu\n", firstDiff(data, viaJson));
    return ok1 && ok2 ? 0 : 1;
}

}  // namespace

int main(int argc, char** argv) {
    if (argc < 3) {
        std::fprintf(stderr, "usage:\n  ptfjson read <in.ptf> <out.json>\n  ptfjson write <in.json> <out.ptf>\n  ptfjson check <in.ptf>...\n");
        return 2;
    }
    const std::string cmd = argv[1];
    try {
        if (cmd == "read" && argc == 4) {
            const std::string json = ptf::toJson(ptf::readPtf(loadBytes(argv[2])));
            save(argv[3], json.data(), json.size());
            return 0;
        }
        if (cmd == "write" && argc == 4) {
            const auto bytes = ptf::writePtf(ptf::fromJson(loadText(argv[2])));
            save(argv[3], bytes.data(), bytes.size());
            return 0;
        }
        if (cmd == "check") {
            int bad = 0;
            for (int k = 2; k < argc; ++k) {
                try {
                    bad += check(argv[k]);
                } catch (const std::exception& e) {
                    std::printf("FAIL  %s: %s\n", argv[k], e.what());
                    ++bad;
                }
            }
            if (argc > 3) std::printf("%d of %d files failed\n", bad, argc - 2);
            return bad ? 1 : 0;
        }
        std::fprintf(stderr, "error: unknown command or wrong arguments\n");
        return 2;
    } catch (const std::exception& e) {
        std::fprintf(stderr, "error: %s\n", e.what());
        return 1;
    }
}
