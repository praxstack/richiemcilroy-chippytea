// Exercise the real FFI and SQLite journal using marked disposable fixtures.
// This driver never removes files and refuses existing state databases.
#include "chippytea.h"
#include <ctype.h>
#include <errno.h>
#include <fcntl.h>
#include <stddef.h>
#include <stdbool.h>
#include <stdint.h>
#include <sqlite3.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/file.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

static double monotonic(void) {
    struct timespec now;
    if (clock_gettime(CLOCK_MONOTONIC_RAW, &now)) exit(5);
    return (double)now.tv_sec + (double)now.tv_nsec / 1e9;
}

static double cpu(const struct rusage *usage) {
    return (double)usage->ru_utime.tv_sec + (double)usage->ru_utime.tv_usec / 1e6
        + (double)usage->ru_stime.tv_sec + (double)usage->ru_stime.tv_usec / 1e6;
}

static void sleep_until(double deadline) {
    for (;;) {
        double remaining = deadline - monotonic();
        if (remaining <= 0) return;
        struct timespec interval = {.tv_sec = (time_t)remaining,
            .tv_nsec = (long)((remaining - (double)(time_t)remaining) * 1e9)};
        if (nanosleep(&interval, NULL) && errno != EINTR) exit(5);
    }
}

static bool valid_fixture(const char *path, unsigned run) {
    const char *prefix = "/private/tmp/chippytea-event-benchmark-";
    size_t length = strlen(prefix);
    if (strncmp(path, prefix, length) || strlen(path + length) != 32) return false;
    for (const char *p = path + length; *p; ++p) if (!isxdigit((unsigned char)*p)) return false;
    int fd = open(path, O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW_ANY);
    if (fd < 0) return false;
    struct stat info;
    bool valid = fstat(fd, &info) == 0 && info.st_uid == geteuid() && (info.st_mode & 0777) == 0700;
    int marker = openat(fd, ".chippytea-event-fixture", O_RDONLY | O_CLOEXEC | O_NOFOLLOW);
    const char magic[] = "chippytea-event-benchmark-v1\n";
    char bytes[sizeof(magic)];
    valid = valid && marker >= 0 && fstat(marker, &info) == 0 && S_ISREG(info.st_mode)
        && info.st_uid == geteuid() && info.st_nlink == 1 && info.st_size == (off_t)sizeof(magic) - 1
        && read(marker, bytes, sizeof(bytes)) == (ssize_t)sizeof(magic) - 1
        && memcmp(bytes, magic, sizeof(magic) - 1) == 0;
    if (marker >= 0) close(marker);
    valid = valid && fstatat(fd, "baseline", &info, AT_SYMLINK_NOFOLLOW) == 0
        && S_ISDIR(info.st_mode) && info.st_uid == geteuid() && (info.st_mode & 0777) == 0700;
    const char *suffixes[] = {"sqlite", "sqlite-wal", "sqlite-shm", "sqlite-journal", "lock"};
    for (size_t i = 0; valid && i < sizeof(suffixes) / sizeof(suffixes[0]); ++i) {
        char name[64];
        snprintf(name, sizeof(name), "state-%u.%s", run, suffixes[i]);
        valid = fstatat(fd, name, &info, AT_SYMLINK_NOFOLLOW) != 0 && errno == ENOENT;
    }
    close(fd);
    return valid;
}

static char *request(void *engine, const char *input) {
    char *response = ct_request(engine, input);
    if (!response || !strstr(response, "\"ok\":true")) {
        fputs(response ? response : "Missing FFI response", stderr);
        fputc('\n', stderr);
        ct_free_string(response);
        ct_close(engine);
        exit(3);
    }
    return response;
}

static char *settle(void *engine) {
    double deadline = monotonic() + 30;
    for (;;) {
        char *snapshot = request(engine, "{\"action\":\"snapshot\"}");
        // Snapshot is canonical engine JSON from this isolated synthetic root.
        if (strstr(snapshot, "\"scanning\":false")) return snapshot;
        ct_free_string(snapshot);
        if (monotonic() > deadline) { ct_cancel(engine); ct_close(engine); exit(4); }
        const struct timespec interval = {.tv_nsec = 5 * 1000 * 1000};
        nanosleep(&interval, NULL);
    }
}

