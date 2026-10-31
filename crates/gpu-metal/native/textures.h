#ifndef CK_METAL_TEXTURES_H
#define CK_METAL_TEXTURES_H
#include <Availability.h>
#include <stddef.h>
#include <stdatomic.h>
#include <stdint.h>

typedef struct CKMcContext CKMcContext;
typedef struct CKMcTexture CKMcTexture;
typedef struct CKMcBuffer CKMcBuffer;
typedef struct CKMcCommand CKMcCommand;
typedef struct CKMcBatch CKMcBatch;
typedef struct CKMcToken CKMcToken;
typedef struct CKMcFrameReader CKMcFrameReader;
typedef struct CKMcResidency CKMcResidency;
typedef struct CKMcTables CKMcTables;

void ck_mc_live_objects(uint64_t *contexts, uint64_t *textures, uint64_t *batches,
                        uint64_t *frame_readers, uint64_t *residency_sets,
                        uint64_t *residency_members, uint64_t *last_residency_remove_order,
                        uint64_t *last_texture_release_order);

CKMcContext *ck_mc_context_new(char *error, size_t capacity);
void ck_mc_context_free(CKMcContext *context);
/* The context's device and command queue, each as one retained reference (the caller takes ownership of
 * it, as a `+1` object pointer): the presenter takes its device and queue from the shared context. */
void *ck_mc_context_device(CKMcContext *context);
void *ck_mc_context_queue(CKMcContext *context);
/* The identity of the context's device (its object address), for the tests: the same number the presenter
 * records for the device it took from the context. */
int64_t ck_mc_context_device_id(CKMcContext *context);
int64_t ck_mc_context_queue_id(CKMcContext *context);
uint64_t ck_mc_texture_estimate(CKMcContext *context, uint32_t width, uint32_t height);
CKMcTexture *ck_mc_texture_new(CKMcContext *context, uint32_t width, uint32_t height,
                               uint64_t *allocated, char *error, size_t capacity);
void ck_mc_texture_free(CKMcTexture *texture);
uint64_t ck_mc_texture_size(const CKMcTexture *texture);
/* The Metal texture (an `id<MTLTexture>`, not retained: it lives as long as the holder) the scene presenter binds. */
void *ck_mc_texture_metal(const CKMcTexture *texture);
int ck_mc_texture_read(CKMcTexture *texture, uint8_t *pixels, size_t length,
                       char *error, size_t capacity);
int ck_mc_measure_shared(CKMcContext *context, uint32_t width, uint32_t height, size_t count,
                         const uint8_t *images, size_t image_length,
                         char *error, size_t capacity);

CKMcBatch *ck_mc_batch_new(CKMcContext *context, uint64_t staging_need,
                           int32_t nil_staging, int32_t nil_command, int32_t nil_encoder,
                           uint64_t *allocated, char *error, size_t capacity);
CKMcTexture *ck_mc_batch_texture(CKMcBatch *batch, uint32_t width, uint32_t height,
                                 int32_t fail_allocation, uint64_t *allocated,
                                 char *error, size_t capacity);
void ck_mc_batch_discard_texture(CKMcBatch *batch, CKMcTexture *texture);
int ck_mc_batch_copy(CKMcBatch *batch, CKMcTexture *texture, uint32_t width, uint32_t height,
                     uint64_t offset, const uint8_t *pixels, size_t length,
                     char *error, size_t capacity);
int ck_mc_batch_close(CKMcBatch *batch, char *error, size_t capacity);
typedef void (*CKMcCompletionWake)(void *userdata);
/* `completion_gate` (test only; NULL in production): when given, completion publication is delayed off Metal's
 * completion thread (bounded, 30 s) until the gate reads non-zero, so a test decides when a submitted batch may arrive.
 * Test hooks never block Metal's completion thread. */
CKMcToken *ck_mc_batch_commit(CKMcBatch *batch, CKMcCompletionWake callback, void *userdata,
                              int32_t fail_commit, int32_t error_completion,
                              uint32_t completion_delay_ms, const _Atomic int32_t *completion_gate,
                              char *error, size_t capacity);
/* A completion token for a command buffer the presenter owns: allocated before the commit, completed by the
 * presenter's own completion handler (which signals it before the presenter's own token, so the ledger's
 * token is never later than the presenter's). `error_text` NULL is success. After the call the handler touches
 * the token no more. */
CKMcToken *ck_mc_command_token_new(char *error, size_t capacity);
void ck_mc_command_token_complete(CKMcToken *token, const char *error_text);
int ck_mc_token_done(CKMcToken *token);
int ck_mc_token_status(CKMcToken *token, char *error, size_t capacity);
void ck_mc_token_free(CKMcToken *token);
uint64_t ck_mc_batch_staging_size(const CKMcBatch *batch);
void ck_mc_batch_free(CKMcBatch *batch);

CKMcFrameReader *ck_mc_frame_reader_new(CKMcContext *context, CKMcTexture *source,
                                        char *error, size_t capacity);
CKMcToken *ck_mc_frame_reader_prepare(CKMcFrameReader *reader, uint32_t completion_delay_ms,
                                      int32_t error_completion, char *error, size_t capacity);
int ck_mc_frame_reader_commit(CKMcFrameReader *reader, char *error, size_t capacity);
int ck_mc_frame_reader_read(CKMcFrameReader *reader, uint8_t *pixels, size_t length,
                            char *error, size_t capacity);
void ck_mc_frame_reader_free(CKMcFrameReader *reader);

CKMcResidency *ck_mc_residency_new(CKMcContext *context, uint32_t slots, int32_t force_tier1,
                                   int32_t fail_creation, char *error, size_t capacity) API_AVAILABLE(macos(15.0));
void ck_mc_residency_add(CKMcResidency *set, void *texture) API_AVAILABLE(macos(15.0));
void ck_mc_residency_remove(CKMcResidency *set, void *texture) API_AVAILABLE(macos(15.0));
void ck_mc_residency_commit(CKMcResidency *set) API_AVAILABLE(macos(15.0));
uint64_t ck_mc_residency_commit_count(CKMcResidency *set) API_AVAILABLE(macos(15.0));
uint64_t ck_mc_residency_size(CKMcResidency *set) API_AVAILABLE(macos(15.0));
void ck_mc_residency_free(CKMcResidency *set) API_AVAILABLE(macos(15.0));

/* Two shared-storage tables of direct MTLResourceID values. Table handles and buffers are owned by this holder. */
CKMcTables *ck_mc_tables_new(CKMcContext *context, uint32_t slots, uint64_t *allocated,
                             char *error, size_t capacity) API_AVAILABLE(macos(13.0));
uint64_t ck_mc_texture_resource_id(CKMcTexture *texture) API_AVAILABLE(macos(13.0));
void *ck_mc_tables_buffer(CKMcTables *tables, uint32_t version) API_AVAILABLE(macos(13.0));
int ck_mc_tables_write(CKMcTables *tables, uint32_t version, uint32_t slot, uint64_t resource_id)
    API_AVAILABLE(macos(13.0));
uint64_t ck_mc_tables_size(CKMcTables *tables) API_AVAILABLE(macos(13.0));
void ck_mc_tables_free(CKMcTables *tables) API_AVAILABLE(macos(13.0));

#endif
