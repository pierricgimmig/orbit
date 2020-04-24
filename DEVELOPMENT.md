# Getting started with development

## Platforms

Windows 10 and Linux are supported.

## Compilers

To build Orbit you need a compiler capable of C++17. The following ones should be fine.
You should prefer clang over GCC, since most of the developers build with clang by default.

* GCC 8 and above on Linux
* Clang 7 and above on Linux
* MSVC 2017, 2019 and above on Windows

## Building Orbit

Orbit relies on `conan` as its package manager.  Conan is written in Python3,
so make sure you have either Conan installed or at least have Python3 installed.

The `bootstrap-orbit.{sh,ps1}` will try to install `conan` via `pip3` if not
installed and reachable via `PATH`. Afterwards it calls `build.{sh,ps1}` which
will compile Orbit for you.

On Linux, `python3` should be preinstalled anyway, but you might need to install
pip (package name: `python3-pip`).

On Windows, one option to install Python is via the Visual Studio Installer.
Alternatively you can download prebuilts from [python.org](https://www.python.org/)
(In both cases verify that `pip3.exe` is in the path, otherwise the bootstrap
script will not be able to install conan for you.)

## Dependencies

All our third-party libraries and dependencies are managed by conan.

There are some exceptions. On Linux, we rely by default on the distribution's Qt5
and Mesa installation. This can be changed by modifying the conan package options
`system_qt` and `system_mesa`.

The simplest way to do that is to change the default values of these two options.
Check out `conanfile.py`. There is a python dictionary called `default_options`
defined in the python class `OrbitConan`.

## Consistent code styling

We use `clang-format` to achieve a consistent code styling across
the whole code base. You need at least version 7.0.0 of `clang-format`.

Please ensure that you applied `clang-format` to all your
files in your pull request.

On Windows, we recommend getting `clang-format` directly from the
LLVM.org website. They offer binary packages of `clang`, where
`clang-format` is part of.

Visual Studio 2017 ships `clang-format` as part of the IDE.
(https://devblogs.microsoft.com/cppblog/clangformat-support-in-visual-studio-2017-15-7-preview-1/)

On most Linux distributions, there is a dedicated package called `clang-format`.

Most modern IDEs provide `clang-format` integration via either an extension
or directly.

A `.clang-format` file which defines our specific code style lives in the
top level directory of the repository. The style is identical to the Google
style.

## FAQ

### What's the difference between `bootstrap-orbit.{sh,ps1}` and `build.{sh,ps1}`?

`bootstrap-orbit.{sh,ps1}` performs all the tasks which have to be done once per developer machine.
This includes:
* Installing system dependencies
* Installing conan if necessary.

Afterwards `bootstrap-orbit.{sh,ps1}` calls `build.{sh,ps1}`.

`build.{sh,ps1}` on the other hand performs all the tasks which have to be done
once per build configuration. It creates a build directory, named after the
given conan profile, installs the conan-managed dependencies into this build folder,
calls `cmake` for build configuration and starts the build.

Whenever the dependencies change you have to call `build.{sh,ps1}` again.
A dependency change might be introduced by a pull from upstream or by a switch
to a different branch.

`build.{sh,ps1}` can initialize as many build configurations as you like from the
same invocation. Just pass conan profile names as command line arguments. Example for Linux:
```bash
./build.sh clang7_debug gcc9_release clang9_relwithdebinfo ggp_release
# is equivalent to
./build.sh clang7_debug
./build.sh gcc9_release
./build.sh clang9_relwithdebinfo
./build.sh ggp_release
```

### Calling `build.{sh,ps1}` after every one-line-change takes forever! What should I do?

`build.{sh,ps1}` is not meant for incremental builds. It should be called only once to initialize
a build directory. (Check out the previous section for more information on what `build.{sh,ps1}`
does.)

For incremental builds, switch to the build directory and ask cmake to run the build:
```bash
cd <build_folder>/
cmake --build . # On Linux
cmake --build . --config {Release,RelWithDebInfo,Debug} # On Windows
```

Alternatively, you can also just call `make` on Linux. Check out the next section on how to
enable building with `ninja`.

### How do I enable `ninja` for my build?

> **Note:** Linux only for now! On Windows you have to use MSBuild.

If you want to use `ninja`, you cannot rely on the `build.sh` script, which automatically
initializes your build with `make` and that cannot be changed easily later on.
So create a build directory from scratch, install the conan-managed dependencies
and invoke `cmake` manually. Here is how it works:

Let's assume you want to build Orbit in debug mode with `clang-9`:
```bash
mkdir build_clang9_debug # Create a build directory; should not exist before.
cd build_clang9_debug/
conan install -pr clang9_debug ../ # Install conan-managed dependencies
cp ../contrib/toolchains/toolchain-linux-clang9-debug.cmake toolchain.cmake # Copy the cmake toolchain file, which matches the conan profile.
cmake -DCMAKE_TOOLCHAIN_FILE=toolchain.cmake -G Ninja ../
ninja
```

> ### Note:
> Please be aware that it is your responsibility to ensure that the conan profile is compatible
> with the toolchain parameters cmake uses. In this example clang9 in debug mode is used in both cases.

#### Another example without toolchain files:
You can also manually pass the toolchain options to cmake via the command line:
```bash
mkdir build_clang9_debug # Create a build directory; should not exist before.
cd build_clang9_debug/
conan install -pr clang9_debug ../ # Install conan-managed dependencies
cmake -DCMAKE_CXX_COMPILER=clang++-9 -DCMAKE_C_COMPILER=clang-9 -DCMAKE_BUILD_TYPE=Debug -G Ninja ../
ninja
```

### How do I integrate with CLion?

In CLion, the IDE itself manages your build configurations, which is incompatible with our `build.{sh,ps1}`
script. That means, you have to manually install conan dependencies into CLion's build directories
and you have to manually verify that the directory's build configuration matches the used conan profile
in terms of compiler, standard library, build type, target platform, etc.

```bash
cd build_directory_created_by_clion/
conan install -pr matching_conan_profile ..
```

After that, you can trigger a rerun of `cmake` from CLion and it will now be able to pick up all the missing
dependencies.

This process can be automated by the [CLion Conan Extension](https://plugins.jetbrains.com/plugin/11956-conan).

After installing the extension, it will take care of installing conan dependencies into your
CLion build directories, whenever necessary. The only task left to you is to create a mapping
between CLion build configurations and conan profiles. Check out
[this blog post](https://blog.jetbrains.com/clion/2019/05/getting-started-with-the-conan-clion-plugin)
on how to do it.

### How do I integrate conan with Visual Studio?

Visual Studio has this concept of having multiple build configurations in the same build directory.
This concept is not very wide-spread on buildsystems coming from the Unix world. Both CMake and
Conan have support for it, but some of our dependencies currently do not support it.

That means, **currently you can't have debug and release builds in the same build folder!**
Please ensure that Visual Studio is set to the build configuration which matches your build-
folder's conan profile. Non-matching build configurations will result in a lot of linker errors.

There is a (Conan Extension for Visual Studio)[https://marketplace.visualstudio.com/items?itemName=conan-io.conan-vs-extension],
which is currently under development, but should be able to help you, when you develop on
Visual Studio.

### The build worked fine, but when I try to call cmake manually I get `cmake not found!`

Conan installs cmake as a build dependency automatically, but won't make it available in the PATH.

If you want to use conan's cmake installation, you can use the `virtualenv` generator to create
a virtual environment which has `cmake` in its PATH:

```bash
cd my_build_folder/
conan install -pr my_conan_profile -g virtualenv ../
source ./activate.sh # On Linux (bash) or on Windows in git-bash
.\activate.bat # On Windows in cmd
.\activate.ps1 # On Windows in powershell
cmake ... # CMake from conan is now available
```

There is also a `deactivate.{sh,bat,ps1}` which make your shell leave the virtual environment.

### `ERROR: .../orbitprofiler/conanfile.py: 'options.ggp' doesn't exist` ?!?

This message or a similar one indicates that your build profiles are
outdated and need to be updated. You can either just call the bootstrap
script again or you can manually update your conan config:
```bash
conan config install contrib/conan/configs/[windows,linux]
```

### How can I use separate debugging symbols for Linux binaries?

Orbit supports loading symbols from your workstation. Simply add directories that contain debugging symbols to the `SymbolPaths.txt` file. This file can be found at
* Windows: `C:\Users\<user>\AppData\Roaming\OrbitProfiler\config\SymbolPaths.txt`
* Linux: `~/orbitprofiler/config/SymbolPaths.txt`

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
