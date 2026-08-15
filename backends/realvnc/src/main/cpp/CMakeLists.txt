cmake_minimum_required(VERSION 3.30)
project(remotedesktop_realvnc C)

# The one native thing this module has. It links against nothing: the viewer
# core is already in the process, and what this reads through is a pointer their
# own Java handed us rather than a symbol of theirs.
include("${CMAKE_CURRENT_SOURCE_DIR}/../../../../cmake/reproducible.cmake")

add_library(remotedesktop_realvnc SHARED realvnc_traffic.c)
target_compile_options(remotedesktop_realvnc PRIVATE -Wall -Wextra)