static void replay_path(char *output, size_t capacity, const char *fixture, unsigned long index) {
    int length = snprintf(output, capacity,
        "%s/baseline/preserved-project/node_modules/group-0000/%s-%04lu.txt",
        fixture, index % 2 ? "missing" : "file", index / 2);
    if (length < 0 || (size_t)length >= capacity) exit(2);
}

static void close_and_wait(void *engine, const char *fixture, unsigned long run) {
    ct_close(engine);
    char path[256];
    snprintf(path, sizeof(path), "%s/state-%lu.lock", fixture, run);
    int fd = open(path, O_RDWR | O_CLOEXEC | O_NOFOLLOW_ANY);
    struct stat info;
    if (fd < 0 || fstat(fd, &info) || !S_ISREG(info.st_mode)
        || info.st_uid != geteuid() || info.st_nlink != 1) exit(3);
    double deadline = monotonic() + 5;
    while (flock(fd, LOCK_EX | LOCK_NB)) {
        if ((errno != EWOULDBLOCK && errno != EAGAIN) || monotonic() > deadline) exit(4);
        const struct timespec interval = {.tv_nsec = 1000 * 1000};
        nanosleep(&interval, NULL);
    }
    if (flock(fd, LOCK_UN)) exit(3);
    close(fd);
}

static sqlite3_int64 integer(sqlite3 *db, const char *query) {
    sqlite3_stmt *statement = NULL;
    if (sqlite3_prepare_v2(db, query, -1, &statement, NULL) != SQLITE_OK
        || sqlite3_step(statement) != SQLITE_ROW
        || sqlite3_column_type(statement, 0) != SQLITE_INTEGER) exit(3);
    sqlite3_int64 result = sqlite3_column_int64(statement, 0);
    if (sqlite3_step(statement) != SQLITE_DONE || sqlite3_finalize(statement) != SQLITE_OK) exit(3);
    return result;
}

// This is a read-only proof of state produced through public commands. If the
// debounce expired during staging, reject the run instead of timing a smaller
// or already-indexed queue. No SQL writes, worker hooks or timing retries exist.
static char *verify_staged_queue(const char *database, const char *fixture,
                                const char *root_id, unsigned long count) {
    sqlite3 *db = NULL;
    if (sqlite3_open_v2(database, &db, SQLITE_OPEN_READONLY | SQLITE_OPEN_NOFOLLOW, NULL) != SQLITE_OK
        || sqlite3_db_readonly(db, "main") != 1) exit(3);
    const char *empty = "SELECT (SELECT count(*) FROM candidates) + (SELECT count(*) FROM scans)"
        " + (SELECT count(*) FROM active_scopes) + (SELECT count(*) FROM refreshes)"
        " + (SELECT count(*) FROM refresh_seen) + (SELECT count(*) FROM kept)"
        " + (SELECT count(*) FROM operations) + (SELECT count(*) FROM earnings)"
        " + (SELECT count(*) FROM windows) + (SELECT count(*) FROM allocations)"
        " + (SELECT count(*) FROM candidate_tombstones)";
    if (integer(db, empty) != 0 || integer(db, "SELECT count(*) FROM roots") != 1
        || integer(db, "SELECT count(*) FROM incomplete_roots") != 1
        || integer(db, "SELECT count(*) FROM wallet WHERE id=1 AND collected=0 AND remainder=0 AND credited=0") != 1
        || integer(db, "SELECT count(*) FROM foreground_state WHERE id=1 AND summary_json IS NULL") != 1
        || integer(db, "SELECT cursor FROM event_cursor WHERE id=1") != 1000) {
        fputs("Raw replay staging changed the index or started a scan; no timing accepted.\n", stderr);
        exit(6);
    }
    size_t capacity = 128 + (count + 1) * (strlen(fixture) + 120);
    char *proof = calloc(capacity, 1);
    if (!proof) exit(3);
    size_t used = (size_t)snprintf(proof, capacity, "{\"unindexed\":true,\"cursor\":1000,\"pending_paths\":[");
    sqlite3_stmt *statement = NULL;
    if (sqlite3_prepare_v2(db, "SELECT root_id,path FROM pending_scopes ORDER BY rowid", -1, &statement, NULL) != SQLITE_OK) exit(3);
    for (unsigned long i = 0; i <= count; ++i) {
        char expected[256];
        if (i == 0) snprintf(expected, sizeof(expected), "%s/baseline/replay-sentinel", fixture);
        else replay_path(expected, sizeof(expected), fixture, i - 1);
        if (sqlite3_step(statement) != SQLITE_ROW
            || sqlite3_column_type(statement, 0) != SQLITE_TEXT
            || sqlite3_column_type(statement, 1) != SQLITE_TEXT
            || strcmp((const char *)sqlite3_column_text(statement, 0), root_id)
            || strcmp((const char *)sqlite3_column_text(statement, 1), expected)) {
            fputs("Raw replay staging did not retain the exact unnormalized queue; no timing accepted.\n", stderr);
            exit(6);
        }
        // The validated fixture prefix and generated suffix contain no JSON escapes.
        int written = snprintf(proof + used, capacity - used, "%s\"%s\"", i ? "," : "", expected);
        if (written < 0 || (size_t)written >= capacity - used) exit(3);
        used += (size_t)written;
    }
    if (sqlite3_step(statement) != SQLITE_DONE || sqlite3_finalize(statement) != SQLITE_OK) exit(6);
    if (capacity - used < 3 || snprintf(proof + used, capacity - used, "]}") != 2) exit(3);
    if (sqlite3_close(db) != SQLITE_OK) exit(3);
    return proof;
}

