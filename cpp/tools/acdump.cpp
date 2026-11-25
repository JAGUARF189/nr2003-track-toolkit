// acdump: read an Assetto Corsa track folder for the Track Toolkit.
//
//   acdump info  <track folder>                         JSON: layouts, models, surfaces, AI line
//   acdump scene <track folder> <layout|-> <out> [--tex N] [--no-tex] [--tex-dir <folder>]
//
// --tex-dir: each texture goes to <folder>\<index>.rgba (u32 w, u32 h, RGBA) or <index>.enc (a
// PNG/JPEG as stored), and the scene keeps only its name (w = h = 0, len 0). Big tracks'
// textures then never sit in memory together.
//
// `scene` writes everything the 3D view needs to <out> (little endian; str = u32 length + bytes;
// positions converted to the Track Toolkit's axes: x, y = -AC z, z = AC y, metres):
//   "ACSCENE2"
//   u32 n, n x texture  { str name, u32 w, u32 h, w*h*4 RGBA bytes; or w = h = 0 and
//                         u32 len + len bytes of a PNG/JPEG for the app to decode (len 0: unreadable) }
//   u32 n, n x material { str name, str shader, i32 texture (-1 none), u8 alphaTested, u8 blend,
//                         i32 detail texture (-1 none, -2 multi-layer with no usable detail; else the
//                         main surface's picture), f32 its repeat }
//   u32 n, n x surface  { str key, u8 validTrack, u8 pitlane }
//   u32 n, n x mesh     { str name, str model file, i32 material, i32 surface (-1 = scenery),
//                         u8 visible, u8 transparent, u32 nv, nv x float[8] (pos, normal, uv),
//                         u32 ni, ni x u32 }
//   u32 n, n x marker   { str name, float pos[3], float forward[3] }      (AC_* empties)
//   u32 n, n x ai point { float pos[3], float distance, float left, float right }
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <exception>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <stdexcept>
#include <map>
#include <string>
#include <vector>

#include "actrack.hpp"
#include "dds.hpp"
#include "kn5.hpp"

