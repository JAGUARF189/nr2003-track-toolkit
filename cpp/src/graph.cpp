// Node helpers and the ptfjson text format.
//
// {"format":"ptfjson","version":1,"root":1,"trailer":"",
//  "objects":[{"id":1,"class":"TrackDescriptor","fields":[["base","i32",1],["name","str",null],...]}]}
//
// Values: i32/u8/size -> integer; f32/f64 -> number printed so it parses back bit-exact
// (non-finite values as the string "bits:<hex>"); blob/ptr -> hex string (ptr may be null);
// str -> string (bytes 0x80-0xFF as \u0080-ÿ) or null; obj -> object id or null.
#include "graph.hpp"

#include <cmath>
#include <cstdio>
#include <cstring>

namespace ptf {

namespace {
const char* const kKindNames[] = {"i32", "u8", "f32", "f64", "size", "blob", "str", "ptr", "obj"};
}

const char* kindName(Kind k) { return kKindNames[static_cast<int>(k)]; }

bool kindFromName(const std::string& s, Kind& out) {
    for (int k = 0; k < 9; ++k)
        if (s == kKindNames[k]) { out = static_cast<Kind>(k); return true; }
    return false;
}

const Field* Node::find(const std::string& name) const {
    for (const Field& f : fields)
        if (f.name == name) return &f;
    return nullptr;
}

double Node::num(const std::string& name, double fallback) const {
    const Field* f = find(name);
    if (!f) return fallback;
    if (f->kind == Kind::F32 || f->kind == Kind::F64) return f->d;
    return static_cast<double>(f->i);
}

std::int32_t Node::ref(const std::string& name) const {
    const Field* f = find(name);
    return f && f->kind == Kind::Obj ? static_cast<std::int32_t>(f->i) : 0;
}

const Node* Graph::node(std::int32_t id) const {
    auto it = nodes.find(id);
    return it == nodes.end() ? nullptr : &it->second;
}

// ------------------------------------------------------------------ writing JSON
namespace {

void putHex(std::string& o, const std::string& b) {
    static const char* hx = "0123456789abcdef";
    o += '"';
    for (unsigned char ch : b) { o += hx[ch >> 4]; o += hx[ch & 15]; }
    o += '"';
}

void putStr(std::string& o, const std::string& s) {
    o += '"';
    char buf[8];
    for (unsigned char ch : s) {
        if (ch == '"' || ch == '\\') { o += '\\'; o += static_cast<char>(ch); }
        else if (ch < 0x20 || ch >= 0x7F) { std::snprintf(buf, sizeof buf, "\\u%04x", ch); o += buf; }
        else o += static_cast<char>(ch);
    }
    o += '"';
}

void putNum(std::string& o, double d, bool single) {
    char buf[40];
    if (!std::isfinite(d)) {
        std::uint64_t bits;
        if (single) { float f = static_cast<float>(d); std::uint32_t b32; std::memcpy(&b32, &f, 4); bits = b32; }
        else std::memcpy(&bits, &d, 8);
        std::snprintf(buf, sizeof buf, "\"bits:%llx\"", static_cast<unsigned long long>(bits));
        o += buf;
        return;
    }
    // Shortest representation that parses back exactly.
    for (int prec = single ? 6 : 15; prec <= 17; ++prec) {
        std::snprintf(buf, sizeof buf, "%.*g", prec, d);
        const double back = std::strtod(buf, nullptr);
        if (single ? static_cast<float>(back) == static_cast<float>(d) : back == d) break;
    }
    if (d == 0 && std::signbit(d)) std::snprintf(buf, sizeof buf, "-0.0");
    o += buf;
}

}  // namespace

std::string toJson(const Graph& g) {
    std::string o;
    o.reserve(1 << 20);
    o += "{\"format\":\"ptfjson\",\"version\":1,\"root\":" + std::to_string(g.root) + ",\"trailer\":";
    putHex(o, g.trailer);
    o += ",\n\"objects\":[";
    bool firstNode = true;
    for (const auto& [id, n] : g.nodes) {
        o += firstNode ? "\n" : ",\n";
        firstNode = false;
        o += "{\"id\":" + std::to_string(id) + ",\"class\":\"" + n.cls + "\",\"fields\":[";
        bool first = true;
        for (const Field& f : n.fields) {
            if (!first) o += ',';
            first = false;
            o += '[';
            putStr(o, f.name);
            o += ",\"";
            o += kindName(f.kind);
            o += "\",";
            switch (f.kind) {
                case Kind::I32: case Kind::U8: case Kind::Size: o += std::to_string(f.i); break;
                case Kind::F32: putNum(o, f.d, true); break;
                case Kind::F64: putNum(o, f.d, false); break;
                case Kind::Blob: putHex(o, f.bytes); break;
                case Kind::Ptr: if (f.null) o += "null"; else putHex(o, f.bytes); break;
                case Kind::Str: if (f.null) o += "null"; else putStr(o, f.bytes); break;
                case Kind::Obj: if (f.i == 0) o += "null"; else o += std::to_string(f.i); break;
            }
            o += ']';
        }
        o += "]}";
    }
    o += "\n]}\n";
    return o;
}

// ------------------------------------------------------------------ reading JSON
namespace {

class JsonIn {
public:
    explicit JsonIn(const std::string& s) : s_(s) {}

