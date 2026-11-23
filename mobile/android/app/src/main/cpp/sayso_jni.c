// JNI glue between SaysoCore (Kotlin, src/native) and the C ABI of
// libsayso_core.so (mobile/core/capi/sayso_core.h). Thin on purpose: the
// keyboard's results go into arrays the Kotlin side owns and reuses, through
// stack buffers here, so those calls create no Java objects. version(),
// lastError() and the voice calls (not on the typing path) return new ones.
#include <jni.h>
#include <stdlib.h>
#include <string.h>

#include "sayso_core.h"

#define JNI_FN(name) JNICALL Java_app_sayso_mobile_engine_SaysoCore_##name

enum { MAX_KEYS = 96, MAX_CANDS = 8, MAX_SUGS = 8, ACTION_INTS = 5 };

JNIEXPORT jint JNI_OnLoad(JavaVM *vm, void *reserved) {
    (void)vm;
    (void)reserved;
    return JNI_VERSION_1_6;
}

// Bytes of a Java byte[] for the length of one call.
typedef struct {
    jbyteArray array;
    jbyte *p;
    jsize n;
} bytes;

static int bytes_get(JNIEnv *env, jbyteArray array, bytes *b) {
    b->array = array;
    b->n = array ? (*env)->GetArrayLength(env, array) : 0;
    b->p = b->n > 0 ? (*env)->GetByteArrayElements(env, array, NULL) : NULL;
    return b->n == 0 || b->p != NULL;
}

static void bytes_release(JNIEnv *env, bytes *b) {
    if (b->p) (*env)->ReleaseByteArrayElements(env, b->array, b->p, JNI_ABORT);
}

static jsize min_len(JNIEnv *env, jarray array, jsize n) {
    jsize len = (*env)->GetArrayLength(env, array);
    return n < len ? n : len;
}

// Copies an action and its text into the caller's arrays.
static jint action_out(JNIEnv *env, int32_t rc, const sayso_action *a, const char *buf, jintArray ints, jbyteArray text) {
    if (rc != SAYSO_OK) return rc;
    jint v[ACTION_INTS] = {a->kind, a->delete_chars, a->delete_utf16, a->text_len, (jint)a->flags};
    jsize n = min_len(env, text, a->text_len);
    v[3] = n;
    (*env)->SetIntArrayRegion(env, ints, 0, ACTION_INTS, v);
    if (n > 0) (*env)->SetByteArrayRegion(env, text, 0, n, (const jbyte *)buf);
    return rc;
}

JNIEXPORT jstring JNI_FN(version)(JNIEnv *env, jclass cls) {
    (void)cls;
    return (*env)->NewStringUTF(env, sayso_version());
}

JNIEXPORT jstring JNI_FN(lastError)(JNIEnv *env, jclass cls, jlong h) {
    (void)cls;
    char buf[512];
    int32_t n = sayso_last_error((sayso_engine)h, buf, sizeof buf - 1);
    buf[n < (int32_t)sizeof buf - 1 ? n : (int32_t)sizeof buf - 1] = 0;
    return (*env)->NewStringUTF(env, buf);
}

JNIEXPORT jlong JNI_FN(create)(JNIEnv *env, jclass cls, jbyteArray model, jbyteArray journal) {
    (void)cls;
    bytes m, j;
    if (!bytes_get(env, model, &m)) return SAYSO_ERR_ARG;
    if (!bytes_get(env, journal, &j)) {
        bytes_release(env, &m);
        return SAYSO_ERR_ARG;
    }
    sayso_engine e = 0;
    int32_t rc = sayso_engine_create((const char *)m.p, m.n, (const char *)j.p, j.n, &e);
    bytes_release(env, &j);
    bytes_release(env, &m);
    return rc == SAYSO_OK ? (jlong)e : rc;
}

