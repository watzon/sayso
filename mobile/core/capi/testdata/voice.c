/* voice.c: drives the voice part of the C ABI: post-processing, styles,
 * history, and the cloud calls against a fake server whose address is
 * argv[3] (TestCVoice runs an httptest server; nothing reaches a real
 * provider). Usage: voice model.sslm journal-dir server-url */
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include "sayso_core.h"

static int failures = 0;
#define CHECK(cond, ...)                                         \
    do {                                                         \
        if (!(cond)) {                                           \
            fprintf(stderr, "FAIL %s:%d: ", __FILE__, __LINE__); \
            fprintf(stderr, __VA_ARGS__);                        \
            fprintf(stderr, "\n");                               \
            failures++;                                          \
        }                                                        \
    } while (0)

#define S(s) s, (int32_t)strlen(s)

static const char *model;
static char journal[1024];
static char server[512];

static sayso_engine open_engine(void) {
    sayso_engine e = 0;
    int32_t rc = sayso_engine_create(S(model), S(journal), &e);
    CHECK(rc == SAYSO_OK, "create: %d", rc);
    return e;
}

static long now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec * 1000L + ts.tv_nsec / 1000000L;
}

/* Counts NUL-terminated fields in a list of n bytes. */
static int fields(const char *buf, int32_t n) {
    int k = 0;
    for (int32_t i = 0; i < n; i++) k += buf[i] == 0;
    return k;
}

/* The field i of a NUL-separated list. */
static const char *field(const char *buf, int i) {
    while (i-- > 0) buf += strlen(buf) + 1;
    return buf;
}

static void post_process(sayso_engine e) {
    CHECK(sayso_add_word(e, S("GitHub")) == SAYSO_OK, "add word");
    char out[256];
    int32_t n = sayso_post_process_transcript(e, S("push it to git hub comma please"), S("I will "), S(""), out, sizeof out);
    CHECK(n > 0 && n < (int32_t)sizeof out && strcmp(out, "push it to GitHub, please") == 0, "post-process: %d %s", n, out);
    n = sayso_post_process_transcript(e, S("hello world."), S(""), S(""), NULL, 0);
    CHECK(n == (int32_t)strlen("Hello world."), "sizing: %d", n);
    char small[4] = "xyz";
    CHECK(sayso_post_process_transcript(e, S("hello world."), S(""), S(""), small, sizeof small) == n && strcmp(small, "xyz") == 0,
          "a short buffer is not written");
}

static void styles(sayso_engine e) {
    CHECK(sayso_put_style(e, S("pirate"), S("Pirate"), S("Talk like a pirate.")) == SAYSO_OK, "put style");
    CHECK(sayso_put_style(e, S("clean"), S("Mine"), S("x")) == SAYSO_ERR_ARG, "a built-in id is refused");
    CHECK(sayso_put_style(e, S("Bad Id"), S("x"), S("")) == SAYSO_ERR_ARG, "a bad id is refused");
    CHECK(sayso_remove_style(e, S("raw")) == SAYSO_ERR_ARG, "built-in styles cannot be removed");
    int32_t need = sayso_list_styles(e, NULL, 0);
    char *buf = malloc(need);
    CHECK(sayso_list_styles(e, buf, need) == need, "list styles");
    int n = fields(buf, need) / 4;
    CHECK(n == 8, "7 built-in styles and 1 custom: %d", n);
    CHECK(strcmp(field(buf, 0), "clean") == 0 && strcmp(field(buf, 3), "1") == 0, "built-ins first: %s", field(buf, 0));
    CHECK(strcmp(field(buf, 4 * 7), "pirate") == 0 && strcmp(field(buf, 4 * 7 + 3), "0") == 0, "custom last");
    free(buf);
}

