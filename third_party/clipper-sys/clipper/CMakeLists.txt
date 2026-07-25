cmake_minimum_required(VERSION 3.9)
project(clipper)

add_library(clipper STATIC clipper.cpp wrapper.cpp)

install(TARGETS clipper DESTINATION .)