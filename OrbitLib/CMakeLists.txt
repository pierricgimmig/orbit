
add_library(OrbitLib SHARED
    OrbitLib.h
    OrbitLib.cpp
)

add_definitions(-DORBIT_LIB_API_EXPORTS)

target_include_directories(OrbitLib
PUBLIC
    ${CMAKE_CURRENT_SOURCE_DIR}
)

target_link_libraries(OrbitLib
    OrbitAsm
    OrbitCore
)
 
 