namespace {

std::string esc(const std::string& s) {
    std::string o;
    for (const unsigned char c : s) {
        if (c == '"' || c == '\\') {
            o += '\\';
            o += char(c);
        } else if (c < 0x20) {
            char b[8];
            std::snprintf(b, sizeof b, "\\u%04x", c);
            o += b;
        } else if (c >= 0x80) {
            // Latin-1 byte -> UTF-8
            o += char(0xC0 | (c >> 6));
            o += char(0x80 | (c & 0x3F));
        } else {
            o += char(c);
        }
    }
    return o;
}

// AC (y up) to Track Toolkit (z up), keeping it right-handed.
void toolkit(const float in[3], float out[3]) {
    out[0] = in[0];
    out[1] = -in[2];
    out[2] = in[1];
}

class Out {
public:
    explicit Out(const std::string& path) : f_(std::fopen(path.c_str(), "wb")) {
        if (!f_) throw std::runtime_error("can't write " + path);
    }
    ~Out() {
        if (f_) std::fclose(f_);
    }
    void raw(const void* p, std::size_t n) {
        if (n && std::fwrite(p, 1, n, f_) != n) throw std::runtime_error("writing the scene failed (disk full?)");
    }
    void u32(std::uint32_t v) { raw(&v, 4); }
    void i32(std::int32_t v) { raw(&v, 4); }
    void u8(std::uint8_t v) { raw(&v, 1); }
    void f32(float v) { raw(&v, 4); }
    void str(const std::string& s) {
        u32(std::uint32_t(s.size()));
        raw(s.data(), s.size());
    }
    void close() {
        if (std::fclose(f_) != 0) throw std::runtime_error("writing the scene failed");
        f_ = nullptr;
    }

private:
    std::FILE* f_;
};

int info(const std::string& dir) {
    const ac::TrackFolder t = ac::scan(dir);
    std::cout << "{\"name\":\"" << esc(t.name) << "\",\"title\":\"" << esc(t.title) << "\",\"layouts\":[";
    for (std::size_t i = 0; i < t.layouts.size(); ++i) {
        const auto& l = t.layouts[i];
        const auto surf = ac::surfaces(l);
        const auto ai = ac::aiLine(l);
        std::cout << (i ? "," : "") << "{\"name\":\"" << esc(l.name) << "\",\"title\":\"" << esc(l.title) << "\",\"models\":[";
        for (std::size_t k = 0; k < l.models.size(); ++k)
            std::cout << (k ? "," : "") << "{\"file\":\"" << esc(l.models[k].file) << "\",\"exists\":" << (l.models[k].exists ? "true" : "false") << "}";
        std::cout << "],\"surfaces\":[";
        for (std::size_t k = 0; k < surf.size(); ++k)
            std::cout << (k ? "," : "") << "{\"key\":\"" << esc(surf[k].key) << "\",\"valid\":" << (surf[k].validTrack ? "true" : "false")
                      << ",\"pitlane\":" << (surf[k].pitlane ? "true" : "false") << "}";
        std::cout << "],\"ai_points\":" << ai.size() << ",\"lap_m\":" << (ai.empty() ? 0.0f : ai.back().distance) << "}";
    }
    std::cout << "]}\n";
    return 0;
}

void writeTexFile(const std::string& texDir, int index, const dds::Image& img, const std::vector<std::uint8_t>& encoded) {
    const std::filesystem::path p = std::filesystem::path(texDir) / (std::to_string(index) + (encoded.empty() ? ".rgba" : ".enc"));
    std::ofstream f(p, std::ios::binary);
    if (!f) throw std::runtime_error("can't write " + p.string());
    if (encoded.empty()) {
        const std::uint32_t wh[2] = {img.width, img.height};
        f.write(reinterpret_cast<const char*>(wh), 8);
        f.write(reinterpret_cast<const char*>(img.rgba.data()), std::streamsize(img.rgba.size()));
    } else {
        f.write(reinterpret_cast<const char*>(encoded.data()), std::streamsize(encoded.size()));
    }
}

int scene(const std::string& dir, const std::string& layoutName, const std::string& outPath, std::uint32_t texSide, const std::string& texDir) {
    const ac::TrackFolder t = ac::scan(dir);
    const ac::Layout* layout = nullptr;
    for (const auto& l : t.layouts)
        if (l.name == layoutName || (layoutName == "-" && l.name.empty())) layout = &l;
    if (!layout) layout = &t.layouts.front();
    const auto surf = ac::surfaces(*layout);

    struct Tex {
        std::string name;
        dds::Image img;
        std::vector<std::uint8_t> encoded;   // a PNG/JPEG stored in the model, decoded by the app
    };
    std::vector<Tex> textures;
    std::map<std::string, int> texIndex;   // lower-case name -> index
    struct Mat {
        std::string name, shader;
        int tex;
        bool alpha;
        int blend;
        int detail;   // multi-layer materials: the main detail texture (-1 none)
        float mult;   // and how many times it repeats per texture unit
    };
    std::vector<Mat> materials;
    std::vector<std::pair<std::string, kn5::Mesh>> meshes;   // (model file, mesh)
    std::vector<kn5::Dummy> markers;
    std::size_t failed = 0;

    for (const auto& ref : layout->models) {
        if (!ref.exists) {
            std::cerr << "warning: " << ref.file << " is missing\n";
            continue;
        }
        if (ref.rotation[0] != 0 || ref.rotation[1] != 0 || ref.rotation[2] != 0)
            std::cerr << "warning: " << ref.file << " has a ROTATION, which is ignored for now\n";
        kn5::Options opt;
        opt.textures = texSide > 0;
        kn5::Model m = kn5::read(ref.path, opt);
        // textures: the ones materials use as their picture, decoded once per name
        std::map<std::string, const kn5::Texture*> byName;
        for (const auto& tx : m.textures) byName.emplace(tx.name, &tx);
        const int matBase = int(materials.size());
        // index of a texture (by name) in the scene's list, decoding it the first time
        const auto texFor = [&](const std::string& d) -> int {
            if (d.empty() || texSide == 0) return -1;
            std::string key = d;
            for (char& c : key) c = char(std::tolower(static_cast<unsigned char>(c)));
            if (const auto it = texIndex.find(key); it != texIndex.end()) return it->second;
            const auto tx = byName.find(d);
            if (tx == byName.end()) return -1;
            Tex nt{d, {}, {}};
            const auto& bytes = tx->second->dds;
            if (bytes.size() >= 4 && std::memcmp(bytes.data(), "DDS ", 4) != 0) {
                nt.encoded = bytes;   // PNG/JPEG: the app's image library reads those
            } else {
                try {
                    nt.img = dds::decode(bytes.data(), bytes.size(), texSide);
                } catch (const std::exception& e) {
                    ++failed;
                    std::cerr << "warning: texture " << d << ": " << e.what() << "\n";
                }
            }
            const int ti = int(textures.size());
            texIndex[key] = ti;
            if (!texDir.empty()) {
                if (nt.img.width || !nt.encoded.empty()) writeTexFile(texDir, ti, nt.img, nt.encoded);
                nt.img = {};
                nt.encoded.clear();
            }
            textures.push_back(std::move(nt));
            return ti;
        };
        for (const auto& mat : m.materials) {
            const int ti = texFor(mat.diffuse());
            // Multi-layer materials (ksMultilayer...): txDiffuse is a blurry overall colour and
            // txMask says where each of up to four detail textures shows. The one whose mask
            // channel covers most is the surface's own picture, tiled by its multiplier.
            int detail = -1;
            float mult = 1;
            const std::string mask = mat.sampler("txMask");
            if (!mask.empty() && !mat.sampler("txDetailR").empty() && texSide > 0) {
                double sum[4] = {0, 0, 0, 0};
                if (const auto mt = byName.find(mask); mt != byName.end() && mt->second->dds.size() >= 4 &&
                                                       std::memcmp(mt->second->dds.data(), "DDS ", 4) == 0) {
                    try {
                        const dds::Image im = dds::decode(mt->second->dds.data(), mt->second->dds.size(), 32);
                        for (std::size_t p = 0; p + 3 < im.rgba.size(); p += 4)
                            for (int c = 0; c < 4; ++c) sum[c] += im.rgba[p + c];
                    } catch (const std::exception&) {
                    }
                }
                // only real detail textures: slots holding a placeholder (nullb.dds) or repeating 0
                // times are unused
                const char* ch[4] = {"R", "G", "B", "A"};
                const auto usable = [&](int c) {
                    std::string t = mat.sampler(std::string("txDetail") + ch[c]);
                    for (char& x : t) x = char(std::tolower(static_cast<unsigned char>(x)));
                    return !t.empty() && t.rfind("null", 0) != 0 && mat.prop(std::string("mult") + ch[c], 1.0f) > 0.0f;
                };
                int best = -1;
                for (int c = 0; c < 4; ++c)
                    if (usable(c) && (best < 0 || sum[c] > sum[best])) best = c;
                if (best >= 0) {
                    detail = texFor(mat.sampler(std::string("txDetail") + ch[best]));
                    mult = mat.prop(std::string("mult") + ch[best], 1.0f);
                } else {
                    detail = -2;   // multi-layer, but no usable detail: only a blurry colour map
                }
            }
            materials.push_back({mat.name, mat.shader, ti, mat.alphaTested, mat.blend, detail, mult});
        }
        for (auto& mesh : m.meshes) {
            for (std::size_t k = 0; k < mesh.pos.size(); k += 3)
                for (int c = 0; c < 3; ++c) mesh.pos[k + c] += ref.position[c];
            if (mesh.material >= 0) mesh.material += matBase;
            meshes.emplace_back(ref.file, std::move(mesh));
        }
        for (auto& d : m.dummies)
            if (d.name.rfind("AC_", 0) == 0) {
                for (int c = 0; c < 3; ++c) d.world[12 + c] += ref.position[c];
                markers.push_back(d);
            }
    }

    Out o(outPath);
    o.raw("ACSCENE2", 8);
    o.u32(std::uint32_t(textures.size()));
    for (const auto& tx : textures) {
        o.str(tx.name);
        o.u32(tx.img.width);
        o.u32(tx.img.height);
        if (tx.img.width && tx.img.height) {
            o.raw(tx.img.rgba.data(), tx.img.rgba.size());
        } else {
            o.u32(std::uint32_t(tx.encoded.size()));
            o.raw(tx.encoded.data(), tx.encoded.size());
        }
    }
    o.u32(std::uint32_t(materials.size()));
    for (const auto& m : materials) {
        o.str(m.name);
        o.str(m.shader);
        o.i32(m.tex);
        o.u8(m.alpha);
        o.u8(std::uint8_t(m.blend));
        o.i32(m.detail);
        o.f32(m.mult);
    }
    o.u32(std::uint32_t(surf.size()));
    for (const auto& s : surf) {
        o.str(s.key);
        o.u8(s.validTrack);
        o.u8(s.pitlane);
    }
    o.u32(std::uint32_t(meshes.size()));
    std::size_t tris = 0;
    for (const auto& [file, m] : meshes) {
        o.str(m.name);
        o.str(file);
        o.i32(m.material);
        o.i32(ac::surfaceOf(m.name, surf));
        o.u8(m.visible);
        o.u8(m.transparent);
        const std::size_t nv = m.pos.size() / 3;
        o.u32(std::uint32_t(nv));
        std::vector<float> v(nv * 8);
        for (std::size_t k = 0; k < nv; ++k) {
            toolkit(&m.pos[k * 3], &v[k * 8]);
            toolkit(&m.normal[k * 3], &v[k * 8 + 3]);
            v[k * 8 + 6] = m.uv[k * 2];
            v[k * 8 + 7] = m.uv[k * 2 + 1];
        }
        o.raw(v.data(), v.size() * 4);
        o.u32(std::uint32_t(m.index.size()));
        o.raw(m.index.data(), m.index.size() * 4);
        tris += m.index.size() / 3;
    }
    o.u32(std::uint32_t(markers.size()));
    for (const auto& d : markers) {
        o.str(d.name);
        float p[3] = {d.world[12], d.world[13], d.world[14]}, q[3], f[3] = {d.world[8], d.world[9], d.world[10]}, g[3];
        toolkit(p, q);
        toolkit(f, g);
        for (float x : q) o.f32(x);
        for (float x : g) o.f32(x);
    }
    const auto ai = ac::aiLine(*layout);
    o.u32(std::uint32_t(ai.size()));
    for (const auto& a : ai) {
        float q[3];
        toolkit(a.pos, q);
        for (float x : q) o.f32(x);
        o.f32(a.distance);
        o.f32(a.left);
        o.f32(a.right);
    }
    o.close();
    std::cerr << meshes.size() << " meshes, " << tris << " triangles, " << textures.size() << " textures (" << failed << " unreadable), "
              << markers.size() << " markers, " << ai.size() << " AI points\n";
    return 0;
}

}  // namespace

int main(int argc, char** argv) {
    try {
        const std::vector<std::string> a(argv + 1, argv + argc);
        if (a.size() >= 2 && a[0] == "info") return info(a[1]);
        if (a.size() >= 4 && a[0] == "scene") {
            std::uint32_t side = 256;
            std::string texDir;
            for (std::size_t k = 4; k < a.size(); ++k) {
                if (a[k] == "--no-tex") side = 0;
                if (a[k] == "--tex" && k + 1 < a.size()) side = std::uint32_t(std::stoul(a[++k]));
                if (a[k] == "--tex-dir" && k + 1 < a.size()) texDir = a[++k];
            }
            if (!texDir.empty()) std::filesystem::create_directories(texDir);
            return scene(a[1], a[2], a[3], side, texDir);
        }
        std::cerr << "usage: acdump info <track folder>\n       acdump scene <track folder> <layout|-> <out> [--tex N] [--no-tex] [--tex-dir <folder>]\n";
        return 2;
    } catch (const std::exception& e) {
        std::cerr << "error: " << e.what() << "\n";
        return 1;
    }
}