JNIEXPORT jlong JNI_FN(createFd)(JNIEnv *env, jclass cls, jint fd, jlong offset, jlong length, jbyteArray journal) {
    (void)cls;
    bytes j;
    if (!bytes_get(env, journal, &j)) return SAYSO_ERR_ARG;
    sayso_engine e = 0;
    int32_t rc = sayso_engine_create_fd(fd, offset, length, (const char *)j.p, j.n, &e);
    bytes_release(env, &j);
    return rc == SAYSO_OK ? (jlong)e : rc;
}

JNIEXPORT void JNI_FN(destroy)(JNIEnv *env, jclass cls, jlong h) {
    (void)env;
    (void)cls;
    sayso_engine_destroy((sayso_engine)h);
}

JNIEXPORT jint JNI_FN(setLayout)(JNIEnv *env, jclass cls, jlong h, jintArray codes, jfloatArray rects, jint count) {
    (void)cls;
    if (count <= 0 || count > MAX_KEYS || (*env)->GetArrayLength(env, codes) < count ||
        (*env)->GetArrayLength(env, rects) < 4 * count) {
        return SAYSO_ERR_ARG;
    }
    jint c[MAX_KEYS];
    jfloat r[4 * MAX_KEYS];
    (*env)->GetIntArrayRegion(env, codes, 0, count, c);
    (*env)->GetFloatArrayRegion(env, rects, 0, 4 * count, r);
    sayso_key keys[MAX_KEYS];
    for (jint i = 0; i < count; i++) {
        keys[i] = (sayso_key){c[i], r[4 * i], r[4 * i + 1], r[4 * i + 2], r[4 * i + 3]};
    }
    return sayso_set_layout((sayso_engine)h, keys, count);
}

JNIEXPORT jint JNI_FN(setEditorFlags)(JNIEnv *env, jclass cls, jlong h, jint flags) {
    (void)env;
    (void)cls;
    return sayso_set_editor_flags((sayso_engine)h, (uint32_t)flags);
}

JNIEXPORT jint JNI_FN(setPreferences)(JNIEnv *env, jclass cls, jlong h, jint autocorrect, jboolean predictions) {
    (void)env;
    (void)cls;
    return sayso_set_preferences((sayso_engine)h, autocorrect, predictions ? 1 : 0);
}

// One call taking one byte[] argument.
typedef int32_t (*text_fn)(sayso_engine, const char *, int32_t);

static jint with_text(JNIEnv *env, jlong h, jbyteArray text, text_fn fn) {
    bytes b;
    if (!bytes_get(env, text, &b)) return SAYSO_ERR_ARG;
    int32_t rc = fn((sayso_engine)h, (const char *)b.p, b.n);
    bytes_release(env, &b);
    return rc;
}

JNIEXPORT jint JNI_FN(setContext)(JNIEnv *env, jclass cls, jlong h, jbyteArray text) {
    (void)cls;
    return with_text(env, h, text, sayso_set_context);
}

JNIEXPORT jint JNI_FN(onTouch)(JNIEnv *env, jclass cls, jlong h, jfloat x, jfloat y, jlong time_ms, jboolean shifted) {
    (void)env;
    (void)cls;
    sayso_touch_result r;
    if (sayso_on_touch((sayso_engine)h, x, y, time_ms, shifted ? 1 : 0, &r) != SAYSO_OK) return 0;
    return (r.flags & SAYSO_TOUCH_LETTER) ? r.code : 0;
}

JNIEXPORT jint JNI_FN(typeChar)(JNIEnv *env, jclass cls, jlong h, jint cp, jintArray ints, jbyteArray text) {
    (void)cls;
    sayso_action a;
    char buf[SAYSO_TEXT_CAP];
    return action_out(env, sayso_type_char((sayso_engine)h, cp, &a, buf, sizeof buf), &a, buf, ints, text);
}

