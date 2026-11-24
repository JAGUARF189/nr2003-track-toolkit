// Assetto Corsa .kn5 models (tracks and their scenery).
//
// Layout (little endian; strings are int32 length + bytes):
//   "sc6969", int32 version (5 or 6; version > 5 adds one int32)
//   int32 n, n x texture { int32 active, string name, int32 size, size bytes (a .dds file) }
//   int32 n, n x material { string name, string shader, u8 blend, u8 alphaTested, int32 depthMode,
//                           int32 n, n x { string name, float value, float[9] } ,
//                           int32 n, n x sampler { string slot, int32 index, string texture } }
//   node (recursive): int32 class, string name, int32 children, u8 active, then
//     class 1 (transform): float[16] row-major matrix (D3D row vectors: v' = v * M)
//     class 2 (mesh): u8 castShadows, u8 visible, u8 transparent,
//                     int32 n, n x { float pos[3], normal[3], uv[2], tangent[3] },
//                     int32 n, n x uint16 index, int32 material, int32 layer,
//                     float lodIn, lodOut, float sphereCentre[3], radius, u8 renderable
//     class 3 (skinned mesh): u8 x3, int32 bones, bones x { string, float[16] },
//                     int32 n, n x 76-byte vertex (as above + 4 weights + 4 bone indices),
//                     int32 n, n x uint16, int32 material, int32 layer, float lodIn, lodOut
//   then `children` nodes.
// Checked to the last byte on mugello (Kunos, version 6) and monaco_2020 (a mod, version 5).
#pragma once

#include <array>
#include <cstdint>
#include <string>
#include <vector>

namespace kn5 {

using Mat4 = std::array<float, 16>;

struct Texture {
    std::string name;
    std::vector<std::uint8_t> dds;   // the .dds file
};

struct Material {
    std::string name, shader;
    int blend = 0;          // 0 opaque, 1 alpha blend, 2 alpha to coverage
    bool alphaTested = false;
    std::vector<std::pair<std::string, std::string>> samplers;   // (slot, texture name)
    std::string diffuse() const;                                  // txDiffuse texture, or ""
};

// A mesh with its vertices already placed in the world (AC coordinates: metres, y up).
struct Mesh {
    std::string name;
    int material = -1;
    bool visible = true, castShadows = true, transparent = false, renderable = true, skinned = false;
    float lodIn = 0, lodOut = 0;
    std::vector<float> pos, normal, uv;   // 3, 3 and 2 floats per vertex
    std::vector<std::uint32_t> index;     // triangles
};

// A named empty (AC_START_0, AC_PIT_0, ...): its world transform.
struct Dummy {
    std::string name;
    Mat4 world;
};

struct Model {
    int version = 0;
    std::vector<Texture> textures;
    std::vector<Material> materials;
    std::vector<Mesh> meshes;
    std::vector<Dummy> dummies;
};

struct Options {
    bool textures = true;    // keep the .dds bytes
    bool geometry = true;    // keep vertices (false: names and counts only)
    bool farLods = false;    // keep meshes only shown from far away (lodIn > 0)
};

// Read a .kn5 file (models.ini POSITION offsets are applied by the caller). Throws std::runtime_error with a readable message.
Model read(const std::string& path, const Options& opt = {});

// Transform helpers (row vectors).
Mat4 identity();
Mat4 multiply(const Mat4& a, const Mat4& b);   // a then b
void transformPoint(const Mat4& m, const float in[3], float out[3]);
void transformDir(const Mat4& m, const float in[3], float out[3]);

}  // namespace kn5
