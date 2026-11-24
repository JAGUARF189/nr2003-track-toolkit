// DirectDraw Surface (.dds) pictures, as used by Assetto Corsa: decode to RGBA.
// Supports DXT1, DXT3, DXT5 (BC1-3) and uncompressed RGB/RGBA/luminance/alpha formats,
// which is everything the checked tracks use (Mugello: Kunos, Monaco 2020: a mod).
#pragma once

#include <cstdint>
#include <string>
#include <vector>

namespace dds {

struct Image {
    std::uint32_t width = 0, height = 0;
    std::vector<std::uint8_t> rgba;   // width * height * 4, top row first
};

// Decode the stored size closest to `maxSide` on the longer side (a mip level no bigger than
// it if the file has one, else the smallest level, box-filtered down to fit). Throws
// std::runtime_error for formats it can't read.
Image decode(const std::uint8_t* data, std::size_t size, std::uint32_t maxSide);

// "DXT1", "RGBA32", ... for messages.
std::string formatName(const std::uint8_t* data, std::size_t size);

}  // namespace dds
