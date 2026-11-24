cmake_minimum_required(VERSION 3.20)
project(ptftools VERSION 0.1.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

if(MSVC)
  add_compile_options(/W4 /permissive- /utf-8 /EHsc)
endif()

# Reader/writer for Papyrus .ptf track files (rts.dll "Archive" format).
add_library(ptf STATIC
  src/archive.cpp
  src/descriptors.cpp
  src/graph.cpp
  src/schema.cpp
)
target_include_directories(ptf PUBLIC src)

# Read-only integrity checker.
add_executable(ptfcheck tools/ptfcheck.cpp)
target_link_libraries(ptfcheck PRIVATE ptf)

# .ptf <-> JSON converter and round-trip checker (used by the Track Toolkit GUI).
add_executable(ptfjson tools/ptfjson.cpp)
target_link_libraries(ptfjson PRIVATE ptf)

# Assetto Corsa track reading (.kn5 models, .dds pictures, layouts, AI line).
add_library(ac STATIC
  src/kn5.cpp
  src/dds.cpp
  src/bc7.cpp
  src/actrack.cpp
)
target_include_directories(ac PUBLIC src)

# AC track folder -> info JSON / 3D scene for the Track Toolkit.
add_executable(acdump tools/acdump.cpp)
target_link_libraries(acdump PRIVATE ac)
target_compile_definitions(acdump PRIVATE _CRT_SECURE_NO_WARNINGS)

# Decode the textures of a .kn5 to raw RGBA files (for checking the DDS/BC7 decoders).
add_executable(kn5tex tools/kn5tex.cpp)
target_link_libraries(kn5tex PRIVATE ac)
target_compile_definitions(kn5tex PRIVATE _CRT_SECURE_NO_WARNINGS)
