// Generic .ptf object graph: every object is a class name plus the exact sequence of
// fields Sandbox's transfer() routine reads or writes. One symmetric routine per class
// (schema.cpp) both reads and writes, so a graph read from a file writes back
// byte-identical. See docs/ptf/classes.md for the layouts.
#pragma once

#include <cstdint>
#include <map>
#include <stdexcept>
#include <string>
#include <vector>

namespace ptf {

enum class Kind : std::uint8_t {
    I32,    // int32
    U8,     // uint8
    F32,    // float
    F64,    // double
    Size,   // transferPointerButNotData: int32 byte size of the array that follows
    Blob,   // fixed number of raw bytes
    Str,    // transferString: int32 length incl. NUL (0 = null) + bytes
    Ptr,    // transferPointer: int32 byte size (0 = null) + bytes
    Obj,    // transferPersistentObject: object id (0 = null)
};

const char* kindName(Kind k);
bool kindFromName(const std::string& s, Kind& out);

struct Field {
    std::string name;
    Kind kind = Kind::I32;
    std::int64_t i = 0;     // I32, U8, Size; object id for Obj (0 = null)
    double d = 0;           // F32, F64
    std::string bytes;      // Blob, Str, Ptr payload (Str without the NUL)
    bool null = false;      // Str, Ptr
};

struct Node {
    std::int32_t id = 0;
    std::string cls;
    std::vector<Field> fields;

    const Field* find(const std::string& name) const;
    double num(const std::string& name, double fallback = 0) const;
    std::int32_t ref(const std::string& name) const;   // object id or 0
};

struct Graph {
    std::map<std::int32_t, Node> nodes;   // by object id as stored in the file
    std::int32_t root = 0;
    std::string trailer;                  // bytes after the root object (some unpacked files have one 00)

    const Node* node(std::int32_t id) const;
};

struct PtfError : std::runtime_error {
    std::size_t offset;
    PtfError(const std::string& msg, std::size_t off)
        : std::runtime_error(msg + " (at file offset " + std::to_string(off) + ")"), offset(off) {}
};

/// Parse a whole .ptf. Throws PtfError.
Graph readPtf(const std::vector<std::uint8_t>& data);
/// Serialize a graph. Throws PtfError if the fields don't follow the class layout.
std::vector<std::uint8_t> writePtf(const Graph& g);

/// JSON form (format "ptfjson" v1). Floats are written so they parse back bit-exact.
std::string toJson(const Graph& g);
Graph fromJson(const std::string& text);

}  // namespace ptf
