enable_language(ASM_MASM)

set(CMAKE_CXX_STANDARD 20)

set(MINHOOK_DIR ${PROJECT_SOURCE_DIR}/third_party/minhook)

set(HEADERS
    include/hijk/hijk.h
)

set(SOURCES
    hijk.asm
    hijk.cpp
)

set(HDE_FILES
    ${MINHOOK_DIR}/src/HDE/hde32.h
    ${MINHOOK_DIR}/src/HDE/hde32.c
    ${MINHOOK_DIR}/src/HDE/hde64.h
    ${MINHOOK_DIR}/src/HDE/hde64.c
    ${MINHOOK_DIR}/src/HDE/table32.h
    ${MINHOOK_DIR}/src/HDE/table64.h
    ${MINHOOK_DIR}/src/HDE/pstdint.h
)

set(MINHOOK_FILES
    ${MINHOOK_DIR}/include/MinHook.h
    ${MINHOOK_DIR}/src/buffer.h
    ${MINHOOK_DIR}/src/buffer.c
    ${MINHOOK_DIR}/src/hook.c
    ${MINHOOK_DIR}/src/trampoline.h
    ${MINHOOK_DIR}/src/trampoline.c
)

source_group("HDE" FILES ${HDE_FILES})
source_group("MinHook" FILES ${MINHOOK_FILES})
source_group("Header Files" FILES ${HEADERS})
source_group("Source Files" FILES ${SOURCES})

add_library(hijk SHARED
    ${SOURCES}
    ${HEADERS}
    ${HDE_FILES}
    ${MINHOOK_FILES}
)

target_include_directories(hijk PUBLIC
    ${MINHOOK_DIR}/include
    ${MINHOOK_DIR}/src
    ${CMAKE_SOURCE_DIR}/src/hijk
)

target_include_directories(hijk PUBLIC
    "include"
)

# Disable incremental linking
set_target_properties(hijk PROPERTIES LINK_FLAGS "/INCREMENTAL:NO")

add_executable(hijk_example main.cpp)
target_link_libraries(hijk_example hijk)
set_property(DIRECTORY ${CMAKE_CURRENT_SOURCE_DIR} PROPERTY VS_STARTUP_PROJECT hijk_example)
