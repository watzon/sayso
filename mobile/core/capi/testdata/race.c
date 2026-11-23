/* race.c: four threads call sayso_on_touch on whatever engine is current
 * while the main thread creates and destroys engines. Destroy unmaps the
 * model, so a call that is still running when it does must finish first;
 * before that was so, this crashed in the decoder within a few hundred
 * iterations. Built and run by mobile/scripts/test-capi.sh. */
#include <pthread.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "sayso_core.h"

static const float KW = 36, KH = 54;
static sayso_key keys[40];
static int nkeys = 0;

static void add_row(const char *letters, float x0, float y) {
    for (int i = 0; letters[i]; i++) {
        keys[nkeys++] = (sayso_key){letters[i], x0 + i * KW, y, KW, KH};
    }
}

static _Atomic sayso_engine current;
static atomic_int stop;
static atomic_long calls, handle_errs, other_errs;

static void *toucher(void *arg) {
    (void)arg;
    sayso_touch_result r;
    for (int j = 0; !atomic_load(&stop); j++) {
        sayso_engine e = atomic_load(&current);
        if (!e) {
            continue;
        }
        int32_t rc = sayso_on_touch(e, (j % 10) * KW + 18, 27, 0, 0, &r);
        atomic_fetch_add(&calls, 1);
        if (rc == SAYSO_ERR_HANDLE) {
            atomic_fetch_add(&handle_errs, 1);
        } else if (rc != SAYSO_OK) {
            atomic_fetch_add(&other_errs, 1);
        }
    }
    return NULL;
}

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: race model.sslm [iterations]\n");
        return 2;
    }
    int iterations = argc > 2 ? atoi(argv[2]) : 3000;
    add_row("qwertyuiop", 0, 0);
    add_row("asdfghjkl", KW / 2, KH);
    add_row("zxcvbnm", 1.5f * KW, 2 * KH);
    keys[nkeys++] = (sayso_key){' ', 2 * KW, 3 * KH, 6 * KW, KH};

    pthread_t threads[4];
    for (int i = 0; i < 4; i++) {
        pthread_create(&threads[i], NULL, toucher, NULL);
    }
    for (int it = 0; it < iterations; it++) {
        sayso_engine e = 0;
        if (sayso_engine_create(argv[1], (int32_t)strlen(argv[1]), "", 0, &e) != SAYSO_OK) {
            fprintf(stderr, "FAIL: create\n");
            return 1;
        }
        sayso_set_layout(e, keys, nkeys);
        atomic_store(&current, e);
        usleep(200);
        atomic_store(&current, 0);
        sayso_engine_destroy(e);
    }
    atomic_store(&stop, 1);
    for (int i = 0; i < 4; i++) {
        pthread_join(threads[i], NULL);
    }
    printf("race: %d engines, %ld touches, %ld after destroy (ERR_HANDLE), %ld other errors\n",
           iterations, atomic_load(&calls), atomic_load(&handle_errs), atomic_load(&other_errs));
    if (atomic_load(&other_errs) != 0) {
        fprintf(stderr, "FAIL: unexpected error codes\n");
        return 1;
    }
    return 0;
}
