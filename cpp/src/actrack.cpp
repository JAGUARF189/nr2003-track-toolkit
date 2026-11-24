#include "actrack.hpp"

#include <algorithm>
#include <cctype>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <map>
#include <sstream>
#include <stdexcept>

namespace fs = std::filesystem;

namespace ac {
namespace {

std::string lower(std::string s) {
    for (char& c : s) c = char(std::tolower(static_cast<unsigned char>(c)));
    return s;
}

std::string trim(const std::string& s) {
    const auto a = s.find_first_not_of(" \t\r\n");
    if (a == std::string::npos) return {};
    const auto b = s.find_last_not_of(" \t\r\n");
    return s.substr(a, b - a + 1);
}

using Ini = std::vector<std::pair<std::string, std::map<std::string, std::string>>>;

Ini readIni(const fs::path& p) {
    Ini out;
    std::ifstream f(p);
    std::string line;
    while (std::getline(f, line)) {
        line = trim(line);
        if (const auto c = line.find_first_of(";/"); c == 0) continue;
        if (line.empty()) continue;
        if (line.front() == '[' && line.back() == ']') {
            out.push_back({line.substr(1, line.size() - 2), {}});
        } else if (const auto eq = line.find('='); eq != std::string::npos && !out.empty()) {
            std::string v = trim(line.substr(eq + 1));
            if (const auto sc = v.find(';'); sc != std::string::npos) v = trim(v.substr(0, sc));
            out.back().second[trim(line.substr(0, eq))] = v;
        }
    }
    return out;
}

void floats3(const std::string& s, float out[3]) {
    std::stringstream ss(s);
    std::string part;
    for (int k = 0; k < 3 && std::getline(ss, part, ','); ++k) out[k] = std::stof(trim(part));
}

// "name": "..." from a ui_track.json (enough for a title; no full JSON parser needed).
std::string jsonName(const fs::path& p) {
    std::ifstream f(p, std::ios::binary);
    if (!f) return {};
    std::string s((std::istreambuf_iterator<char>(f)), {});
    const auto k = s.find("\"name\"");
    if (k == std::string::npos) return {};
    const auto a = s.find('"', s.find(':', k) + 1);
    const auto b = s.find('"', a + 1);
    if (a == std::string::npos || b == std::string::npos) return {};
    return s.substr(a + 1, b - a - 1);
}

std::vector<ModelRef> modelsFrom(const fs::path& ini, const fs::path& dir) {
    std::vector<ModelRef> out;
    for (const auto& [section, kv] : readIni(ini)) {
        if (section.rfind("MODEL", 0) != 0) continue;
        const auto f = kv.find("FILE");
        if (f == kv.end()) continue;
        ModelRef m;
        m.file = f->second;
        m.path = (dir / f->second).string();
        if (const auto p = kv.find("POSITION"); p != kv.end()) floats3(p->second, m.position);
        if (const auto r = kv.find("ROTATION"); r != kv.end()) floats3(r->second, m.rotation);
        m.exists = fs::is_regular_file(m.path);
        out.push_back(std::move(m));
    }
    return out;
}

}  // namespace

TrackFolder scan(const std::string& dirStr) {
    const fs::path dir(dirStr);
    if (!fs::is_directory(dir)) throw std::runtime_error(dirStr + " isn't a folder");
    TrackFolder t;
    t.dir = dir.string();
    t.name = dir.filename().string();
    t.title = jsonName(dir / "ui" / "ui_track.json");
    const auto dirOf = [&](const std::string& sub, const char* what) {
        const fs::path own = dir / sub / what;
        return (fs::is_directory(own) ? own : dir / what).string();
    };
    if (fs::is_regular_file(dir / "models.ini")) {
        t.layouts.push_back({"", t.title, modelsFrom(dir / "models.ini", dir), (dir / "ai").string(), (dir / "data").string()});
    } else if (fs::is_regular_file(dir / (t.name + ".kn5"))) {
        ModelRef m;
        m.file = t.name + ".kn5";
        m.path = (dir / m.file).string();
        m.exists = true;
        t.layouts.push_back({"", t.title, {m}, (dir / "ai").string(), (dir / "data").string()});
    }
    std::vector<fs::path> extra;
    for (const auto& e : fs::directory_iterator(dir)) {
        const std::string n = lower(e.path().filename().string());
        if (e.is_regular_file() && n.rfind("models_", 0) == 0 && e.path().extension() == ".ini") extra.push_back(e.path());
    }
    std::sort(extra.begin(), extra.end());
    for (const auto& p : extra) {
        Layout l;
        const std::string stem = p.stem().string();
        l.name = stem.substr(7);
        l.title = jsonName(dir / "ui" / l.name / "ui_track.json");
        l.models = modelsFrom(p, dir);
        l.aiDir = dirOf(l.name, "ai");
        // a layout's surfaces.ini may be switched off (renamed); use the track's then
        l.dataDir = fs::is_regular_file(dir / l.name / "data" / "surfaces.ini") ? (dir / l.name / "data").string() : (dir / "data").string();
        t.layouts.push_back(std::move(l));
    }
    for (auto& l : t.layouts)
        if (l.title.empty()) l.title = l.name.empty() ? (t.title.empty() ? t.name : t.title) : l.name;
    if (t.layouts.empty()) throw std::runtime_error(dirStr + " has no models.ini or " + t.name + ".kn5, so it isn't an Assetto Corsa track folder");
    return t;
}

std::vector<Surface> surfaces(const Layout& l) {
    std::vector<Surface> out;
    for (const auto& [section, kv] : readIni(fs::path(l.dataDir) / "surfaces.ini")) {
        if (section.rfind("SURFACE_", 0) != 0) continue;
        const auto k = kv.find("KEY");
        if (k == kv.end() || k->second.empty()) continue;
        Surface s;
        s.key = k->second;
        if (const auto v = kv.find("IS_VALID_TRACK"); v != kv.end()) s.validTrack = v->second != "0";
        if (const auto v = kv.find("IS_PITLANE"); v != kv.end()) s.pitlane = v->second == "1";
        if (const auto v = kv.find("FRICTION"); v != kv.end()) {
            try {
                s.friction = std::stof(v->second);
            } catch (...) {
            }
        }
        out.push_back(std::move(s));
    }
    // Every track also has AC's own surfaces (system\data\surfaces.ini: ROAD, GRASS, KERB, SAND);
    // mugello's grass uses those. The track's own come first and win.
    for (fs::path p = fs::path(l.dataDir); !p.empty() && p.has_parent_path() && p != p.parent_path(); p = p.parent_path()) {
        const fs::path global = p / "system" / "data" / "surfaces.ini";
        if (!fs::is_regular_file(global)) continue;
        for (const auto& [section, kv] : readIni(global)) {
            const auto k = kv.find("KEY");
            if (section.rfind("SURFACE_", 0) != 0 || k == kv.end() || k->second.empty()) continue;
            if (std::any_of(out.begin(), out.end(), [&](const Surface& s) { return s.key == k->second; })) continue;
            Surface s;
            s.key = k->second;
            if (const auto v = kv.find("IS_VALID_TRACK"); v != kv.end()) s.validTrack = v->second != "0";
            if (const auto v = kv.find("IS_PITLANE"); v != kv.end()) s.pitlane = v->second == "1";
            out.push_back(std::move(s));
        }
        break;
    }
    // AC treats meshes named <digits>WALL... as walls even when surfaces.ini doesn't list WALL
    // (monaco_2020's 0WALL01...).
    if (std::none_of(out.begin(), out.end(), [](const Surface& s) { return s.key == "WALL"; })) {
        Surface w;
        w.key = "WALL";
        w.validTrack = false;
        out.push_back(w);
    }
    return out;
}

int surfaceOf(const std::string& meshName, const std::vector<Surface>& s) {
    std::size_t k = 0;
    while (k < meshName.size() && std::isdigit(static_cast<unsigned char>(meshName[k]))) ++k;
    if (k == 0) return -1;
    const std::string rest = meshName.substr(k);
    int best = -1;
    std::size_t bestLen = 0;
    for (std::size_t i = 0; i < s.size(); ++i) {
        const std::string& key = s[i].key;
        if (key.size() > bestLen && rest.size() >= key.size() && rest.compare(0, key.size(), key) == 0) {
            best = int(i);
            bestLen = key.size();
        }
    }
    return best;
}

std::vector<AiPoint> aiLine(const Layout& l, const std::string& file) {
    std::ifstream f(fs::path(l.aiDir) / file, std::ios::binary);
    if (!f) return {};
    std::vector<char> b((std::istreambuf_iterator<char>(f)), {});
    const auto at = [&](std::size_t off, auto& v) {
        if (off + sizeof v > b.size()) throw std::runtime_error(file + " is cut short");
        std::memcpy(&v, b.data() + off, sizeof v);
    };
    std::int32_t version = 0, n = 0;
    at(0, version);
    at(4, n);
    if (n <= 0 || n > 1000000) throw std::runtime_error(file + ": bad point count");
    std::vector<AiPoint> out(static_cast<std::size_t>(n));
    for (std::int32_t k = 0; k < n; ++k) {
        float p[4];
        at(16 + std::size_t(k) * 20, p);
        AiPoint& a = out[std::size_t(k)];
        std::memcpy(a.pos, p, sizeof a.pos);
        a.distance = p[3];
        a.left = a.right = 0;
    }
    const std::size_t extra = 16 + std::size_t(n) * 20;
    std::int32_t m = 0;
    if (extra + 4 <= b.size()) at(extra, m);
    if (m == n && extra + 4 + std::size_t(m) * 72 <= b.size()) {
        for (std::int32_t k = 0; k < m; ++k) {
            float e[18];
            at(extra + 4 + std::size_t(k) * 72, e);
            out[std::size_t(k)].left = e[5];
            out[std::size_t(k)].right = e[6];
        }
    }
    return out;
}

}  // namespace ac
