#include "kn5.hpp"

#include <cmath>
#include <cstring>
#include <fstream>
#include <stdexcept>

namespace kn5 {
namespace {

class Reader {
public:
    explicit Reader(const std::string& path) : path_(path) {
        std::ifstream f(path, std::ios::binary | std::ios::ate);
        if (!f) throw std::runtime_error("can't open " + path);
        const auto n = static_cast<std::size_t>(f.tellg());
        data_.resize(n);
        f.seekg(0);
        if (n && !f.read(reinterpret_cast<char*>(data_.data()), static_cast<std::streamsize>(n)))
            throw std::runtime_error("can't read " + path);
    }
    void need(std::size_t n) const {
        if (pos_ + n > data_.size()) throw std::runtime_error(path_ + ": the file ends early (at byte " + std::to_string(pos_) + ")");
    }
    template <class T> T get() {
        need(sizeof(T));
        T v;
        std::memcpy(&v, data_.data() + pos_, sizeof(T));
        pos_ += sizeof(T);
        return v;
    }
    std::int32_t i32() { return get<std::int32_t>(); }
    std::uint8_t u8() { return get<std::uint8_t>(); }
    float f32() { return get<float>(); }
    std::int32_t count(std::size_t each) {
        const std::int32_t n = i32();
        if (n < 0 || std::size_t(n) * each > data_.size() - pos_) throw std::runtime_error(path_ + ": bad count " + std::to_string(n) + " at byte " + std::to_string(pos_));
        return n;
    }
    std::string str() {
        const std::int32_t n = count(1);
        std::string s(reinterpret_cast<const char*>(data_.data() + pos_), std::size_t(n));
        pos_ += std::size_t(n);
        return s;
    }
    const std::uint8_t* take(std::size_t n) {
        need(n);
        const std::uint8_t* p = data_.data() + pos_;
        pos_ += n;
        return p;
    }
    void skip(std::size_t n) { take(n); }
    std::size_t pos() const { return pos_; }
    std::size_t size() const { return data_.size(); }

private:
    std::string path_;
    std::vector<std::uint8_t> data_;
    std::size_t pos_ = 0;
};

void readMesh(Reader& r, Mesh& m, const Mat4& world, bool skinned, const Options& opt) {
    m.skinned = skinned;
    m.castShadows = r.u8() != 0;
    m.visible = r.u8() != 0;
    m.transparent = r.u8() != 0;
    if (skinned) {
        const auto bones = r.count(4);
        for (std::int32_t b = 0; b < bones; ++b) {
            r.str();
            r.skip(64);
        }
    }
    const std::size_t stride = skinned ? 76 : 44;
    const auto nv = r.count(stride);
    const std::uint8_t* v = r.take(std::size_t(nv) * stride);
    const auto ni = r.count(2);
    const std::uint8_t* ix = r.take(std::size_t(ni) * 2);
    m.material = r.i32();
    r.i32();   // layer
    m.lodIn = r.f32();
    m.lodOut = r.f32();
    if (!skinned) {
        r.skip(16);   // bounding sphere
        m.renderable = r.u8() != 0;
    }
    if (!opt.geometry) return;
    m.pos.resize(std::size_t(nv) * 3);
    m.normal.resize(std::size_t(nv) * 3);
    m.uv.resize(std::size_t(nv) * 2);
    for (std::int32_t k = 0; k < nv; ++k) {
        float p[8];
        std::memcpy(p, v + std::size_t(k) * stride, sizeof p);
        transformPoint(world, p, &m.pos[std::size_t(k) * 3]);
        float n[3];
        transformDir(world, p + 3, n);
        const float len = std::sqrt(n[0] * n[0] + n[1] * n[1] + n[2] * n[2]);
        for (int c = 0; c < 3; ++c) m.normal[std::size_t(k) * 3 + c] = len > 1e-12f ? n[c] / len : 0.0f;
        m.uv[std::size_t(k) * 2] = p[6];
        m.uv[std::size_t(k) * 2 + 1] = p[7];
    }
    m.index.resize(std::size_t(ni));
    for (std::int32_t k = 0; k < ni; ++k) {
        std::uint16_t i;
        std::memcpy(&i, ix + std::size_t(k) * 2, 2);
        if (i >= nv) throw std::runtime_error("mesh " + m.name + " has a bad triangle");
        m.index[std::size_t(k)] = i;
    }
}

void readNode(Reader& r, Model& model, const Mat4& parent, const Options& opt, int depth) {
    if (depth > 256) throw std::runtime_error("model nodes nest too deep");
    const std::int32_t cls = r.i32();
    std::string name = r.str();
    const std::int32_t children = r.count(1);
    r.u8();   // active
    Mat4 world = parent;
    switch (cls) {
        case 1: {
            Mat4 local;
            for (float& x : local) x = r.f32();
            world = multiply(local, parent);
            model.dummies.push_back({name, world});
            break;
        }
        case 2:
        case 3: {
            Mesh m;
            m.name = std::move(name);
            readMesh(r, m, parent, cls == 3, opt);
            if (opt.farLods || m.lodIn <= 0.0f) model.meshes.push_back(std::move(m));
            break;
        }
        default:
            throw std::runtime_error("unknown model node type " + std::to_string(cls) + " at byte " + std::to_string(r.pos()));
    }
    for (std::int32_t c = 0; c < children; ++c) readNode(r, model, world, opt, depth + 1);
}

}  // namespace

std::string Material::diffuse() const {
    for (const auto& [slot, tex] : samplers)
        if (slot == "txDiffuse") return tex;
    return {};
}

Mat4 identity() { return {1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1}; }

Mat4 multiply(const Mat4& a, const Mat4& b) {
    Mat4 r{};
    for (int i = 0; i < 4; ++i)
        for (int j = 0; j < 4; ++j) {
            float s = 0;
            for (int k = 0; k < 4; ++k) s += a[i * 4 + k] * b[k * 4 + j];
            r[i * 4 + j] = s;
        }
    return r;
}

void transformPoint(const Mat4& m, const float in[3], float out[3]) {
    for (int j = 0; j < 3; ++j) out[j] = in[0] * m[0 * 4 + j] + in[1] * m[1 * 4 + j] + in[2] * m[2 * 4 + j] + m[3 * 4 + j];
}

void transformDir(const Mat4& m, const float in[3], float out[3]) {
    for (int j = 0; j < 3; ++j) out[j] = in[0] * m[0 * 4 + j] + in[1] * m[1 * 4 + j] + in[2] * m[2 * 4 + j];
}

Model read(const std::string& path, const Options& opt) {
    Reader r(path);
    const std::uint8_t* magic = r.take(6);
    if (std::memcmp(magic, "sc6969", 6) != 0) throw std::runtime_error(path + " isn't an Assetto Corsa model (.kn5), or it is encrypted");
    Model m;
    m.version = r.i32();
    if (m.version < 5 || m.version > 6) throw std::runtime_error(path + ": model version " + std::to_string(m.version) + " isn't supported");
    if (m.version > 5) r.i32();
    const auto nt = r.count(12);
    m.textures.reserve(std::size_t(nt));
    for (std::int32_t k = 0; k < nt; ++k) {
        Texture t;
        r.i32();   // active
        t.name = r.str();
        const auto size = r.count(1);
        const std::uint8_t* d = r.take(std::size_t(size));
        if (opt.textures) t.dds.assign(d, d + size);
        m.textures.push_back(std::move(t));
    }
    const auto nm = r.count(14);
    m.materials.reserve(std::size_t(nm));
    for (std::int32_t k = 0; k < nm; ++k) {
        Material mat;
        mat.name = r.str();
        mat.shader = r.str();
        mat.blend = r.u8();
        mat.alphaTested = r.u8() != 0;
        r.i32();   // depth mode
        const auto np = r.count(44);
        for (std::int32_t p = 0; p < np; ++p) {
            r.str();
            r.skip(40);
        }
        const auto ns = r.count(12);
        for (std::int32_t s = 0; s < ns; ++s) {
            std::string slot = r.str();
            r.i32();
            mat.samplers.emplace_back(std::move(slot), r.str());
        }
        m.materials.push_back(std::move(mat));
    }
    readNode(r, m, identity(), opt, 0);
    return m;
}

}  // namespace kn5
