import os
import shutil
import subprocess
import sys
import argparse

BUILD_DIR = "build"
VCPKG_DIR = "C:/git/vcpkg"
VCPKG_TRIPLET = "x64-windows-static"
VCPKG_INSTALL_COMMAND = f"..\vcpkg\vcpkg.exe install --triplet {VCPKG_TRIPLET}"
CMAKE_CONFIGURE_COMMAND = f"cmake .. -DCMAKE_TOOLCHAIN_FILE={VCPKG_DIR}/scripts/buildsystems/vcpkg.cmake -DVCPKG_TARGET_TRIPLET={VCPKG_TRIPLET}"
GIT_SUBMODULE_INIT_COMMAND = "git submodule update --init --recursive"

def conan_profile_exists(profile):
    result = subprocess.run(["conan", "profile", "show", profile], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    return result.returncode == 0

def check_for_qt():
    qt5_dir = os.environ.get("Qt5_DIR")
    if not qt5_dir:
        print("\nQt5_DIR environment variable not set - Please provide path to Qt distribution\n")
        print("=============================================================================\n")
        print("Orbit depends on the Qt framework which has to be installed separately either using the official installer,\n")
        print("compiled manually from source or installed using a third party installer like aqtinstall.\n")
        print("Check out DEVELOPMENT.md for more details on how to use an official Qt distribution.\n")
        print("Press Ctrl+C to stop here and install Qt first. Make sure the Qt5_DIR environment variable is pointing to your\n") 
        print("Qt installation. Then call this script again.\n")
        sys.exit(1)
    print(f"Qt5_DIR environment variable set to: {qt5_dir}")

def check_prerequisites():
    check_for_qt()

def clean():
    if os.path.exists(BUILD_DIR):
        print(f'Removing "{BUILD_DIR}" directory')
        shutil.rmtree(BUILD_DIR)
    else:
        print(f'"{BUILD_DIR}" directory does not exist, nothing to clean.')

def cmake_configure():
    print("Configuring CMake...")
    result = subprocess.run(CMAKE_CONFIGURE_COMMAND)
    if result.returncode != 0:
        print("Error running cmake.")
        exit(1)

def build():
    print("Building...")
    result = subprocess.run(["cmake", "--build", "."])
    if result.returncode != 0:
        print("Error running cmake --build .")
        exit(1)

def package():
    print("Packaging...")

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description="Build Orbit")
    parser.add_argument("config", type=str, help="Configuration string, example msvc2019_release_x64")
    parser.add_argument("--clean", action="store_true", help="Clean the build")
    parser.add_argument("--service_only", action="store_true", help="Only build OrbitService")
    parser.add_argument("--dependencies_only", action="store_true", help="Only build dependencies")
    parser.add_argument("--lib_only", action="store_true", help="Only build liborbit.so")
    parser.add_argument("--package", action="store_true", help="Package Orbit for distribution")
    parser.add_argument("--package_deps", action="store_true", help="Package Orbit dependencies for distribution")
    
    args = parser.parse_args()

    # Check for prerequisites.
    check_prerequisites()

    # Delete the build directory if --clean was specified.
    if args.clean:
        clean()

    # If the build directory does not exist, create it and configure CMake.
    needs_configure = False
    if not os.path.exists(BUILD_DIR):
        os.makedirs(BUILD_DIR)
        needs_configure = True

    # Change to the build directory.
    os.chdir(BUILD_DIR)

    # Configure if needed.
    if needs_configure:
        cmake_configure()

    # Build.
    build()

    # Package.
    if args.package:
        package()

# TODO:
# - Download dependencies from server, unzib and overlay on vcpkg directory 
# - Git submodules init and update
# - Docker containers for building dependencies and uploading them to a server

# Only build subset of LLVM:
# PS C:\git\llvm-project\llvm\build (main)
#> cmake --build . --target LLVMDebugInfoPDB --config RelWithDebInfo
# cmake --build . --target LLVMDebugInfoPDB LLVMDebugInfoDWARF LLVMDebugInfoPDB LLVMObject LLVMSymbolize --config RelWithDebInfo
# cmake --build . --target LLVMHeaders --config RelWithDebInfo // not working