JNIEXPORT jint JNI_FN(backspace)(JNIEnv *env, jclass cls, jlong h, jintArray ints, jbyteArray text) {
    (void)cls;
    sayso_action a;
    char buf[SAYSO_TEXT_CAP];
    return action_out(env, sayso_backspace((sayso_engine)h, &a, buf, sizeof buf), &a, buf, ints, text);
}

JNIEXPORT jint JNI_FN(revertLastAutocorrect)(JNIEnv *env, jclass cls, jlong h, jintArray ints, jbyteArray text) {
    (void)cls;
    sayso_action a;
    char buf[SAYSO_TEXT_CAP];
    return action_out(env, sayso_revert_last_autocorrect((sayso_engine)h, &a, buf, sizeof buf), &a, buf, ints, text);
}

JNIEXPORT jint JNI_FN(pickSuggestion)(JNIEnv *env, jclass cls, jlong h, jint index, jintArray ints, jbyteArray text) {
    (void)cls;
    sayso_action a;
    char buf[SAYSO_TEXT_CAP];
    return action_out(env, sayso_pick_suggestion((sayso_engine)h, index, &a, buf, sizeof buf), &a, buf, ints, text);
}

JNIEXPORT jint JNI_FN(suggestions)(JNIEnv *env, jclass cls, jlong h, jint max, jintArray ints, jfloatArray floats, jbyteArray text) {
    (void)cls;
    if (max > MAX_SUGS) max = MAX_SUGS;
    if (max > (*env)->GetArrayLength(env, floats)) max = (*env)->GetArrayLength(env, floats);
    if (max > (*env)->GetArrayLength(env, ints) / 3) max = (*env)->GetArrayLength(env, ints) / 3;
    if (max <= 0) return SAYSO_ERR_ARG;
    sayso_suggestion s[MAX_SUGS];
    char buf[SAYSO_TEXT_CAP];
    jsize cap = min_len(env, text, sizeof buf);
    int32_t n = sayso_get_suggestions((sayso_engine)h, s, max, buf, cap);
    if (n <= 0) return n;
    jint iv[3 * MAX_SUGS];
    jfloat fv[MAX_SUGS];
    jsize used = 0;
    for (int32_t i = 0; i < n; i++) {
        iv[3 * i] = s[i].text_offset;
        iv[3 * i + 1] = s[i].text_len;
        iv[3 * i + 2] = (jint)s[i].flags;
        fv[i] = s[i].score;
        used = s[i].text_offset + s[i].text_len;
    }
    (*env)->SetIntArrayRegion(env, ints, 0, 3 * n, iv);
    (*env)->SetFloatArrayRegion(env, floats, 0, n, fv);
    if (used > 0) (*env)->SetByteArrayRegion(env, text, 0, used, (const jbyte *)buf);
    return n;
}

JNIEXPORT jint JNI_FN(currentWord)(JNIEnv *env, jclass cls, jlong h, jbyteArray text) {
    (void)cls;
    char buf[SAYSO_TEXT_CAP];
    int32_t n = sayso_current_word((sayso_engine)h, buf, sizeof buf);
    if (n > 0) {
        jsize w = min_len(env, text, n < (int32_t)sizeof buf ? n : (int32_t)sizeof buf);
        (*env)->SetByteArrayRegion(env, text, 0, w, (const jbyte *)buf);
    }
    return n;
}

JNIEXPORT jint JNI_FN(addWord)(JNIEnv *env, jclass cls, jlong h, jbyteArray word) {
    (void)cls;
    return with_text(env, h, word, sayso_add_word);
}

JNIEXPORT jint JNI_FN(removeWord)(JNIEnv *env, jclass cls, jlong h, jbyteArray word) {
    (void)cls;
    return with_text(env, h, word, sayso_remove_word);
}

