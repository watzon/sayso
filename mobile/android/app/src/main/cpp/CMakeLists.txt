# Built only for the native engine (see app/build.gradle.kts).
# SAYSO_CORE_DIR holds <abi>/libsayso_core.so built from mobile/core;
# SAYSO_CORE_INCLUDE holds sayso_core.h (mobile/core/capi).
cmake_minimum_required(VERSION 3.22.1)
project(sayso_jni C)

if(NOT SAYSO_CORE_DIR OR NOT SAYSO_CORE_INCLUDE)
  message(FATAL_ERROR "Set SAYSO_CORE_DIR (<abi>/libsayso_core.so) and SAYSO_CORE_INCLUDE (sayso_core.h)")
endif()

add_library(sayso_core SHARED IMPORTED)
set_target_properties(sayso_core PROPERTIES
  IMPORTED_LOCATION ${SAYSO_CORE_DIR}/${ANDROID_ABI}/libsayso_core.so)

add_library(sayso_jni SHARED sayso_jni.c)
target_include_directories(sayso_jni PRIVATE ${SAYSO_CORE_INCLUDE})
target_compile_options(sayso_jni PRIVATE -Wall -Wextra -Werror)
target_link_options(sayso_jni PRIVATE -Wl,-z,max-page-size=16384)
target_link_libraries(sayso_jni sayso_core)
