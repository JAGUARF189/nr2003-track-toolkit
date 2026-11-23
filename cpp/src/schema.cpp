// Symmetric reader/writer for every .ptf class. Each transfer routine mirrors the
// sandbox.exe one (addresses in docs/ptf/classes.md) and was checked read-for-read
// against Sandbox running in an emulator (re/ptf_emu.py).
#include "graph.hpp"

#include <cstring>
#include <functional>
#include <unordered_map>

namespace ptf {

namespace {

class Codec;

/// Walks one node's fields: appends them when reading, replays them when writing.
class Cur {
public:
    Cur(Codec& c, Node& n) : c_(c), n_(n) {}

    std::int64_t i32(const char* name);
    std::int64_t u8(const char* name);
    double f32(const char* name);
    double f64(const char* name);
    std::int64_t size(const char* name);
    void blob(const char* name, std::size_t n);
    void str(const char* name);
    void ptr(const char* name);
    std::int32_t obj(const char* name);

    void objList(const char* stem, std::int64_t count) {
        if (count <= 0) return;
        size((std::string(stem) + "_size").c_str());
        for (std::int64_t k = 0; k < count; ++k)
            obj((std::string(stem) + "[" + std::to_string(k) + "]").c_str());
    }
    void blobArray(const char* stem, std::int64_t count, std::size_t each) {
        if (count <= 0) return;
        size((std::string(stem) + "_size").c_str());
        blob(stem, static_cast<std::size_t>(count) * each);
    }

    std::string prefix;
    Codec& codec() { return c_; }
    Node& node() { return n_; }
    std::size_t index() const { return idx_; }

private:
    Field& next(const char* name, Kind kind);

    Codec& c_;
    Node& n_;
    std::size_t idx_ = 0;
};

using Transfer = void (*)(Cur&);
const std::unordered_map<std::string, Transfer>& classTable();

class Codec {
public:
    bool reading = true;
    Graph* g = nullptr;
    const std::vector<std::uint8_t>* in = nullptr;
    std::size_t pos = 0;
    std::vector<std::uint8_t> out;

    void raw(void* p, std::size_t n) {
        if (reading) {
            if (pos + n > in->size()) throw PtfError("unexpected end of file", pos);
            std::memcpy(p, in->data() + pos, n);
            pos += n;
        } else {
            auto b = static_cast<const std::uint8_t*>(p);
            out.insert(out.end(), b, b + n);
        }
    }
    std::size_t where() const { return reading ? pos : out.size(); }

    std::int32_t rawI32(std::int32_t v = 0) { raw(&v, 4); return v; }

    std::int32_t object(std::int32_t id) { return reading ? readObject() : writeObject(id); }

private:
    std::int32_t readObject() {
        const std::size_t start = pos;
        const std::int32_t oid = rawI32();
        if (oid == 0 || g->nodes.count(oid)) return oid;
        const std::int32_t cid = rawI32();
        auto it = classById_.find(cid);
        if (it == classById_.end()) {
            const std::int32_t len = rawI32();
            if (len <= 0 || len > 256) throw PtfError("bad class name length", pos - 4);
            std::string name(static_cast<std::size_t>(len), '\0');
            raw(name.data(), name.size());
            if (name.back() != '\0') throw PtfError("class name not NUL-terminated", pos);
            name.pop_back();
            it = classById_.emplace(cid, name).first;
        }
        auto t = classTable().find(it->second);
        if (t == classTable().end()) throw PtfError("unknown class '" + it->second + "'", start);
        Node& n = g->nodes[oid];   // registered before transfer, like rts.dll
        n.id = oid;
        n.cls = it->second;
        Cur cur(*this, n);
        t->second(cur);
        return oid;
    }

    std::int32_t writeObject(std::int32_t id) {
        if (id == 0) { rawI32(0); return 0; }
        if (auto w = written_.find(id); w != written_.end()) { rawI32(w->second); return id; }
        const Node* src = g->node(id);
        if (!src) throw PtfError("reference to missing object " + std::to_string(id), out.size());
        auto t = classTable().find(src->cls);
        if (t == classTable().end()) throw PtfError("unknown class '" + src->cls + "'", out.size());
        const std::int32_t newId = counter_++;
        written_[id] = newId;
        rawI32(newId);
        if (auto c = classIds_.find(src->cls); c != classIds_.end()) {
            rawI32(c->second);
        } else {
            const std::int32_t cid = counter_++;
            classIds_[src->cls] = cid;
            rawI32(cid);
            rawI32(static_cast<std::int32_t>(src->cls.size() + 1));
            raw(const_cast<char*>(src->cls.c_str()), src->cls.size() + 1);
        }
        Cur cur(*this, const_cast<Node&>(*src));
        t->second(cur);
        if (cur.index() != src->fields.size())
            throw PtfError(src->cls + " #" + std::to_string(id) + ": " +
                               std::to_string(src->fields.size() - cur.index()) + " extra fields", out.size());
        return id;
    }