    [[noreturn]] void fail(const std::string& what) const { throw PtfError("JSON: " + what, p_); }
    void ws() { while (p_ < s_.size() && (s_[p_] == ' ' || s_[p_] == '\n' || s_[p_] == '\r' || s_[p_] == '\t')) ++p_; }
    char peek() { ws(); return p_ < s_.size() ? s_[p_] : '\0'; }
    void expect(char c) { if (peek() != c) fail(std::string("expected '") + c + "'"); ++p_; }
    bool eat(char c) { if (peek() == c) { ++p_; return true; } return false; }
    bool null() {
        if (peek() == 'n' && s_.compare(p_, 4, "null") == 0) { p_ += 4; return true; }
        return false;
    }

    std::string str() {
        expect('"');
        std::string out;
        while (true) {
            if (p_ >= s_.size()) fail("unterminated string");
            char c = s_[p_++];
            if (c == '"') break;
            if (static_cast<unsigned char>(c) >= 0x80) {
                // UTF-8 (other JSON writers emit characters, not \u escapes). File strings are
                // single bytes, so only U+0080..U+00FF are valid: 110000xx 10xxxxxx.
                const unsigned char b0 = static_cast<unsigned char>(c);
                const unsigned char b1 = p_ < s_.size() ? static_cast<unsigned char>(s_[p_]) : 0;
                if ((b0 & 0xFE) != 0xC2 || (b1 & 0xC0) != 0x80) fail("text character above U+00FF is not a byte");
                ++p_;
                out += static_cast<char>(((b0 & 0x03) << 6) | (b1 & 0x3F));
                continue;
            }
            if (c != '\\') { out += c; continue; }
            char e = s_[p_++];
            switch (e) {
                case '"': case '\\': case '/': out += e; break;
                case 'n': out += '\n'; break;
                case 't': out += '\t'; break;
                case 'r': out += '\r'; break;
                case 'b': out += '\b'; break;
                case 'f': out += '\f'; break;
                case 'u': {
                    const unsigned v = static_cast<unsigned>(std::stoul(s_.substr(p_, 4), nullptr, 16));
                    p_ += 4;
                    if (v > 0xFF) fail("\\u escape above 0xFF is not a byte");
                    out += static_cast<char>(v);
                    break;
                }
                default: fail("bad escape");
            }
        }
        return out;
    }

    std::string hex() {
        std::string h = str(), out;
        if (h.size() % 2) fail("odd-length hex");
        for (std::size_t k = 0; k < h.size(); k += 2)
            out += static_cast<char>(std::stoi(h.substr(k, 2), nullptr, 16));
        return out;
    }

