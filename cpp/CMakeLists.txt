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
