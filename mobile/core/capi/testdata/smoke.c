/* smoke.c: drives the C ABI end to end and times sayso_on_touch, including
 * the cgo crossing. Built and run by mobile/scripts/test-capi.sh. */
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#include "sayso_core.h"

static int failures = 0;
#define CHECK(cond, ...)                                  \
    do {                                                  \
        if (!(cond)) {                                    \
            fprintf(stderr, "FAIL %s:%d: ", __FILE__, __LINE__); \
            fprintf(stderr, __VA_ARGS__);                 \
            fprintf(stderr, "\n");                        \
            failures++;                                   \
        }                                                 \
    } while (0)

static const float KW = 36, KH = 54;
static sayso_key keys[40];
static int nkeys = 0;

static void add_row(const char *letters, float x0, float y) {
    for (int i = 0; letters[i]; i++) {
        keys[nkeys++] = (sayso_key){letters[i], x0 + i * KW, y, KW, KH};
    }
}

static void build_layout(void) {
    add_row("qwertyuiop", 0, 0);
    add_row("asdfghjkl", KW / 2, KH);
    add_row("zxcvbnm", 1.5f * KW, 2 * KH);
    keys[nkeys++] = (sayso_key){SAYSO_KEY_SHIFT, 0, 2 * KH, 1.5f * KW, KH};
    keys[nkeys++] = (sayso_key){SAYSO_KEY_BACKSPACE, 8.5f * KW, 2 * KH, 1.5f * KW, KH};
    keys[nkeys++] = (sayso_key){SAYSO_KEY_SYMBOLS, 0, 3 * KH, 2.5f * KW, KH};
    keys[nkeys++] = (sayso_key){' ', 2.5f * KW, 3 * KH, 5 * KW, KH};
    keys[nkeys++] = (sayso_key){'.', 7.5f * KW, 3 * KH, KW, KH};
    keys[nkeys++] = (sayso_key){SAYSO_KEY_ENTER, 8.5f * KW, 3 * KH, 1.5f * KW, KH};
}

static void center(int code, float *x, float *y) {
    for (int i = 0; i < nkeys; i++) {
        if (keys[i].code == code) {
            *x = keys[i].x + keys[i].w / 2;
            *y = keys[i].y + keys[i].h / 2;
            return;
        }
    }
    *x = *y = -1;
}

/* A text field that applies actions. */
static char field[4096];

static void apply(const sayso_action *a, const char *text, const char *fallback) {
    if (a->kind == SAYSO_ACTION_NONE) {
        strcat(field, fallback);
        return;
    }
    /* delete_chars code points; the test text is ASCII */
    size_t n = strlen(field);
    field[n - (size_t)a->delete_chars] = 0;
    strncat(field, text, (size_t)a->text_len);
}

static void type_text(sayso_engine e, const char *s) {
    char buf[SAYSO_TEXT_CAP];
    for (; *s; s++) {
        sayso_action a;
        if (*s >= 'a' && *s <= 'z') {
            float x, y;
            center(*s, &x, &y);
            sayso_touch_result r;
            CHECK(sayso_on_touch(e, x, y, 0, 0, &r) == SAYSO_OK, "on_touch");
            CHECK(r.flags & SAYSO_TOUCH_LETTER, "letter flag for %c", *s);
            char c[2] = {(char)r.code, 0};
            strcat(field, c);
        } else {
            CHECK(sayso_type_char(e, *s, &a, buf, sizeof buf) == SAYSO_OK, "type_char");
            char c[2] = {*s, 0};
            apply(&a, buf, c);
        }
    }
}

static int cmp_long(const void *a, const void *b) {
    long x = *(const long *)a, y = *(const long *)b;
    return (x > y) - (x < y);
}

