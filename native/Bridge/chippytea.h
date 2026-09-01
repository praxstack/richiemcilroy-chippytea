#ifndef CHIPPYTEA_H
#define CHIPPYTEA_H
#include <stddef.h>
#ifdef __cplusplus
extern "C" {
#endif
typedef int (*ct_trash_callback)(const char *path, char *result, size_t capacity);
void *ct_open(const char *database_path, ct_trash_callback trash);
char *ct_request(const void *engine, const char *json);
void ct_cancel(const void *engine);
void ct_free_string(char *value);
void ct_close(void *engine);
#ifdef __cplusplus
}
#endif
#endif
