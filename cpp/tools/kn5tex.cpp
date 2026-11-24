// kn5tex: decode the textures of a .kn5 (for checking the DDS and BC7 decoders).
//   kn5tex <model.kn5> <out folder> [name filter] [max side]
// Writes <name>.rgba (u32 width, u32 height, RGBA bytes) for each texture whose name contains
// the filter, and prints each texture's format.
#include <cstdio>
#include <exception>
#include <filesystem>
#include <iostream>
#include <string>

#include "bc7.hpp"
#include "dds.hpp"
#include "kn5.hpp"

int main(int argc, char** argv) {
    try {
        if (argc < 3) {
            std::cerr << "usage: kn5tex <model.kn5> <out folder> [name filter] [max side]\n";
            return 2;
        }
        std::cout << "BC7 tables consistent: " << (bc7::tablesConsistent() ? "yes" : "NO") << "\n";
        const std::string filter = argc > 3 ? argv[3] : "";
        const std::uint32_t side = argc > 4 ? std::uint32_t(std::stoul(argv[4])) : 512;
        kn5::Options opt;
        opt.geometry = false;
        const kn5::Model m = kn5::read(argv[1], opt);
        std::filesystem::create_directories(argv[2]);
        for (const auto& t : m.textures) {
            if (!filter.empty() && t.name.find(filter) == std::string::npos) continue;
            const std::string fmt = dds::formatName(t.dds.data(), t.dds.size());
            try {
                const dds::Image im = dds::decode(t.dds.data(), t.dds.size(), side);
                const std::string out = (std::filesystem::path(argv[2]) / (t.name + ".rgba")).string();
                std::FILE* f = std::fopen(out.c_str(), "wb");
                if (!f) throw std::runtime_error("can't write " + out);
                std::fwrite(&im.width, 4, 1, f);
                std::fwrite(&im.height, 4, 1, f);
                std::fwrite(im.rgba.data(), 1, im.rgba.size(), f);
                std::fclose(f);
                std::cout << t.name << "  " << fmt << "  " << im.width << "x" << im.height << "\n";
            } catch (const std::exception& e) {
                std::cout << t.name << "  " << fmt << "  FAILED: " << e.what() << "\n";
            }
        }
        return 0;
    } catch (const std::exception& e) {
        std::cerr << "error: " << e.what() << "\n";
        return 1;
    }
}
