# Getting started with development

Orbit consists of two parts - the frontend and the collector, also called the service.
The collector is responsible for instrumenting the target process and recording
profiling events which are then streamed to the frontend, also called the UI.

The communication between frontend and collector is handled by a [gRPC](https://grpc.io/)
connection. gRPC uses HTTP 2.0 as its base communication layer. When talking to a Stadia
instance we wrap that once more into an SSH tunnel.

## Platforms

Both the frontend and the collector (service) run on Windows and Linux.

## Compilers

To build Orbit you need a compiler capable of C++20. The following ones should be fine.

- GCC 9 and above on Linux
- Clang 7 and above on Linux
- MSVC 2022 on Windows (VS2022, toolset v143)

## Dependencies

All third-party libraries and dependencies are managed by **Conan 2**. Make sure
you have Python 3 and Conan 2 installed:

```
pip install "conan>=2.0"
```

> **Note:** The old `bootstrap-orbit.ps1` / `build.ps1` scripts use the Conan 1 API
> and no longer work. Use the Conan 2 workflow described below.

### Qt on Linux

On Linux, Orbit uses the distribution's system Qt 5 installation. You will need
at least Qt 5.12.4. Ubuntu 20.04 LTS and above are fine; Ubuntu 18.04 ships Qt 5.9
which is **not** sufficient.

### Qt on Windows

Qt 5 is **not** managed by Conan — it must be installed separately. Download a
prebuilt distribution from [The Qt Company](https://qt.io/) (registration required)
or use a third-party installer such as
[aqtinstall](https://github.com/miurahr/aqtinstall). Make sure to:

- Match the Qt build to your Visual Studio version and x64 architecture
  (e.g. `msvc2019_64` works for both VS2019 and VS2022)
- Install the **QtWebEngine** component (not selected by default)
- Use at least Qt 5.12.4; Qt 5.15.x is recommended

Set the `Qt5_DIR` environment variable to point at the directory containing
`Qt5Config.cmake` before running the build. For example:

```powershell
$env:Qt5_DIR = "C:\Qt\5.15.2\msvc2019_64\lib\cmake\Qt5"
```

## Building Orbit

### Windows (first time or after dependency changes)

```powershell
# From the repo root, with Qt5_DIR set (see above)
conan install . --output-folder=build --build=missing
cmake --preset conan-default
cmake --build --preset conan-release
```

This generates `CMakeUserPresets.json` and `build/generators/CMakePresets.json` on
first run. Subsequent runs only need the last two commands unless `conanfile.py`
changes.

**Build without the GUI** (skips Qt requirement):

```powershell
conan install . --output-folder=build --build=missing
cmake --preset conan-default -DWITH_GUI=OFF
cmake --build --preset conan-release
```

### Windows (incremental builds)

```powershell
cd build
cmake --build . --config Release
```

### Linux (first time or after dependency changes)

```bash
./bootstrap-orbit.sh
```

`bootstrap-orbit.sh` installs Conan (if needed), installs the Conan config, and
calls `build.sh` for the default profile.

### Linux (incremental builds)

```bash
cd build_default_relwithdebinfo/
cmake --build .
```

## Running Orbit

### Windows

```powershell
.\build\bin\Orbit.exe          # UI frontend
.\build\bin\OrbitService.exe   # Local service (run as Administrator for full access)
```

### Linux

```bash
./build_default_relwithdebinfo/bin/Orbit            # UI frontend
sudo ./build_default_relwithdebinfo/bin/OrbitService  # Collector (needs root)
```

## Consistent code styling

We use `clang-format` to achieve a consistent code styling across
the whole code base. You need at least version 7.0.0 of `clang-format`.

Please ensure that you applied `clang-format` to all your
files in your pull request. Otherwise a presubmit check will fail
and unfortunately only Googlers have access to the detailed log.

On Windows, we recommend getting `clang-format` directly from the
LLVM.org website. They offer binary packages of `clang`, where
`clang-format` is part of.

Visual Studio 2017+ ships `clang-format` as part of the IDE though.
(https://devblogs.microsoft.com/cppblog/clangformat-support-in-visual-studio-2017-15-7-preview-1/)

On most Linux distributions, there is a dedicated package called `clang-format`.

Most modern IDEs provide `clang-format` integration via either an extension
or directly.

A `.clang-format` file which defines our specific code style lives in the
top level directory of the repository. The style is identical to the Google
style. 

## Code Style

As mentioned above we use `clang-format` to enforce certain aspects of code
style. The Google C++ style guide we are following in that can be found
[here](https://google.github.io/styleguide/cppguide.html). It includes brief
discussions or rationales for all the style decisions.

Beyond what it is in the style guide we agreed to a few more additional rules
specific to the Orbit project:

### [[nodiscard]]
We use `[[nodiscard]]` for (almost) all new class methods and free functions
that return a value. If you encouter a use case where it makes no sense or
hurts readability feel free to skip it though.

We do not touch existing code merely to add `[[nodiscard]]` though.

### Error handling
For error handling we use `ErrorMessageOr<T>` from 
[Result.h](https://github.com/google/orbit/blob/main/src/OrbitBase/include/OrbitBase/Result.h).
This class serves the same purpose as `absl::StatusOr<T>`. We thought about
switching to absl but currently the advantage does not seem large enough to
warrant the effort. So for now we stick with `ErrorMessageOr<T>`. 

In cases where no error message needs to be returned it is perfectly fine to
use `std::optional`.

### Exceptions
Currently our code is compiled with exceptions but we strive towards a world
with no exceptions. Particularly we don't use methods from std that throw
exceptions but prefer the variants returning error codes (e.g. in 
`std::filesystem`).

### Namespaces
We place all code belonging to a module named `ModuleName` or `OrbitModuleName`
inside the top-level namespace `orbit_module_name`. All namespaces start with
the `orbit_` prefix. We do not use nested namespaces.

Exclusively for code that needs to be in a `.h` file public to a module (i.e.,
in the `include` directory) even if that code shouldn't be used by other
modules, we use the top-level namespace `orbit_module_name_internal` instead.
This is *not* nested inside `orbit_module_name`.

### File system structure
Each module gets its own subdirectory under `src/`. The subdirectory's name
should be spelled in camel case, i.e. `src/ModuleName`. All public header
files go into a separate include subdirectory `include/ModuleName`, i.e.
the full path looks like `src/ModuleName/include/ModuleName/PublicHeader.h`.

Note that the module's name appears twice in this path.
They will be included relative to the `include/` subdirectory, i.e.
`#include "ModuleName/PublicHeader.h"`.

CPP files and private header files go directly into the module's subdirectory,
i.e. `src/ModuleName/MyClass.cpp`.

### Tests
Unit test files go into the root directory of the module. They should be named after
the component they are testing - followed by the suffix `Test`,
i.e. `src/ModuleName/MyClassTest.cpp`.

### Fuzzers
Similar to unit tests, fuzzers are also named after the component they are fuzzing
with the suffix `Fuzzer`, i.e. `src/ModuleName/MyClassFuzzer.cpp`.

### Platform-specific code
We try to keep platform-specific code out of header files and maintain a
platform-agnostic header for inclusion. The platform-specific implementations
go into separate files, suffixed by the platform name, i.e. `MyClassWindows.cpp`
or `MyClassLinux.cpp`.

It's not always possible to keep the header entirely platform-agnostic. If some
distinctions need to be made, you can use preprocessor macros, e.g.:

```
#ifdef _WIN32
#include <winsock2.h>
#else
#include <sys/socket.h>
#endif
```

## FAQ

### What's the difference between `bootstrap-orbit.sh` and `build.sh`? (Linux)

`bootstrap-orbit.sh` is a one-time setup script per developer machine. It:

- Installs Conan if not already present
- Installs the Conan configuration

Afterwards it calls `build.sh`.

`build.sh` does the per-build-configuration work: runs `conan install`, runs
`cmake`, and starts the build. Re-run it whenever `conanfile.py` changes (e.g.
after pulling or switching branches).

If the build fails with a cryptic CMake error after calling `build.sh`, try
deleting the build directory and re-running `build.sh` to start clean.

> **Note:** The Windows equivalents `bootstrap-orbit.ps1` and `build.ps1` target
> Conan 1 and are no longer functional. On Windows, use the Conan 2 workflow
> described in the **Building Orbit** section above.

### `build.sh` after every one-line change takes forever — what should I do?

`build.sh` is not for incremental builds. Use it once to set up a build directory,
then do incremental builds directly via CMake:

```bash
cd build_default_relwithdebinfo/
cmake --build .          # Linux
```

```powershell
cd build
cmake --build . --config Release   # Windows
```

### How do I enable `ninja` for my Linux build?

Create the build directory manually, install Conan dependencies, then invoke CMake
with the Ninja generator. Example for a debug build with clang-9:

```bash
mkdir build_clang9_debug
cd build_clang9_debug/
conan install --output-folder=. --build=missing -pr clang9_debug ../
cmake -DCMAKE_CXX_COMPILER=clang++-9 -DCMAKE_C_COMPILER=clang-9 \
      -DCMAKE_BUILD_TYPE=Debug -G Ninja ../
ninja
```

### How do I integrate with CLion?

CLion manages build directories itself, so you need to install Conan dependencies
into CLion's build directory manually:

```bash
cd <build_directory_created_by_clion>/
conan install --output-folder=. --build=missing ../
```

Then trigger a CMake re-run from CLion. The
[CLion Conan Plugin](https://plugins.jetbrains.com/plugin/11956-conan) can
automate this step.

### How do I open the project in Visual Studio?

After running `conan install` and `cmake --preset conan-default`, open the
generated solution file `build/orbit_deps.sln` (or use **File → Open → Folder**
in VS2022 and select the repo root — VS will detect `CMakeUserPresets.json`
automatically).

Make sure Visual Studio is set to the **Release** configuration to match the
Conan-installed dependencies. Mixing configurations in the same build folder
causes linker errors.

### How do I integrate with Visual Studio Code?

Visual Studio Code uses configuration files to specify tasks. These are provided
in `contrib/.vscode`. To enable them, copy the folder to the repo root:

```bash
cp -r contrib/vscode .vscode
```

### How can I use separate debugging symbols for Linux binaries?

Orbit supports loading symbols from your workstation. Simply add directories that contain debugging symbols to the `SymbolPaths.txt` file. This file can be found at

- Windows: `C:\Users\<user>\AppData\Roaming\OrbitProfiler\config\SymbolPaths.txt`
- Linux: `~/orbitprofiler/config/SymbolPaths.txt`

The symbols file must named in one of three ways. The same fname as the binary (`game.elf`), the same name plus the `.debug` extension (`game.elf.debug`) or the same name but the `.debug` extension instead of the original one (`game.debug`). To make sure the binary and symbols file have been produced in the same build, Orbit checks that they have a matching build id.

## Cross-Compiling for GGP

Cross compilation is supported on Windows and Linux host systems.

_Note:_ Cross compiling the UI is not supported.

_Note:_ Since the GGP SDK is not publicly available, this only works inside
of Google, at least for now.

Call the script `bootstrap-orbit-ggp.{sh,ps1}` which creates a package out of the GGP
SDK (you do not need to have the SDK installed for this to work, but you will need it
for deployment), and compiles Orbit against the toolchain from the GGP SDK package.

Finally, `build_ggp_release/package/bin/OrbitService` can be copied over
to the instance:

```bash
ggp ssh put build_ggp_release/package/bin/OrbitService /mnt/developer/
```

before the service can be started with:

```bash
ggp ssh shell
> sudo /mnt/developer/OrbitService
```
