// BC7 (BPTC) texture blocks: 16 bytes for 4x4 RGBA texels.
#pragma once

#include <cstdint>

namespace bc7 {

// Decode one block into 16 RGBA texels (row by row).
void decodeBlock(const std::uint8_t* block, std::uint8_t out[16][4]);

// Self-check of the partition and anchor tables (anchors lie in their subset).
bool tablesConsistent();

}  // namespace bc7