    std::unordered_map<std::int32_t, std::string> classById_;
    std::unordered_map<std::int32_t, std::int32_t> written_;
    std::unordered_map<std::string, std::int32_t> classIds_;
    std::int32_t counter_ = 1;
};

Field& Cur::next(const char* name, Kind kind) {
    std::string full = prefix + name;
    if (c_.reading) {
        Field f;
        f.name = std::move(full);
        f.kind = kind;
        n_.fields.push_back(std::move(f));
        ++idx_;
        return n_.fields.back();
    }
    if (idx_ >= n_.fields.size())
        throw PtfError(n_.cls + " #" + std::to_string(n_.id) + ": missing field '" + full + "'", c_.where());
    Field& f = n_.fields[idx_++];
    if (f.name != full || f.kind != kind)
        throw PtfError(n_.cls + " #" + std::to_string(n_.id) + ": expected field '" + full + "' (" +
                           kindName(kind) + "), found '" + f.name + "' (" + kindName(f.kind) + ")",
                       c_.where());
    return f;
}

std::int64_t Cur::i32(const char* name) {
    Field& f = next(name, Kind::I32);
    std::int32_t v = static_cast<std::int32_t>(f.i);
    c_.raw(&v, 4);
    return f.i = v;
}
std::int64_t Cur::size(const char* name) {
    Field& f = next(name, Kind::Size);
    std::int32_t v = static_cast<std::int32_t>(f.i);
    c_.raw(&v, 4);
    return f.i = v;
}
std::int64_t Cur::u8(const char* name) {
    Field& f = next(name, Kind::U8);
    std::uint8_t v = static_cast<std::uint8_t>(f.i);
    c_.raw(&v, 1);
    return f.i = v;
}
double Cur::f32(const char* name) {
    Field& f = next(name, Kind::F32);
    float v = static_cast<float>(f.d);
    c_.raw(&v, 4);
    return f.d = v;
}
double Cur::f64(const char* name) {
    Field& f = next(name, Kind::F64);
    c_.raw(&f.d, 8);
    return f.d;
}
void Cur::blob(const char* name, std::size_t n) {
    Field& f = next(name, Kind::Blob);
    if (c_.reading) f.bytes.resize(n);
    else if (f.bytes.size() != n)
        throw PtfError(prefix + name + ": blob must be " + std::to_string(n) + " bytes", c_.where());
    c_.raw(f.bytes.data(), n);
}
void Cur::str(const char* name) {
    Field& f = next(name, Kind::Str);
    if (c_.reading) {
        const std::size_t at = c_.pos;
        const std::int32_t n = c_.rawI32();
        if (n < 0) throw PtfError("negative string length", at);
        f.null = n == 0;
        if (n > 0) {
            std::string s(static_cast<std::size_t>(n), '\0');
            c_.raw(s.data(), s.size());
            if (s.back() != '\0' || s.find('\0') != s.size() - 1)
                throw PtfError("string is not a single NUL-terminated string", at);
            s.pop_back();
            f.bytes = std::move(s);
        }
    } else {
        if (f.null) { c_.rawI32(0); return; }
        c_.rawI32(static_cast<std::int32_t>(f.bytes.size() + 1));
        c_.raw(f.bytes.data(), f.bytes.size());
        std::uint8_t nul = 0;
        c_.raw(&nul, 1);
    }
}
void Cur::ptr(const char* name) {
    Field& f = next(name, Kind::Ptr);
    if (c_.reading) {
        const std::size_t at = c_.pos;
        const std::int32_t n = c_.rawI32();
        if (n < 0) throw PtfError("negative pointer size", at);
        f.null = n == 0;
        f.bytes.resize(static_cast<std::size_t>(n));
        if (n) c_.raw(f.bytes.data(), f.bytes.size());
    } else {
        c_.rawI32(f.null ? 0 : static_cast<std::int32_t>(f.bytes.size()));
        if (!f.null) c_.raw(f.bytes.data(), f.bytes.size());
    }
}
std::int32_t Cur::obj(const char* name) {
    Field& f = next(name, Kind::Obj);
    const std::int32_t id = c_.object(static_cast<std::int32_t>(f.i));
    // `f` may dangle if reading appended to this node during recursion; it can't (other node).
    return static_cast<std::int32_t>(f.i = id);
}

// ------------------------------------------------------------------ helpers
void base(Cur& c, std::int64_t version) {
    if (c.i32("base") != 1) throw PtfError(c.node().cls + ": Descriptor base version is not 1", c.codec().where());
    c.str("name");
    const std::int64_t v = c.i32("version");
    if (v != version)
        throw PtfError(c.node().cls + " version " + std::to_string(v) + " is not supported (only " +
                           std::to_string(version) + ")", c.codec().where());
}

const Node& nodeOf(Cur& c, std::int32_t id) {
    const Node* n = c.codec().g->node(id);
    if (!n) throw PtfError("missing object " + std::to_string(id), c.codec().where());
    return *n;
}

// Appearance texture slots used by layer k: +0C +10 +18 +14 +1C +20  ->  tex[0,1,3,2,4,5]
constexpr int kLayerSlot[6] = {0, 1, 3, 2, 4, 5};

bool slotSet(const Node& app, int slot) { return app.ref("tex[" + std::to_string(slot) + "]") != 0; }

bool hasTexture(const Node& app) {
    for (int s : {0, 1, 2, 3, 5}) if (slotSet(app, s)) return true;   // +0C +10 +14 +18 +20
    return false;
}

template <class Layer>
void layers(Cur& c, std::int32_t appId, Layer layer) {
    const Node& app = nodeOf(c, appId);
    const std::string saved = c.prefix;
    for (int k = 0; k < 6; ++k) {
        if (!slotSet(app, kLayerSlot[k])) continue;
        c.prefix = saved + "layer" + std::to_string(k) + ".";
        layer(c);
    }
    c.prefix = saved;
}

// ------------------------------------------------------------------ classes
void tTrack(Cur& c) {
    base(c, 8);
    const auto segs = c.i32("segment_count");
    c.u8("night");
    c.f64("light0"); c.f64("light1"); c.f64("light2");
    c.blob("unk2C", 4);
    c.blobArray("records", c.i32("record_count"), 48);
    c.obj("extra");
    c.objList("detail", c.i32("detail_count"));
    c.objList("tsoref", c.i32("tsoref_count"));
    if (segs <= 0) throw PtfError("track has no segments", c.codec().where());
    c.size("seg_size");
    for (std::int64_t k = 0; k < segs; ++k) c.obj(("seg[" + std::to_string(k) + "]").c_str());
}

void tDetail(Cur& c) {
    base(c, 0);
    c.obj("appearance");
    c.f64("d10"); c.f64("d18"); c.f64("d28"); c.f64("d20"); c.f64("d30");
    c.ptr("p38"); c.ptr("p3c");
}

void tAppearance(Cur& c) {
    base(c, 2);
    for (int k = 0; k < 7; ++k) c.obj(("tex[" + std::to_string(k) + "]").c_str());
    for (const char* g : {"v28_", "v40_", "v58_"})
        for (int k = 0; k < 3; ++k) c.f64((std::string(g) + std::to_string(k)).c_str());
    c.f64("d70"); c.f64("d78"); c.f64("d80");
    c.i32("i88");
}

void tTexture(Cur& c) {
    base(c, 2);
    c.str("file");
    c.u8("flag");
}

void tTso(Cur& c) {
    base(c, 3);
    c.str("file");
    c.str("s10");
    c.u8("flag");
}

void tTsoRef(Cur& c) {
    base(c, 6);
    c.f64("x"); c.f64("y"); c.f64("z");
    c.f64("rot1"); c.f64("rot2"); c.f64("rot3");
    c.obj("tso");
    c.i32("i40");
    if (c.u8("no_extra") == 0) {
        c.blob("extra", 32);
        c.i32("extra_i0"); c.i32("extra_i1"); c.i32("extra_i2");
    }
    const auto n = c.i32("item_count");
    if (n > 0) c.blob("items", static_cast<std::size_t>(n) * 40);
    c.str("sound");
}

void tSegment(Cur& c) {
    base(c, 2);
    c.objList("x", c.i32("x_count"));
    c.objList("f", c.i32("f_count"));
    c.objList("w", c.i32("w_count"));
    c.i32("type");
    c.f64("sx"); c.f64("sy"); c.f64("sh");
    c.f64("ex"); c.f64("ey"); c.f64("eh");
    c.u8("b40"); c.u8("b41");
    c.blobArray("rec12", c.i32("rec12_count"), 12);
    c.blobArray("rec16", c.i32("rec16_count"), 16);
}

void tXSection(Cur& c) {
    base(c, 3);
    c.f32("dlat0"); c.f64("alt0"); c.f32("slope0");
    c.f32("dlat1"); c.f64("alt1"); c.f32("slope1");
    c.u8("b14");
    for (const char* g : {"a_", "b_"}) {
        const std::string p(g);
        const auto mode = c.u8((p + "mode").c_str());
        if (mode != 0) { c.f32((p + "f").c_str()); c.i32((p + "i").c_str()); }
        if (mode == 2) { c.u8((p + "u0").c_str()); c.u8((p + "u1").c_str()); }
    }
}

void layerF(Cur& c) {
    if (c.u8("shared") != 0) return;
    c.u8("mode0"); c.f64("texel0");
    c.u8("mode1"); c.f64("texel1");
    c.f64("d0"); c.f64("d1"); c.f64("d2");
}

void layerW(Cur& c) {
    if (c.u8("shared") != 0) return;
    c.u8("mode0"); c.f64("a0"); c.f64("b0");
    c.u8("mode1"); c.f64("a1"); c.f64("b1");
}

void tFSection(Cur& c) {
    base(c, 5);
    c.f32("dlat0"); c.f32("dlat1");
    c.u8("b14");
    c.i32("i19");
    c.i32("i15");
    const auto app = c.obj("appearance");
    if (app == 0 || !hasTexture(nodeOf(c, app))) return;
    c.obj("coords");
    layers(c, app, layerF);
}

void tWSection(Cur& c) {
    base(c, 7);
    c.f32("dlat0"); c.f32("dlat1");
    c.u8("b14");
    c.i32("i55");
    c.f64("height0"); c.f64("height1");
    c.f64("width0"); c.f64("width1");
    c.f64("d35"); c.f64("d3d");
    c.i32("i45");
    if (c.i32("i49") == 1) c.blob("block40", 40);
    c.i32("wall_type");
    const auto subs = c.i32("sub_count");
    const std::string saved = c.prefix;
    for (std::int64_t s = 0; s < subs; ++s) {
        c.prefix = saved + "sub" + std::to_string(s) + ".";
        c.f64("d28");
        const std::string subPrefix = c.prefix;
        for (int face = 0; face < 5; ++face) {
            c.prefix = subPrefix + "face" + std::to_string(face) + ".";
            const auto app = c.obj("appearance");
            if (app == 0 || !hasTexture(nodeOf(c, app))) continue;
            c.obj("coords");
            layers(c, app, layerW);
        }
    }
    c.prefix = saved;
}

void tCoords(Cur& c) {
    base(c, 1);
    c.i32("count");
    if (c.i32("sub_version") != 3) throw PtfError("TextureCoords sub-version is not 3", c.codec().where());
    for (int k = 0; k < 24; ++k) c.ptr(("a[" + std::to_string(k) + "]").c_str());
}

const std::unordered_map<std::string, Transfer>& classTable() {
    static const std::unordered_map<std::string, Transfer> t = {
        {"TrackDescriptor", tTrack},         {"TrackDetailDescriptor", tDetail},
        {"AppearanceDescriptor", tAppearance}, {"TextureDescriptor", tTexture},
        {"TSODescriptor", tTso},             {"TSOReferenceDescriptor", tTsoRef},
        {"SegmentDescriptor", tSegment},     {"X_SectionDescriptor", tXSection},
        {"F_SectionDescriptor", tFSection},  {"W_SectionDescriptor", tWSection},
        {"TextureCoordsDescriptor", tCoords},
    };
    return t;
}

}  // namespace

Graph readPtf(const std::vector<std::uint8_t>& data) {
    Graph g;
    Codec c;
    c.reading = true;
    c.g = &g;
    c.in = &data;
    g.root = c.object(0);
    if (g.root == 0) throw PtfError("file has no root object", 0);
    g.trailer.assign(data.begin() + static_cast<std::ptrdiff_t>(c.pos), data.end());
    return g;
}

std::vector<std::uint8_t> writePtf(const Graph& g) {
    Codec c;
    c.reading = false;
    c.g = const_cast<Graph*>(&g);
    c.object(g.root);
    c.out.insert(c.out.end(), g.trailer.begin(), g.trailer.end());
    return std::move(c.out);
}

}  // namespace ptf
