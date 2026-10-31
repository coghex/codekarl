#ifndef CK_METAL_BRIDGE_H
#define CK_METAL_BRIDGE_H
#include <stddef.h>
#include <stdint.h>

typedef struct CKMetalHandle CKMetalHandle;

/* Returns one owned handle, or NULL and a NUL-terminated error message.
 * Input bytes are borrowed only for this call. All calls must be made on the
 * creating thread. Error buffers are caller-owned and may be NULL if capacity=0.
 */
CKMetalHandle *ck_metal_create(const uint8_t *library, size_t length,
                              char *error, size_t error_capacity);
/* Synchronous bootstrap probe: waits for GPU completion before returning.
 * Writes 64 uint32_t results on success. Returns 0 on success, 1 on failure.
 */
int ck_metal_probe(CKMetalHandle *handle, uint32_t *output, size_t count,
                   char *error, size_t error_capacity);
/* Consumes one owned handle; NULL is allowed. No GPU work remains in flight. */
void ck_metal_destroy(CKMetalHandle *handle);
#endif