static char *stage_replay(void **engine, const char *database, const char *fixture,
                          const char *root_id, unsigned long count, unsigned long run) {
    char input[768], path[256];
    snprintf(input, sizeof(input),
        "{\"action\":\"dirty\",\"root_id\":\"%s\",\"events\":[{\"path\":\"%s/baseline/replay-sentinel\",\"kind\":\"directory\",\"recursive\":true}]}",
        root_id, fixture);
    ct_free_string(request(*engine, input));
    for (unsigned long i = 0; i < count; ++i) {
        replay_path(path, sizeof(path), fixture, i);
        snprintf(input, sizeof(input), "{\"action\":\"unkeep\",\"path\":\"%s\"}", path);
        ct_free_string(request(*engine, input));
    }
    ct_free_string(request(*engine, "{\"action\":\"cursor\",\"value\":1000}"));
    ct_cancel(*engine);
    ct_free_string(settle(*engine));
    close_and_wait(*engine, fixture, run);
    *engine = NULL;
    char *proof = verify_staged_queue(database, fixture, root_id, count);
    *engine = ct_open(database, NULL);
    if (!*engine) exit(3);
    return proof;
}

// Verify the measured phase before a later full Scan could repair unfinished
// work. The authorized root remains incomplete until that separate full pass.
static char *verify_replayed_queue(const char *database) {
    sqlite3 *db = NULL;
    if (sqlite3_open_v2(database, &db, SQLITE_OPEN_READONLY | SQLITE_OPEN_NOFOLLOW, NULL) != SQLITE_OK
        || sqlite3_db_readonly(db, "main") != 1) exit(3);
    if (integer(db, "SELECT (SELECT count(*) FROM pending_scopes) + (SELECT count(*) FROM active_scopes)"
            " + (SELECT count(*) FROM refreshes) + (SELECT count(*) FROM refresh_seen)") != 0
        || integer(db, "SELECT count(*) FROM incomplete_roots") != 1
        || integer(db, "SELECT count(*) FROM candidates") != 1
        || integer(db, "SELECT count(*) FROM scans") != 1) {
        fputs("Replay did not settle its exact durable queue before full-scan validation.\n", stderr);
        exit(6);
    }
    sqlite3_stmt *statement = NULL;
    if (sqlite3_prepare_v2(db, "SELECT json FROM scans", -1, &statement, NULL) != SQLITE_OK
        || sqlite3_step(statement) != SQLITE_ROW || sqlite3_column_type(statement, 0) != SQLITE_TEXT) exit(3);
    const unsigned char *json = sqlite3_column_text(statement, 0);
    char *stats = json ? strdup((const char *)json) : NULL;
    if (!stats || sqlite3_step(statement) != SQLITE_DONE || sqlite3_finalize(statement) != SQLITE_OK
        || sqlite3_close(db) != SQLITE_OK) exit(3);
    return stats;
}