JNIEXPORT jint JNI_FN(addReplacement)(JNIEnv *env, jclass cls, jlong h, jbyteArray from, jbyteArray to, jboolean case_sensitive) {
    (void)cls;
    bytes f, t;
    if (!bytes_get(env, from, &f)) return SAYSO_ERR_ARG;
    if (!bytes_get(env, to, &t)) {
        bytes_release(env, &f);
        return SAYSO_ERR_ARG;
    }
    int32_t rc = sayso_add_replacement((sayso_engine)h, (const char *)f.p, f.n, (const char *)t.p, t.n, case_sensitive ? 1 : 0);
    bytes_release(env, &t);
    bytes_release(env, &f);
    return rc;
}

JNIEXPORT jint JNI_FN(removeReplacement)(JNIEnv *env, jclass cls, jlong h, jbyteArray from) {
    (void)cls;
    return with_text(env, h, from, sayso_remove_replacement);
}

JNIEXPORT jint JNI_FN(resetLearned)(JNIEnv *env, jclass cls, jlong h) {
    (void)env;
    (void)cls;
    return sayso_reset_learned((sayso_engine)h);
}

// Fills a byte[] (or sizes the list when it is null) through fn.
static jint list_into(JNIEnv *env, jbyteArray out, int32_t (*fn)(sayso_engine, uint32_t, char *, int32_t), sayso_engine e, uint32_t which) {
    if (!out) return fn(e, which, NULL, 0);
    jsize cap = (*env)->GetArrayLength(env, out);
    jbyte *p = (*env)->GetByteArrayElements(env, out, NULL);
    if (!p) return SAYSO_ERR_ARG;
    int32_t n = fn(e, which, (char *)p, cap);
    (*env)->ReleaseByteArrayElements(env, out, p, 0);
    return n;
}

static int32_t list_replacements(sayso_engine e, uint32_t which, char *buf, int32_t cap) {
    (void)which;
    return sayso_list_replacements(e, buf, cap);
}

JNIEXPORT jint JNI_FN(listWords)(JNIEnv *env, jclass cls, jlong h, jint which, jbyteArray out) {
    (void)cls;
    return list_into(env, out, sayso_list_words, (sayso_engine)h, (uint32_t)which);
}

JNIEXPORT jint JNI_FN(listReplacements)(JNIEnv *env, jclass cls, jlong h, jbyteArray out) {
    (void)cls;
    return list_into(env, out, list_replacements, (sayso_engine)h, 0);
}

// Layout of ints and floats: see SaysoCore.kt.
enum { D_KEYS = 8, D_CANDS = D_KEYS + 2 * MAX_KEYS, DIAG_INTS = D_CANDS + 4 * MAX_CANDS };
enum { DF_KEYS = 2, DF_CANDS = DF_KEYS + 3 * MAX_KEYS, DIAG_FLOATS = DF_CANDS + 3 * MAX_CANDS };