    double number(bool single) {
        if (peek() == '"') {
            std::string t = str();
            if (t.rfind("bits:", 0) != 0) fail("bad number string");
            const unsigned long long bits = std::stoull(t.substr(5), nullptr, 16);
            if (single) { std::uint32_t b = static_cast<std::uint32_t>(bits); float f; std::memcpy(&f, &b, 4); return f; }
            double d; std::memcpy(&d, &bits, 8); return d;
        }
        ws();
        const char* begin = s_.c_str() + p_;
        char* end = nullptr;
        const double d = std::strtod(begin, &end);
        if (end == begin) fail("expected a number");
        p_ += static_cast<std::size_t>(end - begin);
        return d;
    }

    std::int64_t integer() {
        ws();
        const char* begin = s_.c_str() + p_;
        char* end = nullptr;
        const long long v = std::strtoll(begin, &end, 10);
        if (end == begin) fail("expected an integer");
        p_ += static_cast<std::size_t>(end - begin);
        return v;
    }

    void skipValue() {
        char c = peek();
        if (c == '"') { str(); return; }
        if (c == '{') { ++p_; if (eat('}')) return; do { str(); expect(':'); skipValue(); } while (eat(',')); expect('}'); return; }
        if (c == '[') { ++p_; if (eat(']')) return; do skipValue(); while (eat(',')); expect(']'); return; }
        if (null()) return;
        if (s_.compare(p_, 4, "true") == 0) { p_ += 4; return; }
        if (s_.compare(p_, 5, "false") == 0) { p_ += 5; return; }
        number(false);
    }

private:
    const std::string& s_;
    std::size_t p_ = 0;
};

Field parseField(JsonIn& j) {
    Field f;
    j.expect('[');
    f.name = j.str();
    j.expect(',');
    if (!kindFromName(j.str(), f.kind)) j.fail("unknown field kind");
    j.expect(',');
    switch (f.kind) {
        case Kind::I32: case Kind::U8: case Kind::Size: f.i = j.integer(); break;
        case Kind::F32: f.d = static_cast<float>(j.number(true)); break;
        case Kind::F64: f.d = j.number(false); break;
        case Kind::Blob: f.bytes = j.hex(); break;
        case Kind::Ptr: if (j.null()) f.null = true; else f.bytes = j.hex(); break;
        case Kind::Str: if (j.null()) f.null = true; else f.bytes = j.str(); break;
        case Kind::Obj: f.i = j.null() ? 0 : j.integer(); break;
    }
    j.expect(']');
    return f;
}

}  // namespace

Graph fromJson(const std::string& text) {
    Graph g;
    JsonIn j(text);
    j.expect('{');
    do {
        const std::string key = j.str();
        j.expect(':');
        if (key == "root") g.root = static_cast<std::int32_t>(j.integer());
        else if (key == "trailer") g.trailer = j.hex();
        else if (key == "objects") {
            j.expect('[');
            if (!j.eat(']')) {
                do {
                    Node n;
                    j.expect('{');
                    do {
                        const std::string k = j.str();
                        j.expect(':');
                        if (k == "id") n.id = static_cast<std::int32_t>(j.integer());
                        else if (k == "class") n.cls = j.str();
                        else if (k == "fields") {
                            j.expect('[');
                            if (!j.eat(']')) {
                                do n.fields.push_back(parseField(j)); while (j.eat(','));
                                j.expect(']');
                            }
                        } else j.skipValue();
                    } while (j.eat(','));
                    j.expect('}');
                    if (n.id == 0) j.fail("object without id");
                    const std::int32_t id = n.id;
                    if (!g.nodes.emplace(id, std::move(n)).second) j.fail("duplicate object id " + std::to_string(id));
                } while (j.eat(','));
                j.expect(']');
            }
        } else j.skipValue();
    } while (j.eat(','));
    j.expect('}');
    if (!g.node(g.root)) throw PtfError("JSON: root object missing", 0);
    return g;
}

}  // namespace ptf
