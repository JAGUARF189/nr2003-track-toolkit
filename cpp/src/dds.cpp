#include "dds.hpp"

#include <algorithm>
#include <cstring>
#include <stdexcept>

namespace dds {
namespace {

std::uint32_t u32(const std::uint8_t* p) {
    std::uint32_t v;
    std::memcpy(&v, p, 4);
    return v;
}

constexpr std::uint32_t kAlphaPixels = 0x1, kAlpha = 0x2, kFourCC = 0x4, kRgb = 0x40, kLuminance = 0x20000;

struct Header {
    std::uint32_t width, height, mips, pfFlags, fourCC, bits, mask[4];
    std::size_t dataOffset;
};

Header header(const std::uint8_t* d, std::size_t n) {
    if (n < 128 || std::memcmp(d, "DDS ", 4) != 0) throw std::runtime_error("not a DDS picture");
    Header h{};
    h.height = u32(d + 12);
    h.width = u32(d + 16);
    h.mips = std::max<std::uint32_t>(1, u32(d + 28));
    h.pfFlags = u32(d + 80);
    h.fourCC = u32(d + 84);
    h.bits = u32(d + 88);
    for (int k = 0; k < 4; ++k) h.mask[k] = u32(d + 92 + 4 * k);
    h.dataOffset = 128;
    if ((h.pfFlags & kFourCC) && std::memcmp(d + 84, "DX10", 4) == 0) throw std::runtime_error("DX10 DDS pictures aren't supported yet");
    if (h.width == 0 || h.height == 0 || h.width > 16384 || h.height > 16384) throw std::runtime_error("bad DDS size");
    return h;
}

enum class Fmt { Dxt1, Dxt3, Dxt5, Raw };

Fmt format(const Header& h) {
    if (h.pfFlags & kFourCC) {
        if (h.fourCC == 0x31545844) return Fmt::Dxt1;   // "DXT1"
        if (h.fourCC == 0x33545844) return Fmt::Dxt3;   // "DXT3"
        if (h.fourCC == 0x35545844) return Fmt::Dxt5;   // "DXT5"
        char c[5] = {};
        std::memcpy(c, &h.fourCC, 4);
        throw std::runtime_error(std::string("DDS format ") + c + " isn't supported");
    }
    if (h.bits != 8 && h.bits != 16 && h.bits != 24 && h.bits != 32) throw std::runtime_error("unsupported DDS bit depth");
    return Fmt::Raw;
}

std::size_t levelBytes(const Header& h, Fmt f, std::uint32_t w, std::uint32_t ht) {
    const std::size_t bw = std::max(1u, (w + 3) / 4), bh = std::max(1u, (ht + 3) / 4);
    switch (f) {
        case Fmt::Dxt1: return bw * bh * 8;
        case Fmt::Dxt3:
        case Fmt::Dxt5: return bw * bh * 16;
        case Fmt::Raw: return std::size_t(w) * ht * (h.bits / 8);
    }
    return 0;
}

void rgb565(std::uint16_t c, std::uint8_t out[4]) {
    const int r = (c >> 11) & 31, g = (c >> 5) & 63, b = c & 31;
    out[0] = std::uint8_t((r << 3) | (r >> 2));
    out[1] = std::uint8_t((g << 2) | (g >> 4));
    out[2] = std::uint8_t((b << 3) | (b >> 2));
    out[3] = 255;
}

// One 4x4 colour block; `fourColour` forces the 4-colour mode (DXT3/5).
void colourBlock(const std::uint8_t* b, bool fourColour, std::uint8_t px[16][4]) {
    const std::uint16_t c0 = std::uint16_t(b[0] | (b[1] << 8)), c1 = std::uint16_t(b[2] | (b[3] << 8));
    std::uint8_t pal[4][4];
    rgb565(c0, pal[0]);
    rgb565(c1, pal[1]);
    if (fourColour || c0 > c1) {
        for (int k = 0; k < 3; ++k) {
            pal[2][k] = std::uint8_t((2 * pal[0][k] + pal[1][k]) / 3);
            pal[3][k] = std::uint8_t((pal[0][k] + 2 * pal[1][k]) / 3);
        }
        pal[2][3] = pal[3][3] = 255;
    } else {
        for (int k = 0; k < 3; ++k) pal[2][k] = std::uint8_t((pal[0][k] + pal[1][k]) / 2);
        pal[2][3] = 255;
        pal[3][0] = pal[3][1] = pal[3][2] = pal[3][3] = 0;   // transparent
    }
    const std::uint32_t idx = u32(b + 4);
    for (int i = 0; i < 16; ++i) std::memcpy(px[i], pal[(idx >> (2 * i)) & 3], 4);
}

void decodeBlocks(const std::uint8_t* src, Fmt f, std::uint32_t w, std::uint32_t h, std::vector<std::uint8_t>& out) {
    out.assign(std::size_t(w) * h * 4, 0);
    const std::uint32_t bw = std::max(1u, (w + 3) / 4), bh = std::max(1u, (h + 3) / 4);
    const std::size_t step = f == Fmt::Dxt1 ? 8 : 16;
    for (std::uint32_t by = 0; by < bh; ++by) {
        for (std::uint32_t bx = 0; bx < bw; ++bx) {
            const std::uint8_t* b = src + (std::size_t(by) * bw + bx) * step;
            std::uint8_t px[16][4];
            if (f == Fmt::Dxt1) {
                colourBlock(b, false, px);
            } else {
                colourBlock(b + 8, true, px);
                if (f == Fmt::Dxt3) {
                    for (int i = 0; i < 16; ++i) {
                        const int a = (b[i / 2] >> (4 * (i & 1))) & 15;
                        px[i][3] = std::uint8_t(a * 17);
                    }
                } else {
                    const int a0 = b[0], a1 = b[1];
                    int pal[8] = {a0, a1};
                    if (a0 > a1) {
                        for (int k = 1; k < 7; ++k) pal[k + 1] = ((7 - k) * a0 + k * a1) / 7;
                    } else {
                        for (int k = 1; k < 5; ++k) pal[k + 1] = ((5 - k) * a0 + k * a1) / 5;
                        pal[6] = 0;
                        pal[7] = 255;
                    }
                    std::uint64_t bits = 0;
                    for (int k = 0; k < 6; ++k) bits |= std::uint64_t(b[2 + k]) << (8 * k);
                    for (int i = 0; i < 16; ++i) px[i][3] = std::uint8_t(pal[(bits >> (3 * i)) & 7]);
                }
            }
            for (int i = 0; i < 16; ++i) {
                const std::uint32_t x = bx * 4 + (i & 3), y = by * 4 + (i >> 2);
                if (x < w && y < h) std::memcpy(&out[(std::size_t(y) * w + x) * 4], px[i], 4);
            }
        }
    }
}

// Value of `v` under `mask`, scaled to 0..255.
std::uint8_t channel(std::uint32_t v, std::uint32_t mask) {
    if (!mask) return 0;
    int shift = 0;
    while (!((mask >> shift) & 1)) ++shift;
    const std::uint32_t max = mask >> shift;
    return std::uint8_t(((v & mask) >> shift) * 255 / max);
}

void decodeRaw(const std::uint8_t* src, const Header& h, std::uint32_t w, std::uint32_t ht, std::vector<std::uint8_t>& out) {
    out.assign(std::size_t(w) * ht * 4, 0);
    const std::size_t bpp = h.bits / 8;
    for (std::size_t i = 0; i < std::size_t(w) * ht; ++i) {
        std::uint32_t v = 0;
        std::memcpy(&v, src + i * bpp, bpp);
        std::uint8_t* o = &out[i * 4];
        if (h.pfFlags & kLuminance) {
            o[0] = o[1] = o[2] = channel(v, h.mask[0] ? h.mask[0] : 0xFF);
            o[3] = (h.pfFlags & kAlphaPixels) ? channel(v, h.mask[3]) : 255;
        } else if ((h.pfFlags & kAlpha) && !(h.pfFlags & kRgb)) {
            o[0] = o[1] = o[2] = 255;
            o[3] = channel(v, h.mask[3] ? h.mask[3] : 0xFF);
        } else {
            o[0] = channel(v, h.mask[0]);
            o[1] = channel(v, h.mask[1]);
            o[2] = channel(v, h.mask[2]);
            o[3] = (h.pfFlags & kAlphaPixels) && h.mask[3] ? channel(v, h.mask[3]) : 255;
        }
    }
}

// Box-filter halving until the longer side fits.
void shrink(Image& im, std::uint32_t maxSide) {
    while (std::max(im.width, im.height) > maxSide && im.width > 1 && im.height > 1) {
        const std::uint32_t w = im.width / 2, h = im.height / 2;
        std::vector<std::uint8_t> o(std::size_t(w) * h * 4);
        for (std::uint32_t y = 0; y < h; ++y)
            for (std::uint32_t x = 0; x < w; ++x)
                for (int c = 0; c < 4; ++c) {
                    const auto at = [&](std::uint32_t xx, std::uint32_t yy) { return im.rgba[(std::size_t(yy) * im.width + xx) * 4 + c]; };
                    o[(std::size_t(y) * w + x) * 4 + c] =
                        std::uint8_t((at(2 * x, 2 * y) + at(2 * x + 1, 2 * y) + at(2 * x, 2 * y + 1) + at(2 * x + 1, 2 * y + 1) + 2) / 4);
                }
        im.width = w;
        im.height = h;
        im.rgba.swap(o);
    }
}

}  // namespace

std::string formatName(const std::uint8_t* data, std::size_t size) {
    try {
        const Header h = header(data, size);
        if (h.pfFlags & kFourCC) {
            char c[5] = {};
            std::memcpy(c, &h.fourCC, 4);
            return c;
        }
        return (h.pfFlags & kLuminance ? "L" : (h.pfFlags & kAlphaPixels) ? "RGBA" : "RGB") + std::to_string(h.bits);
    } catch (const std::exception&) {
        return "unknown";
    }
}

Image decode(const std::uint8_t* data, std::size_t size, std::uint32_t maxSide) {
    const Header h = header(data, size);
    const Fmt f = format(h);
    // Walk the mip chain to the first level that fits (or the last one stored).
    std::size_t off = h.dataOffset;
    std::uint32_t w = h.width, ht = h.height;
    for (std::uint32_t level = 0; level + 1 < h.mips && std::max(w, ht) > maxSide; ++level) {
        off += levelBytes(h, f, w, ht);
        w = std::max(1u, w / 2);
        ht = std::max(1u, ht / 2);
    }
    const std::size_t need = levelBytes(h, f, w, ht);
    if (off + need > size) throw std::runtime_error("DDS picture is cut short");
    Image im;
    im.width = w;
    im.height = ht;
    if (f == Fmt::Raw)
        decodeRaw(data + off, h, w, ht, im.rgba);
    else
        decodeBlocks(data + off, f, w, ht, im.rgba);
    shrink(im, maxSide);
    return im;
}

}  // namespace dds