JNIEXPORT jint JNI_FN(diagnostics)(JNIEnv *env, jclass cls, jlong h, jintArray ints, jfloatArray floats, jbyteArray text,
                                   jint key_cap, jint cand_cap) {
    (void)cls;
    if ((*env)->GetArrayLength(env, ints) < DIAG_INTS || (*env)->GetArrayLength(env, floats) < DIAG_FLOATS) return SAYSO_ERR_ARG;
    if (key_cap < 0 || key_cap > MAX_KEYS) key_cap = MAX_KEYS;
    if (cand_cap < 0 || cand_cap > MAX_CANDS) cand_cap = MAX_CANDS;
    sayso_diag d;
    sayso_key_diag kd[MAX_KEYS];
    sayso_cand_diag cd[MAX_CANDS];
    char buf[SAYSO_TEXT_CAP];
    jsize cap = min_len(env, text, sizeof buf);
    int32_t rc = sayso_get_diagnostics((sayso_engine)h, &d, kd, key_cap, cd, cand_cap, buf, cap);
    if (rc != SAYSO_OK) return rc;
    jint iv[DIAG_INTS];
    jfloat fv[DIAG_FLOATS];
    memset(iv, 0, sizeof iv);
    memset(fv, 0, sizeof fv);
    iv[0] = d.would_correct;
    iv[1] = d.literal_is_word;
    iv[2] = d.key_count;
    iv[3] = d.cand_count;
    iv[4] = d.last_touch.code;
    iv[5] = d.last_touch.key_index;
    iv[6] = (jint)d.last_touch.flags;
    fv[0] = d.margin;
    fv[1] = d.literal_total;
    for (int32_t i = 0; i < d.key_count; i++) {
        iv[D_KEYS + 2 * i] = kd[i].code;
        iv[D_KEYS + 2 * i + 1] = kd[i].samples;
        fv[DF_KEYS + 3 * i] = kd[i].expansion;
        fv[DF_KEYS + 3 * i + 1] = kd[i].dx;
        fv[DF_KEYS + 3 * i + 2] = kd[i].dy;
    }
    jsize used = 0;
    for (int32_t i = 0; i < d.cand_count; i++) {
        iv[D_CANDS + 4 * i] = cd[i].text_offset;
        iv[D_CANDS + 4 * i + 1] = cd[i].text_len;
        iv[D_CANDS + 4 * i + 2] = cd[i].edits;
        iv[D_CANDS + 4 * i + 3] = cd[i].kind;
        fv[DF_CANDS + 3 * i] = cd[i].total;
        fv[DF_CANDS + 3 * i + 1] = cd[i].spatial;
        fv[DF_CANDS + 3 * i + 2] = cd[i].lm;
        used = cd[i].text_offset + cd[i].text_len;
    }
    (*env)->SetIntArrayRegion(env, ints, 0, DIAG_INTS, iv);
    (*env)->SetFloatArrayRegion(env, floats, 0, DIAG_FLOATS, fv);
    if (used > 0) (*env)->SetByteArrayRegion(env, text, 0, used, (const jbyte *)buf);
    return SAYSO_OK;
}

// ---- Voice ----

// A new byte[] holding n bytes of p, or NULL.
static jbyteArray new_bytes(JNIEnv *env, const char *p, jsize n) {
    jbyteArray a = (*env)->NewByteArray(env, n);
    if (a && n > 0) (*env)->SetByteArrayRegion(env, a, 0, n, (const jbyte *)p);
    return a;
}

JNIEXPORT jbyteArray JNI_FN(postProcess)(JNIEnv *env, jclass cls, jlong h, jbyteArray raw, jbyteArray before, jbyteArray after) {
    (void)cls;
    bytes r, b, a;
    if (!bytes_get(env, raw, &r)) return NULL;
    if (!bytes_get(env, before, &b)) {
        bytes_release(env, &r);
        return NULL;
    }
    if (!bytes_get(env, after, &a)) {
        bytes_release(env, &b);
        bytes_release(env, &r);
        return NULL;
    }
    char stack[4096];
    char *buf = stack;
    int32_t cap = sizeof stack;
    int32_t n = sayso_post_process_transcript((sayso_engine)h, (const char *)r.p, r.n, (const char *)b.p, b.n, (const char *)a.p, a.n, buf, cap);
    if (n > cap && (buf = malloc(n)) != NULL) {
        cap = n;
        n = sayso_post_process_transcript((sayso_engine)h, (const char *)r.p, r.n, (const char *)b.p, b.n, (const char *)a.p, a.n, buf, cap);
    }
    bytes_release(env, &a);
    bytes_release(env, &b);
    bytes_release(env, &r);
    jbyteArray out = (buf != NULL && n >= 0 && n <= cap) ? new_bytes(env, buf, n) : NULL;
    if (buf != stack) free(buf);
    return out;
}

static int32_t list_styles(sayso_engine e, uint32_t which, char *buf, int32_t cap) {
    (void)which;
    return sayso_list_styles(e, buf, cap);
}

