#ifndef CK_METAL_TEST_BINDLESS_PROBE_H
#define CK_METAL_TEST_BINDLESS_PROBE_H

#include <stddef.h>
#include <stdint.h>

typedef struct CKTestBindlessProbe CKTestBindlessProbe;

CKTestBindlessProbe *ck_test_bindless_probe_new(void *context, void *table,
                                                const uint8_t *library, size_t library_length,
                                                uint32_t slot, int32_t hold_completion,
                                                char *error, size_t capacity);
int ck_test_bindless_probe_prepare(CKTestBindlessProbe *probe,
                                   char *error, size_t capacity);
int ck_test_bindless_probe_commit(CKTestBindlessProbe *probe,
                                  char *error, size_t capacity);
int ck_test_bindless_probe_gpu_done(CKTestBindlessProbe *probe);
int ck_test_bindless_probe_read(CKTestBindlessProbe *probe, uint8_t *output,
                                size_t length, char *error, size_t capacity);
void ck_test_bindless_probe_open_gate(CKTestBindlessProbe *probe);
int ck_test_bindless_table_entry(void *table, uint32_t slots, uint32_t slot,
                                 uint64_t *resource_id);
/* The native length, in bytes, of a table buffer (an id<MTLBuffer>): what the allocation asked for, not its page-rounded
 * allocatedSize. 0 for NULL. */
uint64_t ck_test_bindless_table_length(void *table);
void ck_test_bindless_probe_free(CKTestBindlessProbe *probe);

#endif