// Capture the whole index, last scope and saved foreground before an untimed
// full Scan can repair anything. These reads are outside the measured interval.
static char *cargo_lock_state(const char *database, sqlite3_int64 cursor) {
    sqlite3 *db = NULL;
    if (sqlite3_open_v2(database, &db, SQLITE_OPEN_READONLY | SQLITE_OPEN_NOFOLLOW, NULL) != SQLITE_OK
        || sqlite3_db_readonly(db, "main") != 1
        || sqlite3_exec(db, "BEGIN", NULL, NULL, NULL) != SQLITE_OK) exit(3);
    const char *empty = "SELECT (SELECT count(*) FROM pending_scopes) + (SELECT count(*) FROM active_scopes)"
        " + (SELECT count(*) FROM refreshes) + (SELECT count(*) FROM refresh_seen)"
        " + (SELECT count(*) FROM incomplete_roots) + (SELECT count(*) FROM kept)"
        " + (SELECT count(*) FROM operations) + (SELECT count(*) FROM earnings)"
        " + (SELECT count(*) FROM windows) + (SELECT count(*) FROM allocations)"
        " + (SELECT count(*) FROM candidate_tombstones)";
    if (integer(db, empty) != 0 || integer(db, "SELECT count(*) FROM roots") != 1
        || integer(db, "SELECT count(*) FROM candidates") != 2
        || integer(db, "SELECT count(*) FROM scans") != 1
        || integer(db, "SELECT count(*) FROM wallet WHERE id=1 AND collected=0 AND remainder=0 AND credited=0") != 1
        || integer(db, "SELECT cursor FROM event_cursor WHERE id=1") != cursor) {
        fputs("Cargo.lock replay did not retain its complete index and empty queue; no timing accepted.\n", stderr);
        exit(6);
    }
    sqlite3_stmt *statement = NULL;
    const char *query = "SELECT json_object("
        "'candidates',json((SELECT json_group_array(json(json)) FROM (SELECT json FROM candidates ORDER BY path))),"
        "'scope_stats',json((SELECT json FROM scans)),"
        "'foreground_scan',json((SELECT summary_json FROM foreground_state WHERE id=1)))";
    if (sqlite3_prepare_v2(db, query, -1, &statement, NULL) != SQLITE_OK
        || sqlite3_step(statement) != SQLITE_ROW || sqlite3_column_type(statement, 0) != SQLITE_TEXT) exit(3);
    const unsigned char *json = sqlite3_column_text(statement, 0);
    char *state = json ? strdup((const char *)json) : NULL;
    if (!state || sqlite3_step(statement) != SQLITE_DONE || sqlite3_finalize(statement) != SQLITE_OK
        || sqlite3_exec(db, "COMMIT", NULL, NULL, NULL) != SQLITE_OK || sqlite3_close(db) != SQLITE_OK) exit(3);
    return state;
}

