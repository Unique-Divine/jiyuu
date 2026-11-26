/* Private ABI for the frontend's bounded request queue. */
#ifndef CBM_DAEMON_FRONTEND_SESSION_H
#define CBM_DAEMON_FRONTEND_SESSION_H

#include "daemon/runtime.h"

typedef struct mori_frontend_queue mori_frontend_queue_t;

typedef struct {
    char *message;
    size_t length;
    bool content_length_framed;
    bool has_id;
    int64_t id;
    char *id_str;
    cbm_daemon_runtime_application_token_t request_token;
    bool cancelled;
} frontend_item_t;

enum {
    FRONTEND_CANCELLATION_STALE = 1,
    FRONTEND_CANCELLATION_QUEUED = 2,
    FRONTEND_CANCELLATION_ACTIVE = 3,
};

/* The frontend mutex serializes access. The queue owns admitted buffers until
 * pop transfers them to the worker or free discards them after both threads
 * join. After pop, finish releases the queue's borrow of the active ID. */
mori_frontend_queue_t *mori_frontend_queue_new(void (*free_item_buffer)(void *));
void mori_frontend_queue_free(mori_frontend_queue_t *queue);
bool mori_frontend_queue_is_empty(const mori_frontend_queue_t *queue);

/* 1 takes ownership, 0 asks the reader to wait, -1 means the item can never fit. */
int mori_frontend_queue_push(mori_frontend_queue_t *queue, const frontend_item_t *item);
/* 1 transfers the item and publishes its active ID, 0 is empty, -1 means
 * token reservation failed. Queued cancellations never reserve a token. */
int mori_frontend_queue_pop(
    mori_frontend_queue_t *queue, cbm_daemon_runtime_client_t *client,
    bool (*reserve)(cbm_daemon_runtime_client_t *, cbm_daemon_runtime_application_token_t *),
    frontend_item_t *item_out);
void mori_frontend_queue_finish(mori_frontend_queue_t *queue);
int mori_frontend_queue_cancel(mori_frontend_queue_t *queue, const char *params,
                               bool (*matches)(const char *, int64_t, const char *));

#endif
