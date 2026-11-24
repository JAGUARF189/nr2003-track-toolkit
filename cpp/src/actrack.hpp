// An Assetto Corsa track folder: its layouts, the models each layout loads, its driving
// surfaces and its AI line.
//
// - Layouts: `models.ini` is the default layout; `models_<name>.ini` adds layout <name>, whose
//   own `ai\` and `data\` live in the subfolder <name>\ (falling back to the track's own).
//   A track with no models.ini loads `<folder name>.kn5`.
// - models.ini: [MODEL_n] FILE=path relative to the track folder, POSITION=x,y,z.
// - data\surfaces.ini: [SURFACE_n] KEY=..., IS_VALID_TRACK, IS_PITLANE, FRICTION. A mesh whose
//   name is digits followed by a KEY (e.g. `1TARMAC-MUG-A`, `0KERB02`) is a driving surface.
// - ai\fast_lane.ai: int32 version (7), int32 n, int32, int32, then n x { float x, y, z,
//   distance, int32 id }, int32 m, then m x 18 floats per point (speed, gas, brake, latG,
//   radius, left edge, right edge, camber, direction, normal[3], length, forward[3], tag,
//   grade). Checked on mugello and monaco_2020.
#pragma once

#include <string>
#include <vector>

namespace ac {

struct ModelRef {
    std::string file;   // as written in models.ini
    std::string path;   // full path
    float position[3] = {0, 0, 0};
    float rotation[3] = {0, 0, 0};
    bool exists = false;
};

struct Layout {
    std::string name;    // "" = the default layout
    std::string title;   // from ui\[name\]ui_track.json, else the name
    std::vector<ModelRef> models;
    std::string aiDir, dataDir;
};

struct Surface {
    std::string key;
    bool validTrack = true, pitlane = false;
    float friction = 1;
};

struct AiPoint {
    float pos[3];       // AC coordinates
    float distance;     // metres along the lap
    float left, right;  // distance to the track edges (0 if unknown)
};

struct TrackFolder {
    std::string dir, name, title;
    std::vector<Layout> layouts;
};

TrackFolder scan(const std::string& dir);
std::vector<Surface> surfaces(const Layout& l);
std::vector<AiPoint> aiLine(const Layout& l, const std::string& file = "fast_lane.ai");

// Index of the surface a mesh name drives on (longest KEY after the leading digits), or -1.
int surfaceOf(const std::string& meshName, const std::vector<Surface>& s);

}  // namespace ac