// A consistent read-only view proves that the latency probe starts behind a
// pending event, rather than an already-running traversal or unfinished work.
static void verify_periodic_queue(const char *database, const char *root_id,
                                  const char *pending, sqlite3_int64 cursor) {
    sqlite3 *db = NULL;
    if (sqlite3_open_v2(database, &db, SQLITE_OPEN_READONLY | SQLITE_OPEN_NOFOLLOW, NULL) != SQLITE_OK
        || sqlite3_db_readonly(db, "main") != 1
        || sqlite3_exec(db, "BEGIN", NULL, NULL, NULL) != SQLITE_OK) exit(3);
    const char *empty = "SELECT (SELECT count(*) FROM active_scopes) + (SELECT count(*) FROM refreshes)"
        " + (SELECT count(*) FROM refresh_seen) + (SELECT count(*) FROM incomplete_roots)"
        " + (SELECT count(*) FROM operations) + (SELECT count(*) FROM earnings)"
        " + (SELECT count(*) FROM windows) + (SELECT count(*) FROM allocations)"
        " + (SELECT count(*) FROM kept) + (SELECT count(*) FROM candidate_tombstones)";
    if (integer(db, empty) != 0 || integer(db, "SELECT count(*) FROM roots") != 1
        || integer(db, "SELECT count(*) FROM candidates") != 1
        || integer(db, "SELECT count(*) FROM scans") != 1
        || integer(db, "SELECT count(*) FROM wallet WHERE id=1 AND collected=0 AND remainder=0 AND credited=0") != 1
        || integer(db, "SELECT cursor FROM event_cursor WHERE id=1") != cursor) {
        fputs("Periodic workload did not retain its complete, idle library; no timing accepted.\n", stderr);
        exit(6);
    }
    sqlite3_stmt *statement = NULL;
    if (sqlite3_prepare_v2(db, "SELECT root_id,path FROM pending_scopes", -1, &statement, NULL) != SQLITE_OK) exit(3);
    if (pending && (sqlite3_step(statement) != SQLITE_ROW
        || sqlite3_column_type(statement, 0) != SQLITE_TEXT
        || sqlite3_column_type(statement, 1) != SQLITE_TEXT
        || strcmp((const char *)sqlite3_column_text(statement, 0), root_id)
        || strcmp((const char *)sqlite3_column_text(statement, 1), pending))) {
        fputs("Scan probe did not retain the exact pending scope; no timing accepted.\n", stderr);
        exit(6);
    }
    if (sqlite3_step(statement) != SQLITE_DONE) {
        fputs("Periodic workload retained unexpected queued work; no timing accepted.\n", stderr);
        exit(6);
    }
    if (sqlite3_finalize(statement) != SQLITE_OK || sqlite3_exec(db, "COMMIT", NULL, NULL, NULL) != SQLITE_OK
        || sqlite3_close(db) != SQLITE_OK) exit(3);
}

static void accepted_event(void *engine, const char *input) {
    char *response = request(engine, input);
    if (strstr(response, "\"ignored\":true")) {
        fputs("Periodic event was ignored; no timing accepted.\n", stderr);
        exit(6);
    }
    ct_free_string(response);
}

