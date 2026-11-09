cmake_minimum_required(VERSION 3.18)
project(rk_mpp_consumer LANGUAGES CXX)

find_package(rk-mpp-rs 1.12 CONFIG REQUIRED)

add_executable(rk_mpp_cmake_consumer main.cpp)
target_compile_features(rk_mpp_cmake_consumer PRIVATE cxx_std_17)
target_link_libraries(rk_mpp_cmake_consumer PRIVATE rk_mpp_rs::rk_mpp_rs)

add_executable(rk_mpp_cmake_consumer_static main.cpp)
target_compile_features(rk_mpp_cmake_consumer_static PRIVATE cxx_std_17)
target_link_libraries(rk_mpp_cmake_consumer_static
    PRIVATE rk_mpp_rs::rk_mpp_rs_static)
