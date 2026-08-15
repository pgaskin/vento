# find_package(JPEG), answered with the libjpeg-turbo added as a subdirectory
# beside it rather than with whatever is on the host. LibVNCServer asks for
# JPEG_INCLUDE_DIR as a single directory (it greps jpeglib.h for JCS_EXT_RGB to
# tell libjpeg-turbo from libjpeg), so this names the source tree; jconfig.h,
# which is generated, is on the include path already.

set(JPEG_FOUND TRUE)
set(JPEG_INCLUDE_DIR "${THIRD_PARTY}/libjpeg-turbo/src")
set(JPEG_INCLUDE_DIRS "${JPEG_INCLUDE_DIR}")
set(JPEG_LIBRARY jpeg-static)
set(JPEG_LIBRARIES jpeg-static)
