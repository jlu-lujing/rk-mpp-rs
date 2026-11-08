get_filename_component(_rk_mpp_rs_prefix
    "${CMAKE_CURRENT_LIST_DIR}/../../.." ABSOLUTE)

if(NOT TARGET rk_mpp_rs::rk_mpp_rs)
    add_library(rk_mpp_rs::rk_mpp_rs SHARED IMPORTED)
    set_target_properties(rk_mpp_rs::rk_mpp_rs PROPERTIES
        IMPORTED_LOCATION
            "${_rk_mpp_rs_prefix}/lib/librk_mpp_rs.so.1"
        INTERFACE_INCLUDE_DIRECTORIES
            "${_rk_mpp_rs_prefix}/include")
endif()

if(NOT TARGET rk_mpp_rs::rk_mpp_rs_static)
    add_library(rk_mpp_rs::rk_mpp_rs_static STATIC IMPORTED)
    set_target_properties(rk_mpp_rs::rk_mpp_rs_static PROPERTIES
        IMPORTED_LOCATION
            "${_rk_mpp_rs_prefix}/lib/librk_mpp_rs.a"
        INTERFACE_INCLUDE_DIRECTORIES
            "${_rk_mpp_rs_prefix}/include"
        INTERFACE_LINK_LIBRARIES "dl;pthread;m")
endif()

set(rk-mpp-rs_FOUND TRUE)
set(rk-mpp-rs_VERSION "@VERSION@")
