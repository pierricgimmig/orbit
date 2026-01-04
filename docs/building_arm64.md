# Building OrbitService for ARM64 (Raspberry Pi 4/5)

This document describes how to cross-compile OrbitService for ARM64 (AArch64) targets, specifically for Raspberry Pi 4 and 5.

## Prerequisites

- Docker installed on your build machine
- At least 8GB RAM recommended for cross-compilation
- Conan 2.x installed (for managing build profiles)

## Quick Start

### 1. Build the Docker Image

```bash
cd /path/to/orbit
docker build -f docker/Dockerfile.arm64-cross -t orbit-arm64-cross .
```

### 2. Cross-Compile OrbitService

```bash
docker run --rm -v $(pwd):/orbit -w /orbit orbit-arm64-cross bash -c '
  mkdir -p build_arm64
  cd build_arm64
  conan install .. \
    --profile:build=default \
    --profile:host=/orbit/conan/profiles/arm64-linux-cross \
    --output-folder=. \
    --build=missing
  cmake .. \
    -DCMAKE_TOOLCHAIN_FILE=third_party/toolchains/toolchain-linux-arm64-cross.cmake \
    -DCMAKE_BUILD_TYPE=Release \
    -G Ninja
  ninja OrbitService
'
```

### 3. Deploy to Raspberry Pi

```bash
# Copy the binary to your Raspberry Pi
scp build_arm64/bin/OrbitService pi@raspberrypi:/opt/orbitprofiler/

# If you need the user space instrumentation library (not functional on ARM64)
# scp build_arm64/lib/liborbituserspaceinstrumentation.so pi@raspberrypi:/opt/orbitprofiler/
```

## Detailed Instructions

### Build Directory Structure

After a successful cross-compilation, the build directory will contain:

```
build_arm64/
├── bin/
│   └── OrbitService          # The ARM64 binary
├── lib/
│   └── ...                   # Libraries
└── generators/
    └── ...                   # Conan-generated CMake files
```

### Running on Raspberry Pi

1. Ensure your Raspberry Pi is running a 64-bit OS (Raspberry Pi OS 64-bit recommended)

2. Create the installation directory:
   ```bash
   sudo mkdir -p /opt/orbitprofiler
   sudo chown $USER:$USER /opt/orbitprofiler
   ```

3. Copy the OrbitService binary:
   ```bash
   scp build_arm64/bin/OrbitService pi@raspberrypi:/opt/orbitprofiler/
   ```

4. Run OrbitService:
   ```bash
   ssh pi@raspberrypi
   sudo /opt/orbitprofiler/OrbitService --help
   ```

### Kernel Configuration

OrbitService uses `perf_event_open` for profiling. Ensure the following kernel settings:

```bash
# Check perf_event settings
cat /proc/sys/kernel/perf_event_paranoid

# If needed, allow unprivileged access (or run as root)
sudo sysctl -w kernel.perf_event_paranoid=-1
```

## Known Limitations

1. **UserSpaceInstrumentation**: Dynamic instrumentation (uprobes-based function hooking) is not supported on ARM64. This feature requires x86_64-specific machine code generation.

2. **GPU Tracing**: AMD GPU tracing (via amdgpu tracepoints) is not applicable on Raspberry Pi.

3. **Performance**: Cross-compiled binaries are optimized for Cortex-A72 (RPi 4). They will also work on Cortex-A76 (RPi 5) but may not use all A76-specific optimizations.

## Troubleshooting

### Conan dependency build failures

If Conan fails to build dependencies for ARM64:

1. Ensure you have enough disk space (at least 10GB free)
2. Try building with fewer parallel jobs: add `-j 2` to the ninja command
3. Check that the cross-compiler is correctly installed in the Docker image

### Missing libraries on Raspberry Pi

If OrbitService fails to start due to missing libraries:

```bash
# Check dependencies
ldd /opt/orbitprofiler/OrbitService

# Install missing libraries (example)
sudo apt install libstdc++6
```

### Permission denied for perf_event_open

```bash
# Run as root
sudo /opt/orbitprofiler/OrbitService

# Or adjust kernel settings
sudo sysctl -w kernel.perf_event_paranoid=-1
```

## Files Added/Modified for ARM64 Support

### New Files
- `docker/Dockerfile.arm64-cross` - Docker image for cross-compilation
- `conan/profiles/arm64-linux-cross` - Conan profile for ARM64
- `third_party/toolchains/toolchain-linux-arm64-cross.cmake` - CMake toolchain
- `third_party/libunwindstack/AsmGetRegsArm64.S` - ARM64 register assembly

### Modified Files
- `src/LinuxTracing/PerfEventOpen.h` - ARM64 register masks
- `src/LinuxTracing/PerfEventRecords.h` - ARM64 register structs
- `src/LinuxTracing/PerfEvent.h` - Architecture-aware constants
- `src/LinuxTracing/PerfEvent.cpp` - ARM64 register conversion
- `src/LinuxTracing/LibunwindstackUnwinder.h` - ARM64 type aliases
- `src/LinuxTracing/LibunwindstackUnwinder.cpp` - ARM64 unwinding support
- `src/UserSpaceInstrumentation/CMakeLists.txt` - Disabled on ARM64
- `third_party/libunwindstack/CMakeLists.txt` - ARM64 assembly source