static void history(sayso_engine e) {
    int64_t id = -1;
    CHECK(sayso_history_add(e, 1700000000000LL, 1200, S("whisper-base"), S("clean"), S("hello world"), S("Hello, world."), 0, &id) == SAYSO_OK && id == 1,
          "add: id %lld", (long long)id);
    CHECK(sayso_history_add(e, 1700000001000LL, 300, S("cloud"), S("raw"), S("secret"), S("secret"), 1, &id) == SAYSO_OK && id == 0,
          "sensitive entries are skipped");
    sayso_set_editor_flags(e, SAYSO_EDITOR_PRIVACY);
    CHECK(sayso_history_add(e, 1700000002000LL, 300, S("cloud"), S("raw"), S("private"), S("private"), 0, &id) == SAYSO_OK && id == 0,
          "privacy mode skips entries");
    sayso_set_editor_flags(e, SAYSO_EDITOR_AUTOCAP);
    CHECK(sayso_history_add(e, 1700000003000LL, 900, S("whisper-base"), S("email"), S("second"), S("Second."), 0, &id) == SAYSO_OK && id == 2,
          "add second");
}

static void history_after_reopen(sayso_engine e) {
    int32_t need = sayso_history_list(e, 0, NULL, 0);
    char *buf = malloc(need);
    CHECK(sayso_history_list(e, 0, buf, need) == need && fields(buf, need) == 14, "two entries survive a reopen");
    CHECK(strcmp(field(buf, 0), "2") == 0 && strcmp(field(buf, 7), "1") == 0, "newest first");
    CHECK(strcmp(field(buf, 8), "1700000000000") == 0 && strcmp(field(buf, 9), "1200") == 0 && strcmp(field(buf, 10), "whisper-base") == 0 &&
              strcmp(field(buf, 11), "clean") == 0 && strcmp(field(buf, 12), "hello world") == 0 && strcmp(field(buf, 13), "Hello, world.") == 0,
          "fields round trip");
    free(buf);
    char one[256];
    int32_t n = sayso_history_list(e, 1, one, sizeof one);
    CHECK(fields(one, n) == 7, "limit 1");
    CHECK(sayso_history_delete(e, 2) == SAYSO_OK, "delete");
    n = sayso_history_list(e, 0, one, sizeof one);
    CHECK(fields(one, n) == 7 && strcmp(one, "1") == 0, "one left");
    CHECK(sayso_history_clear(e) == SAYSO_OK && sayso_history_list(e, 0, NULL, 0) == 0, "clear");

    int32_t sneed = sayso_list_styles(e, NULL, 0);
    char *s = malloc(sneed);
    sayso_list_styles(e, s, sneed);
    CHECK(fields(s, sneed) / 4 == 8, "the custom style survives a reopen");
    free(s);
}

static sayso_cloud cloud_cfg(int32_t provider, const char *path, int32_t timeout_ms) {
    static char base[600];
    snprintf(base, sizeof base, "%s%s", server, path);
    sayso_cloud c = {provider, "sk-test", 7, base, (int32_t)strlen(base), "test-model", 10, timeout_ms};
    return c;
}

