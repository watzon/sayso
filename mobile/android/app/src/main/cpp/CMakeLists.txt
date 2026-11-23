# Built only with -PsaysoNative=true (see app/build.gradle.kts).
# SAYSO_CORE_DIR holds <abi>/libsayso_core.so built from mobile/core.
cmake_minimum_required(VERSION 3.22.1)
project(sayso_jni C)

if(NOT SAYSO_CORE_DIR)
  message(FATAL_ERROR "Set SAYSO_CORE_DIR to the folder with <abi>/libsayso_core.so")
endif()

add_library(sayso_core SHARED IMPORTED)
set_target_properties(sayso_core PROPERTIES
  IMPORTED_LOCATION ${SAYSO_CORE_DIR}/${ANDROID_ABI}/libsayso_core.so)

add_library(sayso_jni SHARED sayso_jni.c)
target_link_libraries(sayso_jni sayso_core log)