static void periodic_background(void *engine, const char *database, const char *fixture,
                                const char *root_id, const char *before) {
    enum { count = 16 };
    const double period = 0.4, window = 8.0, maximum_lateness = 0.05;
    double submitted[count], acknowledged[count], cursor_acknowledged[count];
    char path[256], input[768], cursor_request[80];
    snprintf(path, sizeof(path), "%s/baseline/periodic-source", fixture);
    snprintf(input, sizeof(input),
        "{\"action\":\"dirty\",\"root_id\":\"%s\",\"events\":[{\"path\":\"%s\",\"kind\":\"directory\",\"recursive\":true}]}",
        root_id, path);
    ct_free_string(request(engine, "{\"action\":\"cursor\",\"value\":1000}"));
    verify_periodic_queue(database, root_id, NULL, 1000);
    struct rusage initial, final;
    if (getrusage(RUSAGE_SELF, &initial)) exit(5);
    double started = monotonic();
    for (unsigned i = 0; i < count; ++i) {
        sleep_until(started + (double)i * period);
        submitted[i] = monotonic() - started;
        if (submitted[i] - (double)i * period > maximum_lateness) {
            fputs("Periodic input missed its delivery deadline; no timing accepted.\n", stderr);
            exit(6);
        }
        accepted_event(engine, input);
        acknowledged[i] = monotonic() - started;
        snprintf(cursor_request, sizeof(cursor_request), "{\"action\":\"cursor\",\"value\":%u}", 1001 + i);
        ct_free_string(request(engine, cursor_request));
        cursor_acknowledged[i] = monotonic() - started;
        if (cursor_acknowledged[i] - (double)i * period > maximum_lateness) {
            fputs("Periodic receipt exceeded its delivery budget; no timing accepted.\n", stderr);
            exit(6);
        }
    }
    // No polling during the observation window: every build pays for the same
    // sixteen requests/cursors and one endpoint snapshot, including its debounce.
    sleep_until(started + window);
    double endpoint_requested = monotonic() - started;
    char *after = request(engine, "{\"action\":\"snapshot\"}");
    double elapsed = monotonic() - started;
    if (getrusage(RUSAGE_SELF, &final)) exit(5);
    if (elapsed > window + maximum_lateness || !strstr(after, "\"scanning\":false")) {
        fputs("Periodic work did not settle inside the fixed window; no timing accepted.\n", stderr);
        exit(6);
    }
    verify_periodic_queue(database, root_id, NULL, 1016);

    // Probe separately so its polling and full traversal cannot inflate the
    // periodic CPU result. Acknowledgment alone is not evidence of worker wakeup.
    double probe_started = monotonic();
    accepted_event(engine, input);
    double event_ack = monotonic() - probe_started;
    ct_free_string(request(engine, "{\"action\":\"cursor\",\"value\":1017}"));
    double probe_cursor_ack = monotonic() - probe_started;
    sleep_until(probe_started + 0.05);
    double gate_started = monotonic() - probe_started;
    verify_periodic_queue(database, root_id, path, 1017);
    char *gate = request(engine, "{\"action\":\"snapshot\"}");
    double scan_started = monotonic();
    double scan_offset = scan_started - probe_started;
    if (event_ack > maximum_lateness || probe_cursor_ack > maximum_lateness
        || scan_offset > 0.10 || !strstr(gate, "\"scanning\":true")) {
        fputs("Scan probe missed the pending-event window; no timing accepted.\n", stderr);
        exit(6);
    }
    ct_free_string(request(engine, "{\"action\":\"scan\"}"));
    double scan_ack = monotonic() - scan_started;
    char *full = settle(engine);
    double scan_completed = monotonic() - scan_started;
    verify_periodic_queue(database, root_id, NULL, 1017);

    printf("{\"workload\":\"periodic-background\",\"scopes\":16,\"wall_seconds\":%.9f,\"cpu_seconds\":%.9f,\"lifetime_peak_rss_bytes\":%ld,\"before\":%s,\"after\":%s,\"full\":%s,",
        elapsed, cpu(&final) - cpu(&initial), final.ru_maxrss, before, after, full);
    printf("\"period_seconds\":0.4,\"window_seconds\":8,\"maximum_lateness_seconds\":0.05,\"endpoint_requested_seconds\":%.9f,\"event_path\":\"%s\",\"event_kind\":\"directory\",\"event_recursive\":true,\"pre_probe_queue_empty\":true,\"events\":[", endpoint_requested, path);
    for (unsigned i = 0; i < count; ++i) {
        printf("%s{\"scheduled_seconds\":%.9f,\"submitted_seconds\":%.9f,\"acknowledged_seconds\":%.9f,\"cursor_acknowledged_seconds\":%.9f,\"cursor\":%u}",
            i ? "," : "", (double)i * period, submitted[i], acknowledged[i], cursor_acknowledged[i], 1001 + i);
    }
    printf("],\"probe\":{\"event_acknowledged_seconds\":%.9f,\"cursor_acknowledged_seconds\":%.9f,\"gate_started_seconds\":%.9f,\"scan_submitted_seconds\":%.9f,\"scan_acknowledgment_seconds\":%.9f,\"scan_completion_seconds\":%.9f,\"pending_path\":\"%s\",\"cursor\":1017,\"exact_pending_scope\":true,\"final_queue_empty\":true,\"gate\":%s}}\n",
        event_ack, probe_cursor_ack, gate_started, scan_offset, scan_ack, scan_completed, path, gate);
    ct_free_string(after); ct_free_string(gate); ct_free_string(full);
}