static void cloud_calls(sayso_engine e) {
    char *out = malloc(SAYSO_VOICE_TEXT_CAP);
    char err[256];
    int32_t len = -1;

    sayso_cloud off = {SAYSO_STYLE_OFF, NULL, 0, NULL, 0, NULL, 0, 0};
    int32_t rc = sayso_enhance(e, &off, S("polished"), S("we was late"), out, SAYSO_VOICE_TEXT_CAP, &len, err, sizeof err);
    CHECK(rc == 0 && len == 11 && strncmp(out, "we was late", 11) == 0 && err[0] == 0, "enhance with the provider off: %d %s", rc, err);
    sayso_cloud c = cloud_cfg(SAYSO_STYLE_OPENAI_COMPATIBLE, "/ok", 0);
    rc = sayso_enhance(e, &c, S("raw"), S("we was late"), out, SAYSO_VOICE_TEXT_CAP, &len, err, sizeof err);
    CHECK(rc == 0 && err[0] == 0, "a style with no prompt makes no call: %d %s", rc, err);
    rc = sayso_enhance(e, &c, S("nope"), S("we was late"), out, SAYSO_VOICE_TEXT_CAP, &len, err, sizeof err);
    CHECK(rc == 0 && strstr(err, "unknown style") != NULL, "unknown style: %d %s", rc, err);
    rc = sayso_enhance(e, &c, S("polished"), S("we was late"), out, 16, &len, err, sizeof err);
    CHECK(rc == SAYSO_ERR_ARG, "a short out buffer: %d", rc);

    rc = sayso_enhance(e, &c, S("polished"), S("we was late"), out, SAYSO_VOICE_TEXT_CAP, &len, err, sizeof err);
    CHECK(rc == 1 && len == 13 && strncmp(out, "We were late.", 13) == 0, "enhance: %d %.*s %s", rc, len, out, err);

    sayso_cloud slow = cloud_cfg(SAYSO_STYLE_OPENAI_COMPATIBLE, "/slow", 200);
    long t0 = now_ms();
    rc = sayso_enhance(e, &slow, S("polished"), S("we was late"), out, SAYSO_VOICE_TEXT_CAP, &len, err, sizeof err);
    long took = now_ms() - t0;
    CHECK(rc == 0 && strncmp(out, "we was late", 11) == 0 && strstr(err, "timed out") != NULL && took < 1500,
          "timeout falls back: %d %s in %ld ms", rc, err, took);

    sayso_cloud bad = cloud_cfg(SAYSO_STYLE_OPENAI, "/fail", 0);
    rc = sayso_enhance(e, &bad, S("polished"), S("we was late"), out, SAYSO_VOICE_TEXT_CAP, &len, err, sizeof err);
    CHECK(rc == 0 && strstr(err, "401") != NULL && strstr(err, "sk-test") == NULL, "errors fall back without the key: %s", err);

    int16_t pcm[1600] = {0};
    sayso_cloud asr = cloud_cfg(SAYSO_ASR_OPENAI_COMPATIBLE, "/ok", 0);
    rc = sayso_transcribe_cloud(e, &asr, pcm, 1600, out, SAYSO_VOICE_TEXT_CAP, &len, err, sizeof err);
    CHECK(rc == SAYSO_OK && len == 11 && strncmp(out, "hello there", 11) == 0, "transcribe: %d %s", rc, err);
    sayso_cloud asr_off = {SAYSO_ASR_OFF, NULL, 0, NULL, 0, NULL, 0, 0};
    rc = sayso_transcribe_cloud(e, &asr_off, pcm, 1600, out, SAYSO_VOICE_TEXT_CAP, &len, NULL, 0);
    CHECK(rc == SAYSO_ERR_CLOUD, "transcribe with the provider off: %d", rc);
    free(out);
}

/* A slow cloud call in flight must not hold up destroy. */
static void *slow_enhance(void *arg) {
    sayso_engine e = *(sayso_engine *)arg;
    char *out = malloc(SAYSO_VOICE_TEXT_CAP);
    int32_t len;
    sayso_cloud slow = cloud_cfg(SAYSO_STYLE_OPENAI_COMPATIBLE, "/slow", 3000);
    sayso_enhance(e, &slow, S("polished"), S("we was late"), out, SAYSO_VOICE_TEXT_CAP, &len, NULL, 0);
    free(out);
    return NULL;
}

static void destroy_during_call(void) {
    sayso_engine e = open_engine();
    pthread_t th;
    pthread_create(&th, NULL, slow_enhance, &e);
    struct timespec pause = {0, 200 * 1000000L};
    nanosleep(&pause, NULL);
    long t0 = now_ms();
    sayso_engine_destroy(e);
    long took = now_ms() - t0;
    CHECK(took < 500, "destroy waited %ld ms for a cloud call", took);
    pthread_join(th, NULL);
}

int main(int argc, char **argv) {
    if (argc < 4) {
        fprintf(stderr, "usage: voice model.sslm journal-dir server-url\n");
        return 2;
    }
    model = argv[1];
    snprintf(journal, sizeof journal, "%s/typing.journal", argv[2]);
    snprintf(server, sizeof server, "%s", argv[3]);

    sayso_engine e = open_engine();
    post_process(e);
    styles(e);
    history(e);
    sayso_engine_destroy(e);

    e = open_engine();
    history_after_reopen(e);
    cloud_calls(e);
    sayso_engine_destroy(e);

    destroy_during_call();
    if (failures) {
        fprintf(stderr, "%d failures\n", failures);
        return 1;
    }
    printf("voice: ok\n");
    return 0;
}
