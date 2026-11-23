package app.sayso.mobile.asr

// Builds without the sherpa-onnx AAR have no local engine; voice uses the
// system recognizer or the cloud.
fun createLocalAsr(): LocalAsr? = null

const val LOCAL_ASR_BUILT = false