JNIEXPORT jint JNI_FN(listStyles)(JNIEnv *env, jclass cls, jlong h, jbyteArray out) {
    (void)cls;
    return list_into(env, out, list_styles, (sayso_engine)h, 0);
}

JNIEXPORT jint JNI_FN(putStyle)(JNIEnv *env, jclass cls, jlong h, jbyteArray id, jbyteArray name, jbyteArray prompt) {
    (void)cls;
    bytes i, n, p;
    if (!bytes_get(env, id, &i)) return SAYSO_ERR_ARG;
    if (!bytes_get(env, name, &n)) {
        bytes_release(env, &i);
        return SAYSO_ERR_ARG;
    }
    if (!bytes_get(env, prompt, &p)) {
        bytes_release(env, &n);
        bytes_release(env, &i);
        return SAYSO_ERR_ARG;
    }
    int32_t rc = sayso_put_style((sayso_engine)h, (const char *)i.p, i.n, (const char *)n.p, n.n, (const char *)p.p, p.n);
    bytes_release(env, &p);
    bytes_release(env, &n);
    bytes_release(env, &i);
    return rc;
}

JNIEXPORT jint JNI_FN(removeStyle)(JNIEnv *env, jclass cls, jlong h, jbyteArray id) {
    (void)cls;
    return with_text(env, h, id, sayso_remove_style);
}

// The parts of a sayso_cloud, held for one call.
typedef struct {
    bytes key, base, model;
    sayso_cloud c;
} cloud_args;

static int cloud_get(JNIEnv *env, jint provider, jbyteArray key, jbyteArray base, jbyteArray model, jint timeout_ms, cloud_args *a) {
    memset(a, 0, sizeof *a);
    if (!bytes_get(env, key, &a->key)) return 0;
    if (!bytes_get(env, base, &a->base)) {
        bytes_release(env, &a->key);
        return 0;
    }
    if (!bytes_get(env, model, &a->model)) {
        bytes_release(env, &a->base);
        bytes_release(env, &a->key);
        return 0;
    }
    a->c = (sayso_cloud){provider, (const char *)a->key.p, a->key.n, (const char *)a->base.p, a->base.n,
                         (const char *)a->model.p, a->model.n, timeout_ms};
    return 1;
}

static void cloud_release(JNIEnv *env, cloud_args *a) {
    bytes_release(env, &a->model);
    bytes_release(env, &a->base);
    bytes_release(env, &a->key);
}

// result[0] = the text, result[1] = the error (null when empty).
static void voice_result(JNIEnv *env, jobjectArray result, const char *text, int32_t len, const char *err) {
    if (!result || (*env)->GetArrayLength(env, result) < 2) return;
    (*env)->SetObjectArrayElement(env, result, 0, text ? new_bytes(env, text, len) : NULL);
    size_t n = err ? strlen(err) : 0;
    (*env)->SetObjectArrayElement(env, result, 1, n > 0 ? new_bytes(env, err, (jsize)n) : NULL);
}

JNIEXPORT jint JNI_FN(enhance)(JNIEnv *env, jclass cls, jlong h, jint provider, jbyteArray key, jbyteArray base, jbyteArray model,
                               jint timeout_ms, jbyteArray style, jbyteArray text, jobjectArray result) {
    (void)cls;
    cloud_args a;
    if (!cloud_get(env, provider, key, base, model, timeout_ms, &a)) return SAYSO_ERR_ARG;
    bytes s, t;
    if (!bytes_get(env, style, &s)) {
        cloud_release(env, &a);
        return SAYSO_ERR_ARG;
    }
    if (!bytes_get(env, text, &t)) {
        bytes_release(env, &s);
        cloud_release(env, &a);
        return SAYSO_ERR_ARG;
    }
    char *out = malloc(SAYSO_VOICE_TEXT_CAP);
    char err[512] = {0};
    int32_t len = 0, rc = SAYSO_ERR_ARG;
    if (out) {
        rc = sayso_enhance((sayso_engine)h, &a.c, (const char *)s.p, s.n, (const char *)t.p, t.n, out, SAYSO_VOICE_TEXT_CAP, &len, err, sizeof err - 1);
    }
    bytes_release(env, &t);
    bytes_release(env, &s);
    cloud_release(env, &a);
    if (rc >= 0) voice_result(env, result, out, len, err);
    free(out);
    return rc;
}