static long now_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec * 1000000000L + ts.tv_nsec;
}

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: smoke model.sslm [journal]\n");
        return 2;
    }
    const char *model = argv[1];
    const char *journal = argc > 2 ? argv[2] : "";
    printf("%s\n", sayso_version());

    sayso_engine e = 0;
    CHECK(sayso_engine_create("/nonexistent.sslm", 17, "", 0, &e) == SAYSO_ERR_IO, "missing model");
    char msg[256];
    CHECK(sayso_last_error(0, msg, sizeof msg) > 0, "create error message");
    CHECK(sayso_engine_create(model, (int32_t)strlen(model), journal, (int32_t)strlen(journal), &e) == SAYSO_OK, "create");
    CHECK(e != 0, "handle");
    CHECK(sayso_set_context(12345, "x", 1) == SAYSO_ERR_HANDLE, "dead handle");

    /* The fd path, as for an APK asset. */
    int fd = open(model, O_RDONLY);
    off_t len = lseek(fd, 0, SEEK_END);
    sayso_engine e2 = 0;
    CHECK(sayso_engine_create_fd(fd, 0, len, "", 0, &e2) == SAYSO_OK, "create_fd");
    close(fd);
    sayso_engine_destroy(e2);
    CHECK(sayso_should_capitalize(e2) == SAYSO_ERR_HANDLE, "destroyed handle");

    build_layout();
    CHECK(sayso_set_layout(e, keys, nkeys) == SAYSO_OK, "layout");
    CHECK(sayso_set_layout(e, keys, 0) == SAYSO_ERR_ARG, "empty layout");
    CHECK(sayso_set_editor_flags(e, SAYSO_EDITOR_AUTOCAP) == SAYSO_OK, "flags");
    CHECK(sayso_should_capitalize(e) == 1, "capitalize at start");

    /* Autocorrect, then undo it with backspace. */
    type_text(e, "i dont ");
    CHECK(strcmp(field, "I don't ") == 0, "autocorrect: %s", field);
    char buf[SAYSO_TEXT_CAP];
    sayso_action a;
    CHECK(sayso_backspace(e, &a, buf, sizeof buf) == SAYSO_OK, "backspace");
    CHECK(a.kind == SAYSO_ACTION_REPLACE, "backspace reverts");
    apply(&a, buf, "");
    CHECK(strcmp(field, "I dont") == 0, "revert: %s", field);
    CHECK(sayso_backspace(e, &a, buf, 10) == SAYSO_ERR_ARG, "small action buffer");

    /* Suggestions and a pick. */
    type_text(e, " want to beau");
    sayso_suggestion sugs[5];
    char text[512];
    int32_t n = sayso_get_suggestions(e, sugs, 5, text, sizeof text);
    CHECK(n > 0, "suggestions");
    for (int i = 0; i < n; i++) {
        printf("  suggestion %d: %.*s (%.2f, flags %u)\n", i, sugs[i].text_len, text + sugs[i].text_offset, sugs[i].score, sugs[i].flags);
    }
    CHECK(sayso_pick_suggestion(e, n - 1, &a, buf, sizeof buf) == SAYSO_OK, "pick");
    apply(&a, buf, "");
    printf("  field: %s\n", field);

    /* Personal words and replacements. */
    CHECK(sayso_add_word(e, "Sayso", 5) == SAYSO_OK, "add word");
    CHECK(sayso_add_replacement(e, "omw", 3, "on my way", 9, 0) == SAYSO_OK, "add replacement");
    static char long_text[600];
    memset(long_text, 'x', sizeof long_text);
    CHECK(sayso_add_replacement(e, "sig", 3, long_text, 513, 0) == SAYSO_ERR_ARG, "a replacement over 512 bytes is refused");
    CHECK(sayso_last_error(e, NULL, 0) > 0, "and says why");
    sayso_set_context(e, "", 0);
    field[0] = 0;
    type_text(e, "sayso omw ");
    CHECK(strcmp(field, "Sayso on my way ") == 0, "personal: %s", field);

    /* Listing them, for the companion app's dictionary screen. */
    char list[256];
    int32_t need = sayso_list_words(e, SAYSO_WORDS_ADDED | SAYSO_WORDS_LEARNED, NULL, 0);
    CHECK(need == 11, "list words size %d", need); /* "dont" was kept twice above: undone, then kept */
    CHECK(sayso_list_words(e, SAYSO_WORDS_ADDED | SAYSO_WORDS_LEARNED, list, 10) == 11, "list words too small");
    CHECK(sayso_list_words(e, SAYSO_WORDS_ADDED, list, sizeof list) == 6 && memcmp(list, "Sayso\0", 6) == 0, "added words");
    CHECK(sayso_list_words(e, SAYSO_WORDS_LEARNED, list, sizeof list) == 5 && memcmp(list, "dont\0", 5) == 0, "learned words");
    need = sayso_list_replacements(e, list, 4);
    CHECK(need == 16, "list replacements size %d", need);
    CHECK(sayso_list_replacements(e, list, sizeof list) == 16 && memcmp(list, "omw\0on my way\0" "0", 15) == 0 && list[15] == 0,
          "list replacements");

    /* Preferences: autocorrect off keeps the typo; predictions off empties the strip. */
    CHECK(sayso_set_preferences(e, 3, 1) == SAYSO_ERR_ARG, "bad autocorrect level");
    CHECK(sayso_set_preferences(e, SAYSO_AUTOCORRECT_OFF, 0) == SAYSO_OK, "preferences");
    type_text(e, "teh");
    char cur[16];
    CHECK(sayso_current_word(e, cur, sizeof cur) == 3 && strcmp(cur, "teh") == 0, "current word");
    type_text(e, " ");
    CHECK(strcmp(field, "Sayso on my way teh ") == 0, "autocorrect off: %s", field);
    CHECK(sayso_get_suggestions(e, sugs, 5, text, sizeof text) == 0, "predictions off");
    CHECK(sayso_set_preferences(e, SAYSO_AUTOCORRECT_NORMAL, 1) == SAYSO_OK, "preferences back");

    /* Diagnostics. */
    type_text(e, "th");
    sayso_diag d;
    sayso_key_diag kd[64];
    sayso_cand_diag cd[8];
    char dtext[512];
    CHECK(sayso_get_diagnostics(e, &d, kd, 64, cd, 8, dtext, sizeof dtext) == SAYSO_OK, "diagnostics");
    CHECK(d.key_count == nkeys && d.cand_count > 0, "diag counts %d %d", d.key_count, d.cand_count);
    printf("  diag: margin %.2f, top candidate %.*s\n", d.margin, cd[0].text_len, dtext + cd[0].text_offset);
    CHECK(sayso_reset_learned(e) == SAYSO_OK, "reset");

    /* Timing: taps through the C ABI, a word of 8 letters then a space. */
    const char *word = "understa";
    enum { N = 200000 };
    long *ns = malloc(sizeof(long) * N);
    float xs[8], ys[8];
    for (int i = 0; i < 8; i++) {
        center(word[i], &xs[i], &ys[i]);
        xs[i] += (float)((i % 3) - 1) * 6;
    }
    sayso_touch_result r;
    for (int i = 0; i < N; i++) {
        int j = i % 8;
        long t0 = now_ns();
        sayso_on_touch(e, xs[j], ys[j], 0, 0, &r);
        ns[i] = now_ns() - t0;
        if (j == 7) {
            sayso_type_char(e, ' ', &a, buf, sizeof buf);
        }
    }
    qsort(ns, N, sizeof(long), cmp_long);
    long sum = 0;
    for (int i = 0; i < N; i++) sum += ns[i];
    printf("on_touch via C: mean %ld ns, p50 %ld ns, p99 %ld ns, p99.9 %ld ns, max %ld ns (%d taps)\n",
           sum / N, ns[N / 2], ns[N * 99 / 100], ns[N * 999 / 1000], ns[N - 1], N);
    CHECK(ns[N * 99 / 100] < 50000, "p99 %ld ns over 50 us", ns[N * 99 / 100]);
    free(ns);

    sayso_engine_destroy(e);
    if (failures) {
        fprintf(stderr, "%d failures\n", failures);
        return 1;
    }
    printf("ok\n");
    return 0;
}
