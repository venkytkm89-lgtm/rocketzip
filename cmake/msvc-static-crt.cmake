# Used only when building for Windows (MSVC), via .cargo/config.toml.
# zlib-ng is compiled with CMake, which defaults to the DLL C runtime (/MD).
# Force the static runtime so the programs need no Visual C++ Redistributable.
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded" CACHE STRING "" FORCE)