int main(int argc, char **argv) {
    if (argc != 4 && argc != 5) return 2;
    const char *workload = argc == 5 ? argv[4] : "mixed";
    bool replay = strcmp(workload, "unindexed-artifact-replay") == 0;
    bool periodic = strcmp(workload, "periodic-background") == 0;
    bool artifact_batch = strcmp(workload, "artifact-event-batch") == 0;
    bool cargo_lock = strcmp(workload, "cargo-lock-refresh") == 0;
    if (!replay && !periodic && !artifact_batch && !cargo_lock && strcmp(workload, "mixed")) return 2;
    char *end;
    errno = 0;
    unsigned long count = strtoul(argv[2], &end, 10);
    if (errno || end == argv[2] || *end || count < (artifact_batch || cargo_lock ? 1UL : 2UL)
        || count > (replay ? 64UL : 512UL) || (!artifact_batch && !cargo_lock && count % 2)) return 2;
    if (periodic && count != 16) return 2;
    errno = 0;
    unsigned long run = strtoul(argv[3], &end, 10);
    if (errno || end == argv[3] || *end || run > 1000 || !valid_fixture(argv[1], (unsigned)run)) return 2;
    char database[256], input[512];
    snprintf(database, sizeof(database), "%s/state-%lu.sqlite", argv[1], run);
    void *engine = ct_open(database, NULL);
    if (!engine) return 3;
    snprintf(input, sizeof(input), "{\"action\":\"authorize\",\"path\":\"%s/baseline\",\"kind\":\"projects\"}", argv[1]);
    char *authorized = request(engine, input);
    char id[128];
    const char *start = strstr(authorized, "\"id\":\"");
    const char *finish = start ? strchr(start + 6, '"') : NULL;
    if (!finish || finish - (start + 6) <= 0 || finish - (start + 6) >= (ptrdiff_t)sizeof(id)) return 3;
    size_t id_length = (size_t)(finish - (start + 6));
    for (size_t i = 0; i < id_length; ++i) if (!isxdigit((unsigned char)start[6 + i]) && start[6 + i] != '-') return 3;
    memcpy(id, start + 6, id_length); id[id_length] = 0;
    ct_free_string(authorized);
    char *staged = NULL;
    if (replay) staged = stage_replay(&engine, database, argv[1], id, count, run);
    else ct_free_string(request(engine, "{\"action\":\"scan\"}"));
    char *before = settle(engine);
    if (periodic) {
        periodic_background(engine, database, argv[1], id, before);
        ct_free_string(before);
        ct_close(engine);
        return 0;
    }

    char *cargo_before = cargo_lock ? cargo_lock_state(database, 0) : NULL;
    char *events = NULL;
    if (!replay) {
        unsigned long event_count = cargo_lock ? 1 : count;
        size_t capacity = 256 + event_count * (strlen(argv[1]) + (artifact_batch || cargo_lock ? 160 : 100));
        events = calloc(capacity, 1);
        if (!events) return 3;
        int event_directory_fd = -1;
        if (artifact_batch || cargo_lock) {
            char path[256];
            int length = snprintf(path, sizeof(path), "%s/baseline/%s", argv[1],
                cargo_lock ? "cargo-project" : "preserved-project/node_modules");
            if (length < 0 || (size_t)length >= sizeof(path)) return 3;
            event_directory_fd = open(path, O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW_ANY);
            struct stat info;
            if (event_directory_fd < 0 || fstat(event_directory_fd, &info) || !S_ISDIR(info.st_mode)
                || info.st_uid != geteuid()) return 3;
        }
        size_t used = (size_t)snprintf(events, capacity, "{\"action\":\"dirty\",\"root_id\":\"%s\",\"events\":[", id);
        for (unsigned long i = 0; i < event_count; ++i) {
            int written;
            if (artifact_batch || cargo_lock) {
                char name[32];
                int length = cargo_lock ? snprintf(name, sizeof(name), "Cargo.lock")
                    : snprintf(name, sizeof(name), "event-%04lu.txt", i);
                struct stat info;
                if (length < 0 || (size_t)length >= sizeof(name)
                    || fstatat(event_directory_fd, name, &info, AT_SYMLINK_NOFOLLOW)
                    || !S_ISREG(info.st_mode) || info.st_uid != geteuid() || info.st_nlink != 1) {
                    fputs("Typed file replay requires existing owned regular files.\n", stderr);
                    return 6;
                }
                written = snprintf(events + used, capacity - used,
                    "%s{\"path\":\"%s/baseline/%s/%s\",\"kind\":\"file\",\"recursive\":false}",
                    i ? "," : "", argv[1], cargo_lock ? "cargo-project" : "preserved-project/node_modules", name);
            } else {
                written = snprintf(events + used, capacity - used,
                    "%s{\"path\":\"%s/baseline/%s-%04lu\",\"kind\":\"directory\",\"recursive\":true}",
                    i ? "," : "", argv[1], i % 2 ? "missing" : "existing", i / 2);
            }
            if (written < 0 || (size_t)written >= capacity - used) return 3;
            used += (size_t)written;
        }
        if (event_directory_fd >= 0 && close(event_directory_fd)) return 3;
        if (capacity - used < 3 || snprintf(events + used, capacity - used, "]}") != 2) return 3;
    }
    struct rusage initial, final;
    double dirty_acknowledgment = 0, dirty_acknowledgment_cpu = 0;
    if (getrusage(RUSAGE_SELF, &initial)) return 5;
    double started = monotonic();
    if (artifact_batch || cargo_lock) {
        char *acknowledgment = request(engine, events);
        if (strstr(acknowledgment, "\"ignored\":true")) {
            fputs("Typed file-event replay was ignored; no timing accepted.\n", stderr);
            return 6;
        }
        ct_free_string(acknowledgment);
        dirty_acknowledgment = monotonic() - started;
        struct rusage acknowledged;
        if (getrusage(RUSAGE_SELF, &acknowledged)) return 5;
        dirty_acknowledgment_cpu = cpu(&acknowledged) - cpu(&initial);
    } else {
        ct_free_string(request(engine, replay ? "{\"action\":\"resume\"}" : events));
    }
    if (!replay) ct_free_string(request(engine, "{\"action\":\"cursor\",\"value\":1000}"));
    char *after = settle(engine);
    double elapsed = monotonic() - started;
    if (getrusage(RUSAGE_SELF, &final)) return 5;
    char *full = NULL, *scope_stats = NULL;
    char *cargo_after = NULL, *cargo_full = NULL;
    if (replay) {
        scope_stats = verify_replayed_queue(database);
        ct_free_string(request(engine, "{\"action\":\"scan\"}"));
        full = settle(engine);
    }
    if (cargo_lock) {
        cargo_after = cargo_lock_state(database, 1000);
        ct_free_string(request(engine, "{\"action\":\"scan\"}"));
        full = settle(engine);
        cargo_full = cargo_lock_state(database, 1000);
    }
    printf("{\"workload\":\"%s\",\"scopes\":%lu,\"wall_seconds\":%.9f,\"cpu_seconds\":%.9f,\"lifetime_peak_rss_bytes\":%ld,\"before\":%s,\"after\":%s",
        workload, count, elapsed, cpu(&final) - cpu(&initial), final.ru_maxrss, before, after);
    if (replay) printf(",\"staged\":%s,\"replay_queue_empty\":true,\"replay_scope_stats\":%s,\"full\":%s", staged, scope_stats, full);
    if (artifact_batch || cargo_lock) printf(",\"event_paths_verified\":true,\"dirty_request\":%s,\"dirty_acknowledgment_seconds\":%.9f,\"dirty_acknowledgment_cpu_seconds\":%.9f",
        events, dirty_acknowledgment, dirty_acknowledgment_cpu);
    if (cargo_lock) printf(",\"full\":%s,\"library_states\":{\"before\":%s,\"after\":%s,\"full\":%s},\"measured_queue_empty\":true",
        full, cargo_before, cargo_after, cargo_full);
    puts("}");
    free(events); free(staged); free(scope_stats); ct_free_string(full);
    free(cargo_before); free(cargo_after); free(cargo_full);
    ct_free_string(before); ct_free_string(after); ct_close(engine);
    return 0;
}
