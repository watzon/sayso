// JNI glue between NativeTypingEngine (Kotlin) and the C ABI of libsayso_core.so.
// Add one JNIEXPORT function per external fun of NativeTypingEngine.
#include <jni.h>

JNIEXPORT jint JNI_OnLoad(JavaVM *vm, void *reserved) {
    (void)vm;
    (void)reserved;
    return JNI_VERSION_1_6;
}