JNIEXPORT jint JNI_FN(transcribeCloud)(JNIEnv *env, jclass cls, jlong h, jint provider, jbyteArray key, jbyteArray base, jbyteArray model,
                                       jint timeout_ms, jshortArray pcm, jobjectArray result) {
    (void)cls;
    cloud_args a;
    if (!cloud_get(env, provider, key, base, model, timeout_ms, &a)) return SAYSO_ERR_ARG;
    jsize n = pcm ? (*env)->GetArrayLength(env, pcm) : 0;
    jshort *samples = n > 0 ? (*env)->GetShortArrayElements(env, pcm, NULL) : NULL;
    char *out = malloc(SAYSO_VOICE_TEXT_CAP);
    char err[512] = {0};
    int32_t len = 0, rc = SAYSO_ERR_ARG;
    if (out && (n == 0 || samples)) {
        rc = sayso_transcribe_cloud((sayso_engine)h, &a.c, (const int16_t *)samples, n, out, SAYSO_VOICE_TEXT_CAP, &len, err, sizeof err - 1);
    }
    if (samples) (*env)->ReleaseShortArrayElements(env, pcm, samples, JNI_ABORT);
    cloud_release(env, &a);
    if (rc == SAYSO_OK) voice_result(env, result, out, len, NULL);
    if (rc == SAYSO_ERR_CLOUD) voice_result(env, result, NULL, 0, err);
    free(out);
    return rc;
}

JNIEXPORT jlong JNI_FN(historyAdd)(JNIEnv *env, jclass cls, jlong h, jlong timestamp_ms, jlong duration_ms, jbyteArray engine_id,
                                   jbyteArray style_id, jbyteArray raw, jbyteArray final_text, jboolean sensitive) {
    (void)cls;
    bytes f[4];
    jbyteArray arrays[4] = {engine_id, style_id, raw, final_text};
    int got = 0;
    for (; got < 4; got++) {
        if (!bytes_get(env, arrays[got], &f[got])) break;
    }
    int64_t id = 0;
    int32_t rc = SAYSO_ERR_ARG;
    if (got == 4) {
        rc = sayso_history_add((sayso_engine)h, timestamp_ms, duration_ms, (const char *)f[0].p, f[0].n, (const char *)f[1].p, f[1].n,
                               (const char *)f[2].p, f[2].n, (const char *)f[3].p, f[3].n, sensitive ? 1 : 0, &id);
    }
    while (got-- > 0) bytes_release(env, &f[got]);
    return rc == SAYSO_OK ? (jlong)id : rc;
}

static int32_t history_list(sayso_engine e, uint32_t limit, char *buf, int32_t cap) {
    return sayso_history_list(e, (int32_t)limit, buf, cap);
}

JNIEXPORT jint JNI_FN(historyList)(JNIEnv *env, jclass cls, jlong h, jint limit, jbyteArray out) {
    (void)cls;
    return list_into(env, out, history_list, (sayso_engine)h, limit > 0 ? (uint32_t)limit : 0);
}

JNIEXPORT jint JNI_FN(historyDelete)(JNIEnv *env, jclass cls, jlong h, jlong id) {
    (void)env;
    (void)cls;
    return sayso_history_delete((sayso_engine)h, id);
}

JNIEXPORT jint JNI_FN(historyClear)(JNIEnv *env, jclass cls, jlong h) {
    (void)env;
    (void)cls;
    return sayso_history_clear((sayso_engine)h);
}
